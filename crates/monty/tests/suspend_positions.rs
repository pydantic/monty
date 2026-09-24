//! Tests for the source position every suspension carries: a `FunctionCall`
//! or `OsCall` points at its call expression, a `NameLookup` at the name, and
//! `ResolveFutures` at the `await` the main task is blocked on.

use monty::{Dump, MontyRepl, MontyRun, ReplProgress, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExtFunctionResult, MontyObject, NameLookupResult, PrintWriter, ResourceTracker, SourceRange,
};

/// The text `position` covers in `source`, which it names `filename`; offsets are UTF-8 bytes.
#[track_caller]
fn covered<'s>(position: &SourceRange, filename: &str, source: &'s str) -> &'s str {
    assert_eq!(position.filename, filename);
    &source[position.start as usize..position.end as usize]
}

/// Starts `code` as a one-shot run and resolves every name lookup to a
/// function, so calls of undefined names suspend as `FunctionCall`s.
fn start(code: &str) -> RunProgress {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup
            .resume(
                NameLookupResult::Value(MontyObject::function(name, None)),
                PrintWriter::Stdout,
            )
            .unwrap();
    }
    progress
}

#[test]
fn function_call_points_at_the_call_expression() {
    let code = "x = 1\ny = fetch(x, 2) + 1";
    let call = start(code).into_function_call().unwrap();
    assert_eq!(call.function_name, "fetch");
    assert_eq!(covered(&call.position, "test.py", code), "fetch(x, 2)");
    // offsets count UTF-8 bytes: each `é` is two
    let code = "y = 'éé' + fetch()";
    let call = start(code).into_function_call().unwrap();
    assert_eq!((call.position.start, call.position.end), (13, 20));
    assert_eq!(covered(&call.position, "test.py", code), "fetch()");
}

#[test]
fn a_call_inside_a_function_points_into_its_body() {
    let code = "def helper(n):\n    return fetch(n)\n\nhelper(3)";
    let call = start(code).into_function_call().unwrap();
    assert_eq!(covered(&call.position, "test.py", code), "fetch(n)");
}

#[test]
fn name_lookup_points_at_the_name() {
    let code = "total = 1 + missing";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let lookup = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
        .into_name_lookup()
        .unwrap();
    assert_eq!(lookup.name, "missing");
    assert_eq!(covered(&lookup.position, "test.py", code), "missing");
}

#[test]
fn os_call_points_at_the_call_expression() {
    let code = "import os\nhome = os.getenv('HOME')";
    let call = start(code).into_os_call().unwrap();
    assert_eq!(call.function_call.name(), "os.getenv");
    assert_eq!(covered(&call.position, "test.py", code), "os.getenv('HOME')");
    // answer the call so the run winds down instead of dropping live values
    let done = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    assert!(done.into_complete().is_some());
}

#[test]
fn an_os_call_after_a_nested_dunder_call_points_at_the_call() {
    // `sleep` runs `__index__` in a nested frame before suspending; returning
    // from it must not move the position on to the next instruction, `other`
    let code = "import time\nclass Index:\n    def __index__(self):\n        return 0\nother = 1\nprint(time.sleep(Index()), other)";
    let call = start(code).into_os_call().unwrap();
    assert_eq!(call.function_call.name(), "system.sleep");
    assert_eq!(covered(&call.position, "test.py", code), "time.sleep(Index())");
    let done = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    assert!(done.into_complete().is_some());
}

#[test]
fn a_call_inside_eval_points_into_the_string() {
    let call = start("eval('1 + fetch()')").into_function_call().unwrap();
    assert_eq!(covered(&call.position, "<string>", "1 + fetch()"), "fetch()");
    // offsets index the eval text after its leading whitespace
    let call = start("eval('  \\t1 + fetch()')").into_function_call().unwrap();
    assert_eq!(covered(&call.position, "<string>", "1 + fetch()"), "fetch()");
}

#[test]
fn a_call_from_an_earlier_feed_points_into_that_snippet() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let first_feed = "def helper(n):\n    return fetch(n)";
    repl.feed_run(first_feed, vec![], PrintWriter::Stdout).unwrap();
    let progress = repl.feed_start("helper(3)", vec![], PrintWriter::Stdout).unwrap();
    let ReplProgress::FunctionCall(call) = progress else {
        panic!("expected a function call");
    };
    assert_eq!(covered(&call.position, "<python-input-0>", first_feed), "fetch(n)");

    // The position is part of the suspended state, so a dump carries it.
    let bytes = dump(
        "repl.py",
        None,
        SessionRef::Suspended(&ReplProgress::FunctionCall(call)),
    )
    .unwrap();
    let Session::Suspended(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a suspended session, loaded something else");
    };
    let ReplProgress::FunctionCall(call) = *loaded else {
        panic!("expected a function call");
    };
    assert_eq!(covered(&call.position, "<python-input-0>", first_feed), "fetch(n)");
}

#[test]
fn a_one_shot_dump_carries_the_position() {
    let code = "value = fetch()";
    let progress = start(code);
    let bytes = dump("test.py", None, SessionRef::Running(&progress)).unwrap();
    let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a running session, loaded something else");
    };
    let position = loaded.into_function_call().unwrap().position;
    assert_eq!(covered(&position, "test.py", code), "fetch()");
}

#[test]
fn resolve_futures_points_at_the_main_task_await() {
    // The main task awaits the pending future itself.
    let code = "x = await fetch()";
    let call = start(code).into_function_call().unwrap();
    assert_eq!(covered(&call.position, "test.py", code), "fetch()");
    let waiting = call
        .resume_pending(PrintWriter::Stdout)
        .unwrap()
        .into_resolve_futures()
        .unwrap();
    assert_eq!(covered(waiting.position(), "test.py", code), "await fetch()");
}

#[test]
fn resolve_futures_points_at_the_main_task_await_while_spawned_tasks_block() {
    // Both calls happen in spawned tasks; once the second blocks nothing can
    // run, so the VM parks and the main task's `gather` is the reported wait.
    let code = "import asyncio\n\nasync def task():\n    return await fetch()\n\nawait asyncio.gather(task(), task())";
    let mut progress = start(code);
    let mut pending = Vec::new();
    loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                assert_eq!(covered(&call.position, "test.py", code), "fetch()");
                pending.push(call.call_id);
                assert!(pending.len() <= 2, "only two calls precede the wait");
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(waiting) => {
                assert_eq!(pending.len(), 2);
                assert_eq!(
                    covered(waiting.position(), "test.py", code),
                    "await asyncio.gather(task(), task())"
                );
                // settle both futures so the run winds down instead of dropping live values
                let results = pending
                    .iter()
                    .map(|&id| (id, ExtFunctionResult::Return(MontyObject::int(1))))
                    .collect();
                let done = waiting.resume(results, PrintWriter::Stdout).unwrap();
                assert!(done.into_complete().is_some());
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
}
