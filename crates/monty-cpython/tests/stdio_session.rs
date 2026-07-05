//! Drives the CPython child over an in-memory transport that plays the parent:
//! it scripts a session (Configure → Feeds → Shutdown) and answers the
//! `NameLookup`s and `FunctionCall`s the child emits from a small
//! external-function table (and a host-value table), exactly as a real parent would.
//!
//! These tests share one `auto-initialize` interpreter across the cargo test
//! harness's threads, so you may see a stray `SandboxGlobals is unsendable, but is
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

use monty::{ExtFunctionResult, MontyException, MontyObject, NameLookupResult};
use monty_cpython::{
    run_with_transport,
    transport::{Incoming, SendError, Transport},
};
use monty_proto::pb;
use pyo3::{Py, PyAny, Python, prelude::*};

type External = Box<dyn Fn(&[MontyObject]) -> ExtFunctionResult>;

/// Serializes the tests: they share a single process-wide embedded interpreter,
/// and the GIL switches threads mid-execution, so two sessions running at once
/// would race on global interpreter state (`sys.stdout`, `sys.path`). Each test
/// drives the child under this lock via [`drive`].
static INTERPRETER: Mutex<()> = Mutex::new(());

/// Runs `parent`'s scripted session to completion while holding [`INTERPRETER`]
/// (poison is ignored — a panicking test leaves no shared invariant broken).
///
/// A session installs thread-bound (`unsendable`) `Stdio` sinks as
/// `sys.stdout`/`sys.stderr`. In the real worker the process exits after one
/// session, but here the interpreter is shared across the harness's threads, so a
/// later test's GC of this session's objects could write a warning to a stale
/// sink from the wrong thread and panic. Snapshot the real streams and restore
/// them after the run — both to leave thread-safe streams installed between tests
/// and to drop this session's `Stdio` sinks on their own thread.
fn drive(parent: ScriptedParent) {
    let _guard = INTERPRETER.lock().unwrap_or_else(PoisonError::into_inner);
    let saved = Python::attach(|py| {
        let sys = py.import("sys").expect("import sys");
        let get = |name: &str| -> Py<PyAny> { sys.getattr(name).expect("sys stream").unbind() };
        (get("stdout"), get("stderr"))
    });
    let _ = run_with_transport(Box::new(parent));
    Python::attach(|py| {
        let sys = py.import("sys").expect("import sys");
        sys.setattr("stdout", saved.0.bind(py)).expect("restore stdout");
        sys.setattr("stderr", saved.1.bind(py)).expect("restore stderr");
    });
}

/// An in-memory parent: replays a request script, answers `NameLookup`s from
/// `name_values` (host values) and `externals` (host functions), and answers
/// `FunctionCall`s from `externals`, capturing every event for assertions.
struct ScriptedParent {
    script: VecDeque<pb::ParentRequest>,
    pending_resume: Option<pb::ParentRequest>,
    externals: HashMap<String, External>,
    name_values: HashMap<String, MontyObject>,
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
        match &event.kind {
            // Mirror a real parent: a NameLookup resolves to a host value, a host
            // function, or undefined (see `resolve_pool_name_lookup`).
            Some(pb::child_event::Kind::NameLookup(lookup)) => {
                let result = if let Some(value) = self.name_values.get(&lookup.name) {
                    NameLookupResult::Value(value.clone())
                } else if self.externals.contains_key(&lookup.name) {
                    NameLookupResult::Value(MontyObject::Function {
                        name: lookup.name.clone(),
                        docstring: None,
                    })
                } else {
                    NameLookupResult::Undefined
                };
                self.pending_resume = Some(resume_name_lookup(result));
            }
            // A FunctionCall is answered with a ResumeCall.
            Some(pb::child_event::Kind::FunctionCall(call)) => {
                let result = match self.externals.get(&call.function_name) {
                    Some(handler) => handler(&call.args),
                    None => ExtFunctionResult::NotFound(call.function_name.clone()),
                };
                self.pending_resume = Some(resume_call(call.call_id, result));
            }
            _ => {}
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
        name_values: HashMap::new(),
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

    // The print streamed through as a stdout-tagged Print event.
    let printed: String = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Print(p) if p.stream == pb::PrintStream::Stdout as i32 => Some(p.text.clone()),
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
    // The error carries the full sandbox traceback: a single module-level frame
    // for the trailing `1 / 0` expression on line 1 (reported under the
    // configured `script_name`, driver frames filtered), with a source preview
    // and carets under the failing span.
    let rendered = MontyException::try_from(error.clone())
        .expect("valid exception")
        .to_string();
    assert_eq!(
        rendered,
        "Traceback (most recent call last):\n  \
         File \"main.py\", line 1, in <module>\n    \
         1 / 0\n    ~~~~~\n\
         ZeroDivisionError: division by zero"
    );
}

/// A sandbox exception raised through nested user frames carries a multi-frame
/// CPython traceback back to the parent: one frame per user frame, outermost
/// first, under the configured `script_name`, each with a source preview. Carets
/// follow CPython — shown under the `f()` call, hidden for the `raise` — and the
/// `runner.py` driver frames are filtered out.
#[test]
fn error_carries_multi_frame_traceback() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            // line 1: def f(): / line 2: raise / line 3: blank / line 4: f()
            feed("def f():\n    raise ValueError('boom')\n\nf()"),
            shutdown(),
        ]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    assert_eq!(error.exc_type, "ValueError");
    assert_eq!(error.message.as_deref(), Some("boom"));

    // Outermost first: the module-level `f()` call on line 4 (carets shown),
    // then the `raise` inside `f` on line 2 (carets hidden, matching CPython).
    let rendered = MontyException::try_from(error.clone())
        .expect("valid exception")
        .to_string();
    assert_eq!(
        rendered,
        "Traceback (most recent call last):\n  \
         File \"main.py\", line 4, in <module>\n    \
         f()\n    ~~~\n  \
         File \"main.py\", line 2, in f\n    \
         raise ValueError('boom')\n\
         ValueError: boom"
    );
}

/// Caret columns are character offsets, not bytes: a non-ASCII preview line
/// still underlines the right span. CPython reports the failing span as UTF-8
/// byte offsets (end byte 12 for the 11-character `'héllo' + 1`); the rebuilt
/// frame converts them to characters so the carets span all 11 characters.
#[test]
fn traceback_carets_are_character_aligned() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed("'héllo' + 1"), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    assert_eq!(error.exc_type, "TypeError");
    let rendered = MontyException::try_from(error.clone())
        .expect("valid exception")
        .to_string();
    assert_eq!(
        rendered,
        "Traceback (most recent call last):\n  \
         File \"main.py\", line 1, in <module>\n    \
         'héllo' + 1\n    ~~~~~~~~~~~\n\
         TypeError: can only concatenate str (not \"int\") to str"
    );
}

/// Source previews resolve across feeds: a function defined in one feed and
/// called from a later one still renders its own source line. This is why each
/// feed compiles under a unique internal filename with its source registered in
/// `linecache` — a single shared filename would collide on line numbers and show
/// the wrong (or no) preview for the earlier feed's frame.
#[test]
fn traceback_preview_resolves_across_feeds() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            // Feed 1 defines `boom` (the `return` is on line 2) and completes.
            feed("def boom():\n    return undefined_xyz"),
            // Feed 2 calls it; the NameError unwinds through feed 1's line 2.
            feed("boom()"),
            shutdown(),
        ]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    assert_eq!(error.exc_type, "NameError");
    // The `boom` frame's preview is feed 1's `return undefined_xyz`, even though
    // the error surfaced in feed 2 (whose source has no line 2).
    let rendered = MontyException::try_from(error.clone())
        .expect("valid exception")
        .to_string();
    assert_eq!(
        rendered,
        "Traceback (most recent call last):\n  \
         File \"main.py\", line 1, in <module>\n    \
         boom()\n    ~~~~~~\n  \
         File \"main.py\", line 2, in boom\n    \
         return undefined_xyz\n           ~~~~~~~~~~~~~\n\
         NameError: name 'undefined_xyz' is not defined"
    );
}

/// Syntax errors are raised before a user traceback frame exists, so the runner
/// rewrites the compile-time `<input-N>` filename directly on the `SyntaxError`.
#[test]
fn syntax_error_reports_configured_script_name() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed("1 +"), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    assert_eq!(error.exc_type, "SyntaxError");
    assert_eq!(error.message.as_deref(), Some("invalid syntax (main.py, line 1)"));
    assert!(error.traceback.is_empty());
}

/// A CRLF-fed snippet must not leave a carriage return in the preview line — a
/// stray `\r` would move the cursor and misrender the caret line.
#[test]
fn traceback_preview_strips_carriage_returns() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        // CRLF line endings; the ZeroDivisionError is on line 2.
        script: VecDeque::from([configure(), feed("a = 1\r\nb = a / 0"), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    let frame = error.traceback.last().expect("a frame");
    assert_eq!(frame.preview_line.as_deref(), Some("b = a / 0"));
}

/// The sandbox is full CPython, so user code can monkey-patch the `traceback`
/// module. A patch that breaks traceback extraction must degrade gracefully: the
/// exception's type and message still reach the parent, only the traceback drops
/// to empty (extraction is best-effort). A second feed restores the module — the
/// embedded interpreter is shared across tests.
#[test]
fn error_survives_sandbox_patching_traceback_module() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            feed(
                "import traceback\n\
                 _oe, _os = traceback.extract_tb, traceback.StackSummary\n\
                 traceback.extract_tb = None\n\
                 traceback.StackSummary = None\n\
                 raise ValueError('real')",
            ),
            feed("import traceback\ntraceback.extract_tb, traceback.StackSummary = _oe, _os"),
            shutdown(),
        ]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let error = events
        .iter()
        .filter_map(|e| e.kind.as_ref())
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");

    // The exception still propagates intact; only the traceback is lost, since
    // extraction calls the patched (broken) `traceback.extract_tb` and bails.
    assert_eq!(error.exc_type, "ValueError");
    assert_eq!(error.message.as_deref(), Some("real"));
    assert!(error.traceback.is_empty());
}

/// `sys.stdout` and `sys.stderr` are separate sinks: each `print()` chunk streams
/// as a `Print` event tagged with its stream, so the parent can tell them apart.
#[test]
fn stdout_and_stderr_are_separate_streams() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            feed("import sys\nprint('out')\nprint('err', file=sys.stderr)"),
            shutdown(),
        ]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let prints: Vec<(i32, String)> = events
        .iter()
        .filter_map(|e| match e.kind.as_ref() {
            Some(pb::child_event::Kind::Print(p)) => Some((p.stream, p.text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec![
            (pb::PrintStream::Stdout as i32, "out".to_string()),
            (pb::PrintStream::Stdout as i32, "\n".to_string()),
            (pb::PrintStream::Stderr as i32, "err".to_string()),
            (pb::PrintStream::Stderr as i32, "\n".to_string()),
        ]
    );
}

/// Sandboxed code runs as the top-level script: `__name__` is `'__main__'`
/// (seeded as a real namespace entry, so it resolves from the dict and never
/// triggers a host `NameLookup`), letting `if __name__ == '__main__':` fire.
#[test]
fn name_is_dunder_main() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed("ran = __name__ == '__main__'\n__name__"), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // `__name__` resolved from the namespace, not via a host NameLookup.
    assert!(
        !kinds.iter().any(|k| matches!(k, pb::child_event::Kind::NameLookup(_))),
        "no NameLookup for __name__: {kinds:?}"
    );
    let complete = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::String("__main__".to_string()));
}

/// An undefined name the host resolves to a plain *value* (not a function) is
/// returned by `__missing__` directly and used as a value, no `FunctionCall` involved.
#[test]
fn resolves_a_name_to_a_host_value() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed("n + 1"), shutdown()]),
        pending_resume: None,
        name_values: HashMap::from([("n".to_string(), MontyObject::Int(41))]),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // `n` was resolved via a NameLookup (and never as a FunctionCall).
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, pb::child_event::Kind::NameLookup(l) if l.name == "n")),
        "a NameLookup for n: {kinds:?}"
    );
    assert!(
        !kinds
            .iter()
            .any(|k| matches!(k, pb::child_event::Kind::FunctionCall(_))),
        "no FunctionCall: {kinds:?}"
    );
    // The host value (41) was used as a value: 41 + 1 == 42.
    let complete = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::Int(42));
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
        name_values: HashMap::new(),
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

/// A cell's body and its trailing expression run on a *single* event loop, so an
/// async object the body binds to the loop is still usable from the trailing
/// expression. Two `asyncio.run` calls (one per half) would bind the queue to a
/// loop closed before `get()` runs, raising "bound to a different event loop".
#[test]
fn top_level_await_shares_one_event_loop() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            // `q` is bound to the loop by `put` in the body; `get` in the trailing
            // expression must see the same loop to return the queued value.
            feed("import asyncio\nq = asyncio.Queue()\nawait q.put(7)\nawait q.get()"),
            shutdown(),
        ]),
        pending_resume: None,
        name_values: HashMap::new(),
        externals: HashMap::new(),
        events: events.clone(),
    };

    drive(parent);

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();
    assert!(
        !kinds.iter().any(|k| matches!(k, pb::child_event::Kind::Error(_))),
        "no Error events (a split loop would raise a RuntimeError): {kinds:?}"
    );
    let complete = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .expect("a Complete event");
    assert_eq!(complete, MontyObject::Int(7));
}

/// `InstallDependencies` before `Configure` has no session to install into, so
/// the child rejects it with a protocol-violation `Error` and keeps serving.
#[test]
fn install_without_session_is_rejected() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([install(&["anything"])]),
        pending_resume: None,
        name_values: HashMap::new(),
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
        name_values: HashMap::new(),
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

/// Requirement strings that uv would parse as command-line options are rejected
/// before the worker shells out.
#[test]
fn install_rejects_flag_like_requirement() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), install(&["-r /etc/hosts"]), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
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
    assert_eq!(
        error.message.as_deref(),
        Some("invalid requirement \"-r /etc/hosts\": must not start with '-' (it would be parsed as a uv option)")
    );
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
        name_values: HashMap::new(),
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
        name_values: HashMap::new(),
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
        name_values: HashMap::new(),
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

/// PEP 723 dependencies use the same validation as explicit
/// `InstallDependencies`, so inline metadata cannot smuggle uv options.
#[test]
fn pep723_rejects_flag_like_requirement() {
    let code = "# /// script\n# dependencies = [\"--index-url=http://evil\"]\n# ///\nprint('never runs')";
    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([configure(), feed(code), shutdown()]),
        pending_resume: None,
        name_values: HashMap::new(),
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
    assert_eq!(
        error.message.as_deref(),
        Some(
            "invalid requirement \"--index-url=http://evil\": must not start with '-' (it would be parsed as a uv option)"
        )
    );
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
        name_values: HashMap::new(),
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
        // Parent-visible filename reported in tracebacks and syntax errors.
        script_name: "main.py".to_string(),
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

fn resume_name_lookup(result: NameLookupResult) -> pb::ParentRequest {
    let kind = match result {
        NameLookupResult::Value(obj) => pb::resume_name_lookup::Kind::Value(obj.into()),
        NameLookupResult::Undefined => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
    };
    request(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
        kind: Some(kind),
    }))
}

fn request(kind: pb::parent_request::Kind) -> pb::ParentRequest {
    pb::ParentRequest { kind: Some(kind) }
}
