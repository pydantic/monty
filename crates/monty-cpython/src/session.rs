//! The protocol state machine: turns `pb::ParentRequest`s into `pb::ChildEvent`s
//! by running feeds in embedded CPython.
//!
//! Mirrors the strict alternation of `monty subprocess` (one request in, zero
//! or more `Print` events, then exactly one turn-ender) but uses a *blocking*
//! host-call model: an undefined name suspends and resumes entirely inside the
//! feed (a `NameLookup` to resolve it, then a `FunctionCall` if it is called —
//! see [`crate::pyexec::SandboxGlobals`]), so the top-level loop only ever sees
//! `Feed → Complete/Error`. `ResumeCall` and `ResumeNameLookup` therefore never
//! reach the top level (they are consumed inline), and Dump/Load/ResumeFutures
//! are not supported.

use std::process::ExitCode;

use monty::ExcType;
use monty_proto::{
    MONTY_VERSION, exceeds_max_value_depth, pb,
    python::{DcRegistry, exc_py_to_monty, monty_to_py, py_to_monty_value},
    validate_requirement,
};
use pyo3::prelude::*;

use crate::{
    events::{complete_event, error_event, error_from_exception, fatal_event, ok_event, violation},
    install::InstallEnv,
    pep_723,
    pyexec::{Runner, SandboxGlobals, Stdio},
    traceback::py_traceback_frames,
    transport::{Incoming, SendError, SharedTransport},
};

/// The child's version tag, compared against `Configure.monty_version`.
/// Shared with the rest of the protocol so all sides agree on one value.
const CHILD_VERSION: &str = MONTY_VERSION;

/// What the run loop should do after handling one request.
enum Flow {
    /// Send this turn-ending event and keep serving.
    Reply(pb::ChildEvent),
    /// Send this final event, then exit with this code.
    Exit { event: pb::ChildEvent, code: ExitCode },
}

/// REPL session state.
enum State {
    /// No session; only `Configure` / `Reset` / `Shutdown` are valid.
    Idle,
    /// A session is open: `namespace` persists across feeds. It *is* the
    /// `SandboxGlobals` (a `dict` subclass that bridges undefined names to the
    /// host). `sys.stdout`/`sys.stderr` are separate `Stdio` sinks kept alive by
    /// the interpreter, so they need no Rust-side handle here. `install` holds the
    /// session's `uv` install dir, created lazily on the first `InstallDependencies`.
    /// `script_name` is the parent-visible filename used for tracebacks and syntax
    /// errors.
    Ready {
        namespace: Py<SandboxGlobals>,
        install: Option<InstallEnv>,
        /// Parent-visible filename reported in tracebacks and syntax errors.
        script_name: String,
    },
}

/// All child state for one connection.
pub struct Session {
    transport: SharedTransport,
    /// Compiled `RUNNER` module providing the REPL runner (`run`).
    runner: Runner,
    state: State,
}

impl Session {
    /// Builds a session over `transport`, compiling the Python runner once.
    pub fn new(py: Python<'_>, transport: SharedTransport) -> PyResult<Self> {
        Ok(Self {
            transport,
            runner: Runner::new(py)?,
            state: State::Idle,
        })
    }

    /// Serves requests until the parent shuts down, closes the connection, or
    /// the stream breaks. Returns the process exit code.
    pub fn run(&mut self, py: Python<'_>) -> ExitCode {
        loop {
            let incoming = self.transport.borrow_mut().recv();
            match incoming {
                Incoming::Request(request) => match self.handle(py, request) {
                    Flow::Reply(event) => match self.transport.borrow_mut().send(&event) {
                        Ok(()) => {}
                        // The turn-ender itself didn't fit the wire frame (e.g. a
                        // huge `Complete` value). This is recoverable: replace it
                        // with a small error and keep serving rather than crashing
                        // the whole session over one oversized result.
                        Err(SendError::TooLarge { len, max }) => {
                            let replacement = error_event(
                                ExcType::RuntimeError,
                                &format!("result value of {len} bytes exceeds the maximum frame of {max} bytes"),
                            );
                            if self.transport.borrow_mut().send(&replacement).is_err() {
                                return ExitCode::from(3);
                            }
                        }
                        // The peer is gone — nothing left to do.
                        Err(SendError::Io(_)) => return ExitCode::from(3),
                    },
                    Flow::Exit { event, code } => {
                        let _ = self.transport.borrow_mut().send(&event);
                        return code;
                    }
                },
                // Clean EOF at a frame boundary: the parent closed the connection.
                Incoming::Eof => return ExitCode::SUCCESS,
                // A framed-but-undecodable request leaves the stream synced; answer
                // with an error and keep serving.
                Incoming::Malformed(msg) => {
                    let event = violation(&format!("malformed request: {msg}"));
                    if self.transport.borrow_mut().send(&event).is_err() {
                        return ExitCode::from(3);
                    }
                }
                // The stream desynchronized — unrecoverable.
                Incoming::Fatal(msg) => {
                    let _ = self
                        .transport
                        .borrow_mut()
                        .send(&fatal_event(&format!("malformed request frame: {msg}")));
                    return ExitCode::from(2);
                }
            }
        }
    }

    /// Handles one request, producing exactly one turn-ending event (or an exit).
    fn handle(&mut self, py: Python<'_>, request: pb::ParentRequest) -> Flow {
        let Some(kind) = request.kind else {
            return Flow::Reply(violation("request has no kind"));
        };
        match kind {
            pb::parent_request::Kind::Configure(configure) => {
                // Version skew is fatal: the protocol has no in-band negotiation,
                // and a mismatched build can frame differently.
                if configure.monty_version != CHILD_VERSION {
                    let message = format!(
                        "version skew: parent={:?} child={CHILD_VERSION:?}",
                        configure.monty_version
                    );
                    return Flow::Exit {
                        event: fatal_event(&message),
                        code: ExitCode::from(4),
                    };
                }
                self.handle_configure(py, &configure)
            }
            pb::parent_request::Kind::Feed(feed) => Flow::Reply(self.handle_feed(py, feed)),
            pb::parent_request::Kind::InstallDependencies(req) => Flow::Reply(self.handle_install(py, &req)),
            // A monty-cpython worker serves exactly one session per process — there
            // is no in-process reuse (a checkout dials a fresh worker). So `Reset`
            // ("end the session") and `Shutdown` ("end the worker") are the same
            // thing: acknowledge and exit, letting the OS reclaim the interpreter,
            // its `sys.modules`, and any install dir.
            pb::parent_request::Kind::Reset(_) | pb::parent_request::Kind::Shutdown(_) => Flow::Exit {
                event: ok_event(),
                code: ExitCode::SUCCESS,
            },
            // A blocking name lookup / host call consumes its own ResumeNameLookup /
            // ResumeCall inline, so one at the top level means the parent is out of step.
            pb::parent_request::Kind::ResumeCall(_) => {
                Flow::Reply(violation("unexpected ResumeCall: no host call is suspended"))
            }
            pb::parent_request::Kind::ResumeNameLookup(_) => {
                Flow::Reply(violation("unexpected ResumeNameLookup: no name lookup is suspended"))
            }
            pb::parent_request::Kind::ResumeFutures(_) => {
                Flow::Reply(violation("ResumeFutures is not supported by the CPython worker"))
            }
            pb::parent_request::Kind::Dump(_) => Flow::Reply(violation("Dump is not supported by the CPython worker")),
            pb::parent_request::Kind::Load(_) => Flow::Reply(violation("Load is not supported by the CPython worker")),
        }
    }

    /// Opens a fresh CPython session: a new namespace whose undefined names route
    /// to the parent, with `sys.stdout` pointed at the bridge.
    ///
    /// A failed open is fatal: the interpreter could not be initialised, which is
    /// not recoverable on this worker, so we honour the `fatal_event` contract and
    /// exit after telling the parent (which discards and replaces the worker)
    /// rather than emitting a fatal event yet continuing to serve.
    fn handle_configure(&mut self, py: Python<'_>, configure: &pb::Configure) -> Flow {
        if !matches!(self.state, State::Idle) {
            return Flow::Reply(violation("Configure while a session already exists"));
        }
        match self.open_session(py, configure.script_name.clone()) {
            Ok(()) => Flow::Reply(ok_event()),
            Err(err) => Flow::Exit {
                event: fatal_event(&format!("failed to start CPython session: {err}")),
                code: ExitCode::from(5),
            },
        }
    }

    /// Builds the namespace and routes sandbox stdout/stderr to the parent. The
    /// namespace *is* the [`SandboxGlobals`]: a `dict` subclass whose `__missing__`
    /// resolves undefined globals through the host. `sys.stdout`/`sys.stderr` are
    /// separate [`Stdio`] sinks (each `print()` chunk becomes a `Print` event
    /// tagged with its stream).
    fn open_session(&mut self, py: Python<'_>, script_name: String) -> PyResult<()> {
        let globals = Bound::new(py, SandboxGlobals::new(py, self.transport.clone()))?;
        // Sandboxed code runs as the top-level script, so `__name__` is
        // `'__main__'` (lets `if __name__ == '__main__':` guards fire). Seed it as
        // a real dict entry — like CPython's `__main__` module — so it resolves
        // from the namespace, not through the host `__missing__` path.
        globals.set_item("__name__", "__main__")?;
        let sys = py.import("sys")?;
        sys.setattr(
            "stdout",
            Bound::new(py, Stdio::new(self.transport.clone(), pb::PrintStream::Stdout))?,
        )?;
        sys.setattr(
            "stderr",
            Bound::new(py, Stdio::new(self.transport.clone(), pb::PrintStream::Stderr))?,
        )?;
        self.state = State::Ready {
            namespace: globals.unbind(),
            install: None,
            script_name,
        };
        Ok(())
    }

    /// Installs the requested packages into the session and replies `Ok`, or
    /// `Error` (carrying uv's stderr) on failure; an empty request is a no-op.
    /// Valid only once a session exists.
    fn handle_install(&mut self, py: Python<'_>, req: &pb::InstallDependencies) -> pb::ChildEvent {
        if !matches!(self.state, State::Ready { .. }) {
            return violation("InstallDependencies without a session");
        }
        if req.requirements.is_empty() {
            return ok_event();
        }
        if let Err(message) = validate_requirements(&req.requirements) {
            return error_event(ExcType::ValueError, &message);
        }
        match self.install_requirements(py, &req.requirements) {
            Ok(()) => ok_event(),
            Err(message) => error_event(ExcType::RuntimeError, &message),
        }
    }

    /// Installs `requirements` with `uv` into the session's install dir (created
    /// lazily on first use) and makes them importable. Shared by explicit
    /// `InstallDependencies` requests and the per-feed PEP 723 auto-install.
    /// Returns `Err(message)` (uv's stderr, or a setup failure) on failure.
    fn install_requirements(&mut self, py: Python<'_>, requirements: &[String]) -> Result<(), String> {
        let State::Ready { install, .. } = &mut self.state else {
            return Err("no session".to_owned());
        };
        if install.is_none() {
            let env = InstallEnv::create().map_err(|err| format!("failed to create install directory: {err}"))?;
            *install = Some(env);
        }
        let env = install.as_mut().expect("install dir was just created");
        env.install(py, requirements)
    }

    /// Runs one snippet to completion, returning `Complete` (the trailing
    /// expression's value) or `Error`. Before executing, any dependencies the
    /// snippet declares in a PEP 723 `# /// script` block are installed.
    fn handle_feed(&mut self, py: Python<'_>, feed: pb::Feed) -> pb::ChildEvent {
        if !matches!(self.state, State::Ready { .. }) {
            return violation("Feed without a session");
        }

        // PEP 723: install dependencies declared inline in the snippet before
        // running it, so its imports resolve. A snippet without a metadata block
        // yields an empty list (the common, fast path).
        match pep_723::dependencies(&feed.code) {
            Ok(deps) if !deps.is_empty() => {
                if let Err(message) = validate_requirements(&deps) {
                    return error_event(ExcType::ValueError, &message);
                }
                if let Err(message) = self.install_requirements(py, &deps) {
                    return error_event(ExcType::RuntimeError, &message);
                }
            }
            Ok(_) => {}
            Err(err) => return error_event(ExcType::ValueError, &err.to_string()),
        }

        let State::Ready {
            namespace, script_name, ..
        } = &self.state
        else {
            return violation("Feed without a session");
        };
        let namespace = namespace.bind(py).clone();
        let script_name = script_name.clone();
        let dc = DcRegistry::new(py);

        if let Some(event) = bind_inputs(py, &namespace, &dc, feed.inputs) {
            return event;
        }

        match self.runner.run(py, feed.code, &namespace, &script_name) {
            Ok(value) => match py_to_monty_value(&value, &dc) {
                Ok(value) if exceeds_max_value_depth(&value) => error_event(
                    ExcType::RuntimeError,
                    "result value is too deeply nested to send over the wire",
                ),
                Ok(value) => complete_event(value),
                Err(exc) => error_from_exception(&exc),
            },
            Err(err) => {
                // The sandbox raised: convert the type/message, then rebuild the
                // CPython traceback into structured frames (reported under
                // `script_name`) so the parent gets a real stack with source
                // previews and carets rather than a bare `Type: message`.
                let mut exc = exc_py_to_monty(py, &err);
                exc.add_traceback(py_traceback_frames(py, &self.runner, &err, &script_name));
                error_from_exception(&exc)
            }
        }
    }
}

/// Validates all requirement strings before they reach uv.
///
/// The Rust pool validates explicit `InstallDependencies` requests before
/// sending them, but the CPython worker also validates both explicit requests
/// and PEP 723 auto-installs so non-Rust parents get the same guard.
fn validate_requirements(requirements: &[String]) -> Result<(), String> {
    for requirement in requirements {
        validate_requirement(requirement)?;
    }
    Ok(())
}

/// Binds the feed's input globals into the namespace, returning `Some(event)`
/// with the `Error` to send if an input value is malformed, else `None`.
fn bind_inputs(
    py: Python<'_>,
    namespace: &Bound<'_, SandboxGlobals>,
    dc: &DcRegistry,
    inputs: Vec<pb::NamedValue>,
) -> Option<pb::ChildEvent> {
    for input in inputs {
        // A `None` payload is an *absent* protobuf field, not Python `None`
        // (which arrives as a present `MontyObject`). A named input with no
        // value is a malformed frame, so surface it rather than binding nothing.
        let Some(value) = input.value else {
            return Some(violation(&format!("input '{}' has no value", input.name)));
        };
        let object = match value.into_object() {
            Ok(value) => match monty_to_py(py, &value, dc) {
                Ok(object) => object,
                Err(err) => return Some(error_from_exception(&exc_py_to_monty(py, &err))),
            },
            Err(err) => {
                return Some(error_event(
                    ExcType::RuntimeError,
                    &format!("invalid input value: {err}"),
                ));
            }
        };
        if let Err(err) = namespace.set_item(input.name, object) {
            return Some(error_from_exception(&exc_py_to_monty(py, &err)));
        }
    }
    None
}
