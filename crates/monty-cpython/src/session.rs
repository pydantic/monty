//! The protocol state machine: turns `pb::ParentRequest`s into `pb::ChildEvent`s
//! by running feeds in embedded CPython.
//!
//! Mirrors the strict alternation of `monty subprocess` (one request in, zero
//! or more `Print` events, then exactly one turn-ender) but uses a *blocking*
//! host-call model: an undefined name suspends and resumes entirely inside the
//! feed (see [`crate::pyexec::HostBridge`]), so the top-level loop only ever
//! sees `Feed → Complete/Error`. `ResumeCall` therefore never reaches the top
//! level, and Dump/Load/ResumeNameLookup/ResumeFutures are not supported.

use std::process::ExitCode;

use _monty::{
    convert::{monty_to_py, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::exc_py_to_monty,
};
use monty::ExcType;
use monty_proto::{exceeds_max_value_depth, pb};
use pyo3::{prelude::*, types::PyModule};

use crate::{
    events::{complete_event, error_event, error_from_exception, fatal_event, ok_event, violation},
    pyexec::{HostBridge, init_runner},
    transport::{Incoming, SharedTransport},
};

/// The child's version tag, compared against `Configure.monty_version`.
/// Workspace-versioned so it matches the pool that drives it.
const CHILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the run loop should do after handling one request.
enum Flow {
    /// Send this turn-ending event and keep serving.
    Reply(pb::ChildEvent),
    /// Optionally send a final event, then exit with this code.
    Exit {
        event: Option<pb::ChildEvent>,
        code: ExitCode,
    },
}

/// REPL session state.
enum State {
    /// No session; only `Configure` / `Reset` / `Shutdown` are valid.
    Idle,
    /// A session is open: `namespace` persists across feeds. The `HostBridge`
    /// that bridges undefined names and `print()` is kept alive by Python (the
    /// namespace's `_host` and `sys.stdout` both reference it), so it needs no
    /// Rust-side handle here.
    Ready { namespace: Py<PyAny> },
}

/// All child state for one connection.
pub struct Session {
    transport: SharedTransport,
    /// Compiled `PREAMBLE` module (`_CallbackGlobals`, `_run`).
    runner: Py<PyModule>,
    state: State,
}

impl Session {
    /// Builds a session over `transport`, compiling the Python runner once.
    pub fn new(py: Python<'_>, transport: SharedTransport) -> PyResult<Self> {
        Ok(Self {
            transport,
            runner: init_runner(py)?,
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
                    Flow::Reply(event) => {
                        if self.transport.borrow_mut().send(&event).is_err() {
                            return ExitCode::from(3);
                        }
                    }
                    Flow::Exit { event, code } => {
                        if let Some(event) = event {
                            let _ = self.transport.borrow_mut().send(&event);
                        }
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
                        event: Some(fatal_event(&message)),
                        code: ExitCode::from(4),
                    };
                }
                Flow::Reply(self.handle_configure(py, &configure))
            }
            pb::parent_request::Kind::Feed(feed) => Flow::Reply(self.handle_feed(py, feed)),
            pb::parent_request::Kind::Reset(_) => {
                self.state = State::Idle;
                Flow::Reply(ok_event())
            }
            pb::parent_request::Kind::Shutdown(_) => Flow::Exit {
                event: Some(ok_event()),
                code: ExitCode::SUCCESS,
            },
            // A blocking host call consumes its own ResumeCall, so one at the top
            // level means the parent is out of step.
            pb::parent_request::Kind::ResumeCall(_) => {
                Flow::Reply(violation("unexpected ResumeCall: no host call is suspended"))
            }
            pb::parent_request::Kind::ResumeNameLookup(_) => {
                Flow::Reply(violation("ResumeNameLookup is not supported by the CPython worker"))
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
    fn handle_configure(&mut self, py: Python<'_>, _configure: &pb::Configure) -> pb::ChildEvent {
        if !matches!(self.state, State::Idle) {
            return violation("Configure while a session already exists");
        }
        match self.open_session(py) {
            Ok(()) => ok_event(),
            Err(err) => fatal_event(&format!("failed to start CPython session: {err}")),
        }
    }

    /// Builds the bridge + namespace and routes sandbox stdout through it.
    fn open_session(&mut self, py: Python<'_>) -> PyResult<()> {
        let host = Py::new(py, HostBridge::new(py, self.transport.clone()))?;
        let runner = self.runner.bind(py);
        let namespace = runner
            .getattr("_CallbackGlobals")?
            .call1((host.clone_ref(py),))?
            .unbind();
        py.import("sys")?.setattr("stdout", host.bind(py))?;
        self.state = State::Ready { namespace };
        Ok(())
    }

    /// Runs one snippet to completion, returning `Complete` (the trailing
    /// expression's value) or `Error`.
    fn handle_feed(&mut self, py: Python<'_>, feed: pb::Feed) -> pb::ChildEvent {
        let State::Ready { namespace, .. } = &self.state else {
            return violation("Feed without a session");
        };
        let namespace = namespace.bind(py).clone();
        let dc = DcRegistry::new(py);

        if let Some(event) = bind_inputs(py, &namespace, &dc, feed.inputs) {
            return event;
        }

        let runner = self.runner.bind(py);
        match runner
            .getattr("_run")
            .and_then(|run| run.call1((feed.code, &namespace)))
        {
            Ok(value) => match py_to_monty_value(&value, &dc) {
                Ok(value) if exceeds_max_value_depth(&value) => error_event(
                    ExcType::RuntimeError,
                    "result value is too deeply nested to send over the wire",
                ),
                Ok(value) => complete_event(value),
                Err(exc) => error_from_exception(&exc),
            },
            Err(err) => error_from_exception(&exc_py_to_monty(py, &err)),
        }
    }
}

/// Binds the feed's input globals into the namespace, returning `Some(event)`
/// with the `Error` to send if an input value is malformed, else `None`.
fn bind_inputs(
    py: Python<'_>,
    namespace: &Bound<'_, PyAny>,
    dc: &DcRegistry,
    inputs: Vec<pb::NamedValue>,
) -> Option<pb::ChildEvent> {
    for input in inputs {
        let Some(value) = input.value else { continue };
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
