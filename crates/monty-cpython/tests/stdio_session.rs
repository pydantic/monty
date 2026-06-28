//! Drives the CPython child over an in-memory transport that plays the parent:
//! it scripts a session (Configure → Feeds → Shutdown) and answers each
//! `FunctionCall` the child emits from a small external-function table, exactly
//! as a real parent would.
//!
//! These tests share one `auto-initialize` interpreter across the cargo test
//! harness's threads, so you may see a stray `HostBridge is unsendable, but is
//! being dropped on another thread` line on stderr when the interpreter's cyclic
//! GC reclaims a session's objects on a harness thread other than the one that
//! created them. It is harmless (PyO3 skips the drop; the test still passes) and
//! cannot occur in the real worker, which serves one session on a single thread
//! end-to-end — verified by driving the actual binary over real stdio.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    env,
    path::Path,
    process::Command,
    rc::Rc,
    sync::{Mutex, PoisonError},
};

use monty::{ExtFunctionResult, MontyObject};
use monty_cpython::{
    run_with_transport,
    transport::{Incoming, SendError, Transport},
};
use monty_proto::pb;

type External = Box<dyn Fn(&[MontyObject]) -> ExtFunctionResult>;

/// Serializes the tests: they share a single process-wide embedded interpreter,
/// and the GIL switches threads mid-execution, so two sessions running at once
/// would race on global interpreter state (`sys.stdout`, `sys.path`). Each test
/// drives the child under this lock via [`drive`].
static INTERPRETER: Mutex<()> = Mutex::new(());

/// Runs `parent`'s scripted session to completion while holding [`INTERPRETER`]
/// (poison is ignored — a panicking test leaves no shared invariant broken).
fn drive(parent: ScriptedParent) {
    let _guard = INTERPRETER.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = run_with_transport(Box::new(parent));
}

/// An in-memory parent: replays a request script and answers `FunctionCall`s
/// from `externals`, capturing every event the child sends for assertions.
struct ScriptedParent {
    script: VecDeque<pb::ParentRequest>,
    pending_resume: Option<pb::ParentRequest>,
    externals: HashMap<String, External>,
    events: Rc<RefCell<Vec<pb::ChildEvent>>>,
}

impl Transport for ScriptedParent {
    fn recv(&mut self) -> Incoming {
        if let Some(resume) = self.pending_resume.take() {
            return Incoming::Request(resume);
        }
        match self.script.pop_front() {
            Some(request) => Incoming::Request(request),
            None => Incoming::Eof,
        }
    }

    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), SendError> {
        self.events.borrow_mut().push(event.clone());
        // Mirror a real parent: a FunctionCall is answered with a ResumeCall.
        if let Some(pb::child_event::Kind::FunctionCall(call)) = &event.kind {
            let result = match self.externals.get(&call.function_name) {
                Some(handler) => handler(&call.args),
                None => ExtFunctionResult::NotFound(call.function_name.clone()),
            };
            self.pending_resume = Some(resume_call(call.call_id, result));
        }
        Ok(())
    }
}

#[test]
fn drives_a_full_session() {
    let externals: HashMap<String, External> = HashMap::from([(
        "double".to_string(),
        Box::new(|args: &[MontyObject]| match args {
            [MontyObject::Int(n)] => ExtFunctionResult::Return(MontyObject::Int(n * 2)),
            _ => ExtFunctionResult::NotFound("double".to_string()),
        }) as External,
    )]);

    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            feed("double(21) + 1"),    // host call: 21 -> 42, + 1 -> 43
            feed("print('hello')\n7"), // streamed print, then trailing value
            feed("1 / 0"),             // raises, ends the turn with Error
            shutdown(),
        ]),
        pending_resume: None,
        externals,
        events: events.clone(),
    };

    // Runs to the scripted Shutdown and returns (exit code is not asserted —
    // `ExitCode` is opaque; the captured events below prove the behavior).
    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // First and last turn-enders are the Configure / Shutdown acks.
    assert!(
        matches!(kinds.first(), Some(pb::child_event::Kind::Ok(_))),
        "first event is Ok"
    );
    assert!(
        matches!(kinds.last(), Some(pb::child_event::Kind::Ok(_))),
        "last event is Ok"
    );

    // The host call was emitted with the converted argument.
    let call = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::FunctionCall(c) => Some(c),
            _ => None,
        })
        .expect("a FunctionCall event");
    assert_eq!(call.function_name, "double");
    assert_eq!(call.args, vec![MontyObject::Int(21)]);

    // Both feeds completed; collect the Complete values in order.
    let completes: Vec<MontyObject> = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(completes, vec![MontyObject::Int(43), MontyObject::Int(7)]);

    // The print streamed through as a stdout event.
    let printed: String = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Print(p) => Some(p.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(printed, "hello\n");

    // The dividing-by-zero feed ended with a ZeroDivisionError.
    let error = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");
    assert_eq!(error.exc_type, "ZeroDivisionError");
}

/// Top-level `await` is supported (`PyCF_ALLOW_TOP_LEVEL_AWAIT` + `asyncio.run`),
/// `__missing__` still resolves host calls inside coroutines, but a host call is
/// not itself awaitable.
#[test]
fn supports_top_level_await() {
    let externals: HashMap<String, External> = HashMap::from([(
        "double".to_string(),
        Box::new(|args: &[MontyObject]| match args {
            [MontyObject::Int(n)] => ExtFunctionResult::Return(MontyObject::Int(n * 2)),
            _ => ExtFunctionResult::NotFound("double".to_string()),
        }) as External,
    )]);

    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            // Top-level await of an async def that makes a (synchronous) host call:
            // proves the coroutine is driven AND that `__missing__` resolves the
            // undefined `double` from inside the coroutine.
            feed("async def f():\n    return double(21)\nawait f()"),
            // Top-level await in the body, then a trailing synchronous value.
            feed("import asyncio\nawait asyncio.sleep(0)\n5"),
            // Awaiting a host call is a TypeError: the proxy returns a plain value.
            feed("await double(21)"),
            shutdown(),
        ]),
        pending_resume: None,
        externals,
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // Both awaiting feeds completed with their values, in order.
    let completes: Vec<MontyObject> = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(completes, vec![MontyObject::Int(42), MontyObject::Int(5)]);

    // Awaiting the host call ended the third feed with a TypeError.
    let error = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");
    assert_eq!(error.exc_type, "TypeError");
}

/// `InstallDependencies` before `Configure` has no session to install into, so
/// the child rejects it with a protocol-violation `Error` and keeps serving.
#[test]
fn install_without_session_is_rejected() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([install(&["anything"])]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .find_map(|e| match e.kind.as_ref() {
            Some(pb::child_event::Kind::Error(err)) => err.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");
    assert_eq!(error.exc_type, "RuntimeError");
    assert_eq!(
        error.message.as_deref(),
        Some("protocol violation: InstallDependencies without a session")
    );
}

/// An empty requirement list is a no-op that acknowledges with `Ok` without
/// running uv or creating an install directory.
#[test]
fn empty_install_is_a_noop() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), install(&[]), shutdown()]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    // Configure, empty install, and Shutdown each acknowledge with Ok.
    let oks = events
        .iter()
        .filter(|e| matches!(e.kind, Some(pb::child_event::Kind::Ok(_))))
        .count();
    assert_eq!(oks, 3, "Configure + empty install + Shutdown all ack with Ok");
}

/// End-to-end install of a real package with `uv`, then importing it in a feed.
///
/// Ignored by default: it requires `uv` on `PATH` (or `MONTY_UV`) and network
/// access to a package index. Run explicitly with
/// `cargo test -p monty-cpython -- --ignored installs_and_imports_a_package`.
#[test]
#[ignore = "requires uv on PATH and network access to a package index"]
fn installs_and_imports_a_package() {
    ensure_test_venv();
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            install(&["six==1.16.0"]),
            feed("import six\nsix.__version__"),
            shutdown(),
        ]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // The install acknowledged with Ok (no Error from uv).
    assert!(
        !kinds.iter().any(|k| matches!(k, pb::child_event::Kind::Error(_))),
        "no Error events: {kinds:?}"
    );
    // The feed imported the freshly installed package and returned its version.
    let complete = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::String("1.16.0".to_string()));
}

/// An ordinary `#` comment is not a PEP 723 block, so no install is attempted
/// and the feed runs offline (a false trigger would shell out to uv and fail).
#[test]
fn ordinary_comments_do_not_trigger_pep723() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed("# just a comment\nx = 41\nx + 1"), shutdown()]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let complete = events
        .iter()
        .find_map(|e| match e.kind.as_ref() {
            Some(pb::child_event::Kind::Complete(c)) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::Int(42));
}

/// PEP 723 permits at most one `script` block; a snippet with two ends the feed
/// with a `ValueError` before any install or execution.
#[test]
fn pep723_multiple_blocks_is_an_error() {
    // A blank line between the blocks keeps them separate matches (without it the
    // greedy regex merges them into one, which is a TOML error instead).
    let code = "# /// script\n# dependencies = [\"a\"]\n# ///\n\n# /// script\n# dependencies = [\"b\"]\n# ///\n";
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed(code), shutdown()]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .find_map(|e| match e.kind.as_ref() {
            Some(pb::child_event::Kind::Error(err)) => err.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");
    assert_eq!(error.exc_type, "ValueError");
    assert_eq!(error.message.as_deref(), Some("multiple PEP 723 script blocks found"));
}

/// End-to-end PEP 723: a feed declaring a dependency in its inline metadata has
/// it installed (via `uv`) before the snippet runs, so the import resolves.
///
/// Ignored by default: requires `uv` on `PATH` (or `MONTY_UV`) and network
/// access. Run with `cargo test -p monty-cpython -- --ignored feed_installs_pep723`.
#[test]
#[ignore = "requires uv on PATH and network access to a package index"]
fn feed_installs_pep723_dependencies() {
    ensure_test_venv();
    let code = "# /// script\n# dependencies = [\"six==1.16.0\"]\n# ///\nimport six\nsix.__version__";
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed(code), shutdown()]),
        pending_resume: None,
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();
    assert!(
        !kinds.iter().any(|k| matches!(k, pb::child_event::Kind::Error(_))),
        "no Error events: {kinds:?}"
    );
    let complete = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::String("1.16.0".to_string()));
}

fn configure() -> pb::ParentRequest {
    request(pb::parent_request::Kind::Configure(pb::Configure {
        monty_version: env!("CARGO_PKG_VERSION").to_string(),
        ..Default::default()
    }))
}

fn install(requirements: &[&str]) -> pb::ParentRequest {
    request(pb::parent_request::Kind::InstallDependencies(pb::InstallDependencies {
        requirements: requirements.iter().map(ToString::to_string).collect(),
    }))
}

/// Creates `./.venv` if absent, standing in for the deployment image's `uv venv`
/// (the worker installs into — and refuses to create — `./.venv`). Assumes uv's
/// default Python matches the interpreter this test process embeds, so the venv's
/// `site-packages` is the one the worker adds to `sys.path`. Only the `#[ignore]`d
/// install tests (which already need uv + network) call this.
fn ensure_test_venv() {
    if !Path::new(".venv").is_dir() {
        let uv = env::var("MONTY_UV").unwrap_or_else(|_| "uv".to_string());
        let status = Command::new(uv)
            .args(["venv", ".venv"])
            .status()
            .expect("spawn uv venv");
        assert!(status.success(), "uv venv failed to create ./.venv");
    }
}

fn feed(code: &str) -> pb::ParentRequest {
    request(pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_string(),
        ..Default::default()
    }))
}

fn shutdown() -> pb::ParentRequest {
    request(pb::parent_request::Kind::Shutdown(pb::Shutdown {}))
}

fn resume_call(call_id: u32, result: ExtFunctionResult) -> pb::ParentRequest {
    request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id,
        result: Some(result.into()),
    }))
}

fn request(kind: pb::parent_request::Kind) -> pb::ParentRequest {
    pb::ParentRequest { kind: Some(kind) }
}
