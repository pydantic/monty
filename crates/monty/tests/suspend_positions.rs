//! Tests for the source position every suspension carries: a `FunctionCall`
//! or `OsCall` points at its call expression, a `NameLookup` at the name, and
//! `ResolveFutures` at the `await` the main task is blocked on.

use monty::{Dump, MontyRepl, MontyRun, ReplProgress, RunProgress, Session, SessionRef, dump};
use monty_types::{CodeLoc, CompileOptions, MontyObject, NameLookupResult, PrintWriter, ResourceTracker, SourceRange};

/// Builds the expected position of a single-line expression in `filename`.
fn range(filename: &str, line: u32, column: u32, end_column: u32) -> SourceRange {
    SourceRange {
        filename: filename.to_owned(),
        start: CodeLoc { line, column },
        end: CodeLoc {
            line,
            column: end_column,
        },
    }
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
    let call = start("x = 1\ny = fetch(x, 2) + 1").into_function_call().unwrap();
    assert_eq!(call.function_name, "fetch");
    assert_eq!(call.position, range("test.py", 2, 5, 16));
}

#[test]
fn a_call_inside_a_function_points_into_its_body() {
    let call = start("def helper(n):\n    return fetch(n)\n\nhelper(3)")
        .into_function_call()
        .unwrap();
    assert_eq!(call.position, range("test.py", 2, 12, 20));
}

#[test]
fn name_lookup_points_at_the_name() {
    let runner = MontyRun::new(
        "total = 1 + missing".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let lookup = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
        .into_name_lookup()
        .unwrap();
    assert_eq!(lookup.name, "missing");
    assert_eq!(lookup.position, range("test.py", 1, 13, 20));
}

#[test]
fn os_call_points_at_the_call_expression() {
    let call = start("import os\nhome = os.getenv('HOME')").into_os_call().unwrap();
    assert_eq!(call.function_call.name(), "os.getenv");
    assert_eq!(call.position, range("test.py", 2, 8, 25));
}

#[test]
fn a_call_inside_eval_points_into_the_string() {
    let call = start("eval('1 + fetch()')").into_function_call().unwrap();
    assert_eq!(call.position, range("<string>", 1, 5, 12));
}

#[test]
fn a_call_from_an_earlier_feed_points_into_that_snippet() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("def helper(n):\n    return fetch(n)", vec![], PrintWriter::Stdout)
        .unwrap();
    let progress = repl.feed_start("helper(3)", vec![], PrintWriter::Stdout).unwrap();
    let ReplProgress::FunctionCall(call) = progress else {
        panic!("expected a function call");
    };
    assert_eq!(call.position, range("<python-input-0>", 2, 12, 20));

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
    assert_eq!(call.position, range("<python-input-0>", 2, 12, 20));
}

#[test]
fn a_one_shot_dump_carries_the_position() {
    let progress = start("value = fetch()");
    let bytes = dump("test.py", None, SessionRef::Running(&progress)).unwrap();
    let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a running session, loaded something else");
    };
    assert_eq!(
        loaded.into_function_call().unwrap().position,
        range("test.py", 1, 9, 16)
    );
}

#[test]
fn resolve_futures_points_at_the_main_task_await() {
    // The main task awaits the pending future itself.
    let call = start("x = await fetch()").into_function_call().unwrap();
    assert_eq!(call.position, range("test.py", 1, 11, 18));
    let waiting = call
        .resume_pending(PrintWriter::Stdout)
        .unwrap()
        .into_resolve_futures()
        .unwrap();
    assert_eq!(*waiting.position(), range("test.py", 1, 5, 18));
}

#[test]
fn resolve_futures_points_at_the_main_task_await_while_spawned_tasks_block() {
    // Both calls happen in spawned tasks; once the second blocks nothing can
    // run, so the VM parks and the main task's `gather` is the reported wait.
    let code = "import asyncio\n\nasync def task():\n    return await fetch()\n\nawait asyncio.gather(task(), task())";
    let mut progress = start(code);
    let mut pending = 0;
    loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                assert_eq!(call.position, range("test.py", 4, 18, 25));
                pending += 1;
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(waiting) => {
                assert_eq!(pending, 2);
                assert_eq!(*waiting.position(), range("test.py", 6, 1, 37));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
}
