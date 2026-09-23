//! Tests for the source position every suspension carries: a `FunctionCall`
//! or `OsCall` points at its call expression, a `NameLookup` at the name, and
//! `ResolveFutures` at the `await` the main task is blocked on.

use monty::{Dump, MontyRepl, MontyRun, ReplProgress, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CodeLoc, CompileOptions, ExtFunctionResult, MontyObject, NameLookupResult, PrintWriter, ResourceTracker,
    SourceRange,
};

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
    // columns count characters, not UTF-8 bytes
    let call = start("y = 'éé' + fetch()").into_function_call().unwrap();
    assert_eq!(call.position, range("test.py", 1, 12, 19));
}

#[test]
fn every_line_ending_starts_a_line() {
    // a bare `\r` ends a line as the parser sees it; `\r\n` ends just one
    let call = start("x = 1\rfetch()").into_function_call().unwrap();
    assert_eq!(call.position, range("test.py", 2, 1, 8));
    let call = start("x = 1\r\ny = 2\r\nfetch()").into_function_call().unwrap();
    assert_eq!(call.position, range("test.py", 3, 1, 8));

    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl.feed_start("x = 1\rfetch()", vec![], PrintWriter::Stdout).unwrap();
    let ReplProgress::FunctionCall(call) = progress else {
        panic!("expected a function call");
    };
    assert_eq!(call.position, range("<python-input-0>", 2, 1, 8));
}

#[test]
fn a_long_non_ascii_line_resolves_columns_past_the_checkpoints() {
    // columns past several 64-byte character checkpoints, on a line of multi-byte chars
    let padding = "é".repeat(200);
    let call = start(&format!("y = '{padding}' + fetch()"))
        .into_function_call()
        .unwrap();
    assert_eq!(call.position, range("test.py", 1, 210, 217));
    // 45,000 characters in: a scan from the line start per range would make compiling this quadratic
    let call = start(&format!("{}fetch()", "x = 'é'; ".repeat(5000)))
        .into_function_call()
        .unwrap();
    assert_eq!(call.position, range("test.py", 1, 45_001, 45_008));
}

#[test]
fn a_non_ascii_line_starting_mid_checkpoint_counts_from_its_start() {
    // lines 1 and 2 hold 16 bytes but 13 chars, so line 3 starts inside the first 64-byte chunk
    let head = "# é\nx = 'éé'\n";
    // the call ends at byte 64, exactly on a checkpoint and at the end of the source
    let code = format!("{head}y = '{}' + fetch()", "é".repeat(16));
    assert_eq!(code.len(), 64);
    let call = start(&code).into_function_call().unwrap();
    assert_eq!(call.position, range("test.py", 3, 26, 33));
    // the line starts in the first chunk and the call lies in the second
    let call = start(&format!("{head}y = '{}' + fetch()", "é".repeat(40)))
        .into_function_call()
        .unwrap();
    assert_eq!(call.position, range("test.py", 3, 50, 57));
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
    // answer the call so the run winds down instead of dropping live values
    let done = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    assert!(done.into_complete().is_some());
}

#[test]
fn a_call_inside_eval_points_into_the_string() {
    let call = start("eval('1 + fetch()')").into_function_call().unwrap();
    assert_eq!(call.position, range("<string>", 1, 5, 12));
    // `eval` strips leading spaces and tabs, not newlines; positions index the stripped text
    let call = start("eval('  \\t1 + fetch()')").into_function_call().unwrap();
    assert_eq!(call.position, range("<string>", 1, 5, 12));
    let call = start("eval(' \\n1 + fetch()')").into_function_call().unwrap();
    assert_eq!(call.position, range("<string>", 2, 5, 12));
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
    let mut pending = Vec::new();
    loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                assert_eq!(call.position, range("test.py", 4, 18, 25));
                pending.push(call.call_id);
                assert!(pending.len() <= 2, "only two calls precede the wait");
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(waiting) => {
                assert_eq!(pending.len(), 2);
                assert_eq!(*waiting.position(), range("test.py", 6, 1, 37));
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
