//! Tests for aborting a suspended feed: the host raises an exception at the
//! suspension point instead of answering it, uncatchably, so a snippet that
//! retries refused calls cannot continue — how `max_suspensions` is enforced.

use insta::assert_snapshot;
use monty::{MontyRepl, MontyRun, RunProgress};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, NameLookupResult, PrintWriter,
    ResourceTracker,
};

/// The exception the pool aborts with once `max_suspensions` is spent.
fn limit_exceeded() -> MontyException {
    MontyException::new(
        ExcType::RuntimeError,
        Some("suspension limit exceeded: 4 > 3".to_owned()),
    )
}

/// Starts `code` and resolves every leading name lookup to a function.
fn start(code: &str) -> RunProgress {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = run
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup
            .resume(
                NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
                PrintWriter::Stdout,
            )
            .unwrap();
    }
    progress
}

/// The issue's shape: every refused call is caught and retried. Aborting the
/// external call ends the run even though the call sits inside `except
/// Exception`, and the traceback points at the call.
#[test]
fn abort_external_call_is_uncatchable() {
    let code = "\
def retry():
    while True:
        try:
            return fetch('x')
        except Exception:
            pass

retry()
";
    let call = start(code).into_function_call().expect("external call");
    let exc = call.abort(limit_exceeded(), PrintWriter::Stdout).unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("suspension limit exceeded: 4 > 3"));
    assert_snapshot!(exc.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 8, in <module>
        retry()
        ~~~~~~~
      File "test.py", line 4, in retry
        return fetch('x')
               ~~~~~~~~~~
    RuntimeError: suspension limit exceeded: 4 > 3
    "#);
}

/// OS calls abort the same way; the read's pending buffer-store effect is
/// rolled back (checked by the refcount harness) rather than applied to nothing.
#[test]
fn abort_os_call_after_open() {
    let code = "\
f = open('/data/x.txt')
try:
    f.read()
except Exception:
    pass
";
    let call = start(code).into_os_call().expect("open");
    assert_eq!(call.function_call.name(), "open");
    let handle = MontyObject::FileHandle(monty_types::MontyFileHandle {
        path: "/data/x.txt".to_owned(),
        mode: "r".parse().unwrap(),
        position: 0,
    });
    let read = call
        .resume(ExtFunctionResult::Return(handle), PrintWriter::Stdout)
        .unwrap()
        .into_os_call()
        .expect("read");
    assert_eq!(read.function_call.name(), "Path.read_text");
    let exc = read.abort(limit_exceeded(), PrintWriter::Stdout).unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.traceback().len(), 1);
}

/// A name lookup can be aborted before it is ever resolved.
#[test]
fn abort_name_lookup() {
    let run = MontyRun::new(
        "x = unknown_name\n".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = run
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::NameLookup(lookup) = progress else {
        panic!("expected a name lookup");
    };
    let exc = lookup.abort(limit_exceeded(), PrintWriter::Stdout).unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.traceback()[0].start.line, 1);
}

/// Aborting while blocked on external futures — after a partial resolution,
/// the re-suspension that a host would count as one more round trip.
#[test]
fn abort_resolve_futures_after_partial_resolution() {
    let code = "\
import asyncio

async def main():
    try:
        a, b = await asyncio.gather(foo(), bar())
    except Exception:
        return -1
    return a + b

await main()
";
    let mut progress = start(code);
    let mut call_ids = vec![];
    let state = loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                call_ids.push(call.call_id);
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(state) => break state,
            other => panic!("unexpected progress {other:?}"),
        }
    };
    let state = state
        .resume(
            vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(1)))],
            PrintWriter::Stdout,
        )
        .unwrap()
        .into_resolve_futures()
        .expect("still waiting on bar");
    let exc = state.abort(limit_exceeded(), PrintWriter::Stdout).unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
}

/// A REPL abort hands the session back usable: globals set before the
/// aborted snippet survive, and nothing after the suspension ran.
#[test]
fn repl_abort_keeps_the_session_usable() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("x = 41", vec![], PrintWriter::Stdout).unwrap();
    // calling an unknown name suspends at FunctionCall directly
    let call = repl
        .feed_start("fetch('x')\nx = 0", vec![], PrintWriter::Stdout)
        .unwrap()
        .into_function_call()
        .expect("fetch call");
    let err = call.abort(limit_exceeded(), PrintWriter::Stdout).unwrap_err();
    assert_eq!(err.error.exc_type(), ExcType::RuntimeError);
    let mut repl = err.repl;
    assert_eq!(
        repl.feed_run("x + 1", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(42)
    );
}
