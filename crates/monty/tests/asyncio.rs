//! Tests for async edge cases around ResolveFutures::resume behavior.
//!
//! These tests verify the behavior of the async execution model, specifically around
//! resolving external futures incrementally via `ResolveFutures::resume()`.

use monty::{
    ExcType, ExtFunctionResult, MontyException, MontyObject, MontyRun, NameLookupResult, NoLimitTracker, PrintWriter,
    ResolveFutures, RunProgress,
};

/// Helper to create a MontyRun for async external function tests.
///
/// Sets up an async function that calls two async external functions (`foo` and `bar`)
/// via asyncio.gather and returns their sum.
fn create_gather_two_runner() -> MontyRun {
    let code = r"
import asyncio

async def main():
    a, b = await asyncio.gather(foo(), bar())
    return a + b

await main()
";
    MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap()
}

/// Helper to create a MontyRun for async external function tests with three functions.
fn create_gather_three_runner() -> MontyRun {
    let code = r"
import asyncio

async def main():
    a, b, c = await asyncio.gather(foo(), bar(), baz())
    return a + b + c

await main()
";
    MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap()
}

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
fn resolve_name_lookups<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> Result<RunProgress<T>, monty::MontyException> {
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup.resume(
            NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
            PrintWriter::Stdout,
        )?;
    }
    Ok(progress)
}

/// Helper to drive execution through external calls until we get ResolveFutures.
///
/// Returns (pending_call_ids, state, collected_call_ids) where collected_call_ids
/// are the call_ids from all the FunctionCalls we processed with resume_pending().
fn drive_to_resolve_futures<T: monty::ResourceTracker>(mut progress: RunProgress<T>) -> (ResolveFutures<T>, Vec<u32>) {
    let mut collected_call_ids = Vec::new();

    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                progress = lookup
                    .resume(
                        NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
                        PrintWriter::Stdout,
                    )
                    .unwrap();
            }
            RunProgress::FunctionCall(call) => {
                collected_call_ids.push(call.call_id);
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(state) => {
                return (state, collected_call_ids);
            }
            RunProgress::Complete(_) => {
                panic!("unexpected Complete before ResolveFutures");
            }
            RunProgress::OsCall(call) => {
                panic!("unexpected OsCall: {:?}", call.function);
            }
        }
    }
}

// === Test: Resume with all call_ids at once ===

#[test]
fn resume_with_all_call_ids() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    assert_eq!(call_ids.len(), 2, "should have 2 pending calls");

    // Resume with all results at once
    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10))),
        (call_ids[1], ExtFunctionResult::Return(MontyObject::Int(32))),
    ];

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with partial results ===

#[test]
fn resume_with_partial_results() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Resume with only the first result
    let results = vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Should still need more futures resolved
    let state = progress.into_resolve_futures().expect("should still need futures");

    // Resume with the second result
    let results = vec![(call_ids[1], ExtFunctionResult::Return(MontyObject::Int(32)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with unknown call_id ===

#[test]
fn resume_with_unknown_call_id() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, _call_ids) = drive_to_resolve_futures(progress);

    // Resume with an unknown call_id
    let results = vec![(9999, ExtFunctionResult::Return(MontyObject::Int(10)))];
    let result = state.resume(results, PrintWriter::Stdout);

    assert!(result.is_err(), "should error on unknown call_id");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    let msg = exc.message().unwrap();
    assert!(
        msg.contains("unknown call_id 9999"),
        "error should mention unknown call_id, got: {msg}"
    );
}

// === Test: Resume with empty results ===

#[test]
fn resume_with_empty_results() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Resume with empty results - should still be blocked
    let results: Vec<(u32, ExtFunctionResult)> = vec![];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Should still need futures resolved
    let state = progress.into_resolve_futures().expect("should still need futures");

    // Now resolve everything
    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10))),
        (call_ids[1], ExtFunctionResult::Return(MontyObject::Int(32))),
    ];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with error result ===

#[test]
fn resume_with_error_result() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Resume with one success and one error
    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10))),
        (
            call_ids[1],
            ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("test error".to_string()))),
        ),
    ];

    let result = state.resume(results, PrintWriter::Stdout);

    // Should propagate the error
    assert!(result.is_err(), "should propagate error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("test error"));
}

// === Test: Resume with three functions, reversed order ===

#[test]
fn resume_with_reversed_order() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Resume with results in reverse order - should still work
    let results = vec![
        (call_ids[1], ExtFunctionResult::Return(MontyObject::Int(32))), // bar() = 32
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10))), // foo() = 10
    ];

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Three-way gather with incremental resolution ===

#[test]
fn three_way_gather_incremental() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    assert_eq!(call_ids.len(), 3, "should have 3 pending calls");

    // Resolve one at a time
    let results = vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let state = progress.into_resolve_futures().expect("need more");

    let results = vec![(call_ids[1], ExtFunctionResult::Return(MontyObject::Int(200)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let state = progress.into_resolve_futures().expect("need more");

    let results = vec![(call_ids[2], ExtFunctionResult::Return(MontyObject::Int(300)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(600));
}

// === Test: Duplicate call_id in results (should be fine - second is ignored) ===

#[test]
fn resume_with_duplicate_call_id() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Include duplicate - second value should be ignored
    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(10))),
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(99))), // duplicate - ignored!
        (call_ids[1], ExtFunctionResult::Return(MontyObject::Int(32))),
    ];

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: gather_error_propagated_as_exception ===

#[test]
fn gather_error_propagated_as_exception() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Both fail with errors
    let results = vec![
        (
            call_ids[0],
            ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("foo error".to_string()))),
        ),
        (
            call_ids[1],
            ExtFunctionResult::Error(MontyException::new(
                ExcType::RuntimeError,
                Some("bar error".to_string()),
            )),
        ),
    ];

    let result = state.resume(results, PrintWriter::Stdout);

    // One of the errors should propagate (implementation may choose either)
    assert!(result.is_err(), "should propagate an error");
}

// === Test: Sequential awaits - second fails ===

fn create_sequential_awaits_runner() -> MontyRun {
    let code = r"
import asyncio

async def main():
    a = await foo()
    b = await bar()
    return a + b

await main()
";
    MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap()
}

#[test]
fn sequential_awaits_second_fails() {
    let runner = create_sequential_awaits_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();

    // First external call (foo)
    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected FunctionCall for foo");
    };
    let foo_call_id = call.call_id;
    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();

    // Should yield for resolution
    let state = progress.into_resolve_futures().expect("should need foo resolved");
    assert_eq!(state.pending_call_ids(), vec![foo_call_id]);

    // Resolve foo successfully
    let results = vec![(foo_call_id, ExtFunctionResult::Return(MontyObject::Int(10)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();

    // Second external call (bar)
    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected FunctionCall for bar");
    };
    let bar_call_id = call.call_id;
    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();

    // Should yield for resolution
    let state = progress.into_resolve_futures().expect("should need bar resolved");
    assert_eq!(state.pending_call_ids(), vec![bar_call_id]);

    // Fail bar with an exception
    let results = vec![(
        bar_call_id,
        ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("bar failed".to_string()))),
    )];

    let result = state.resume(results, PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate bar's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("bar failed"));
}

// === Test: Sequential awaits - first fails ===

#[test]
fn sequential_awaits_first_fails() {
    let runner = create_sequential_awaits_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();

    // First external call (foo)
    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected FunctionCall for foo");
    };
    let foo_call_id = call.call_id;
    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need foo resolved");

    // Fail foo with an exception - bar should never be called
    let results = vec![(
        foo_call_id,
        ExtFunctionResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("foo failed early".to_string()),
        )),
    )];

    let result = state.resume(results, PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate foo's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("foo failed early"));
}

// === Test: Gather - first external fails before second is resolved ===

#[test]
fn gather_first_external_fails_immediately() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    assert_eq!(call_ids.len(), 2, "should have 2 calls");

    // Resolve first call with error, second with success
    let results = vec![(
        call_ids[0],
        ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("foo failed".to_string()))),
    )];

    let result = state.resume(results, PrintWriter::Stdout);

    // Error should propagate immediately (no need to resolve second)
    assert!(result.is_err(), "should propagate foo's error immediately");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("foo failed"));
}

// === Test: Gather - second external fails ===

#[test]
fn gather_second_external_fails() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Resolve second call with error
    let results = vec![(
        call_ids[1],
        ExtFunctionResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("bar failed".to_string()),
        )),
    )];

    let result = state.resume(results, PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate bar's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("bar failed"));
}

// === Test: Both gather tasks fail ===

#[test]
fn gather_both_fail() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    let results = vec![
        (
            call_ids[0],
            ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("foo failed".to_string()))),
        ),
        (
            call_ids[1],
            ExtFunctionResult::Error(MontyException::new(
                ExcType::RuntimeError,
                Some("bar failed".to_string()),
            )),
        ),
    ];

    let result = state.resume(results, PrintWriter::Stdout);
    assert!(result.is_err(), "should propagate one of the errors");
}

// === Test: Three-way gather, partial error ===

#[test]
fn three_way_gather_partial_error() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // First and third succeed, second fails
    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(100))),
        (
            call_ids[1],
            ExtFunctionResult::Error(MontyException::new(
                ExcType::TypeError,
                Some("bar type error".to_string()),
            )),
        ),
        (call_ids[2], ExtFunctionResult::Return(MontyObject::Int(300))),
    ];

    let result = state.resume(results, PrintWriter::Stdout);
    assert!(result.is_err(), "should propagate bar's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TypeError);
}

// === Test: Incremental resolution with error on second round ===

#[test]
fn incremental_resolution_error_on_second_round() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // First resolve one successfully
    let results = vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let state = progress.into_resolve_futures().expect("need more");

    // Then fail the second
    let results = vec![(
        call_ids[1],
        ExtFunctionResult::Error(MontyException::new(
            ExcType::ValueError,
            Some("delayed failure".to_string()),
        )),
    )];

    let result = state.resume(results, PrintWriter::Stdout);
    assert!(result.is_err(), "should propagate delayed error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("delayed failure"));
}

// === Test: Gather with all at once, mixed success/failure ===

#[test]
fn gather_three_all_at_once_mixed() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    let results = vec![
        (call_ids[0], ExtFunctionResult::Return(MontyObject::Int(100))),
        (call_ids[1], ExtFunctionResult::Return(MontyObject::Int(200))),
    ];

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let state = progress.into_resolve_futures().expect("need more");

    let results = vec![(
        call_ids[2],
        ExtFunctionResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("baz failed".to_string()),
        )),
    )];

    let result = state.resume(results, PrintWriter::Stdout);
    assert!(result.is_err(), "should propagate baz error");
}

// === Tests: Nested gather with task switching ===
//
// These tests target a pair of bugs in task switching during incremental resolution:
// - Correct value pushing when restoring from a resolved task (Bug 1)
// - Correct waiter context detection for current task (Bug 2)

/// Helper to drive execution, collecting function calls and resolving them async,
/// until we reach ResolveFutures. Returns the snapshot and a vec of
/// (call_id, function_name) pairs for all external calls made.
fn drive_collecting_calls<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> (ResolveFutures<T>, Vec<(u32, String)>) {
    let mut collected = Vec::new();

    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                progress = lookup
                    .resume(
                        NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
                        PrintWriter::Stdout,
                    )
                    .unwrap();
            }
            RunProgress::FunctionCall(call) => {
                collected.push((call.call_id, call.function_name.clone()));
                progress = call.resume_pending(PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(state) => {
                return (state, collected);
            }
            RunProgress::Complete(_) => {
                panic!("unexpected Complete before ResolveFutures");
            }
            RunProgress::OsCall(call) => {
                panic!("unexpected OsCall: {:?}", call.function);
            }
        }
    }
}

/// Tests nested gathers where spawned tasks do sequential external await then inner gather.
///
/// Pattern:
/// - Outer gather spawns 3 coroutine tasks
/// - Each coroutine does `await get_lat_lng(city)` then `await asyncio.gather(get_temp(city), get_desc(city))`
/// - All external functions are resolved via async futures
///
/// This exercises both Bug 1 (resolved value not pushed to restored task stack) and
/// Bug 2 (current task's gather result pushed to wrong location).
#[test]
fn nested_gather_with_spawned_tasks_and_external_futures() {
    let code = r"
import asyncio

async def process(city):
    coords = await get_lat_lng(city)
    temp, desc = await asyncio.gather(get_temp(city), get_desc(city))
    return coords + temp + desc

async def main():
    results = await asyncio.gather(
        process('a'),
        process('b'),
        process('c'),
    )
    return results[0] + results[1] + results[2]

await main()
";

    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    // Drive until all initial external calls are made and we need to resolve futures
    let (state, calls) = drive_collecting_calls(progress);

    // The 3 spawned tasks each call get_lat_lng first, so we expect 3 get_lat_lng calls
    assert_eq!(calls.len(), 3, "should have 3 initial get_lat_lng calls");
    for (_, name) in &calls {
        assert_eq!(name, "get_lat_lng", "initial calls should all be get_lat_lng");
    }

    // Resolve all 3 get_lat_lng calls: each returns 100
    let results: Vec<(u32, ExtFunctionResult)> = calls
        .iter()
        .map(|(id, _)| (*id, ExtFunctionResult::Return(MontyObject::Int(100))))
        .collect();

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // After resolving get_lat_lng, each task proceeds to the inner gather which
    // calls get_temp and get_desc. Drive those calls.
    let (state, calls) = drive_collecting_calls(progress);

    // Each of 3 tasks calls get_temp + get_desc = 6 calls total
    assert_eq!(calls.len(), 6, "should have 6 inner gather calls (3 tasks * 2 each)");
    let temp_calls: Vec<_> = calls.iter().filter(|(_, n)| n == "get_temp").collect();
    let desc_calls: Vec<_> = calls.iter().filter(|(_, n)| n == "get_desc").collect();
    assert_eq!(temp_calls.len(), 3, "should have 3 get_temp calls");
    assert_eq!(desc_calls.len(), 3, "should have 3 get_desc calls");

    // Resolve all inner calls: get_temp returns 10, get_desc returns 1
    let results: Vec<(u32, ExtFunctionResult)> = calls
        .iter()
        .map(|(id, name)| {
            let val = if name == "get_temp" { 10 } else { 1 };
            (*id, ExtFunctionResult::Return(MontyObject::Int(val)))
        })
        .collect();

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Each task returns coords(100) + temp(10) + desc(1) = 111
    // main returns 111 + 111 + 111 = 333
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(333));
}

/// Tests nested gathers with incremental resolution (one task at a time).
///
/// Same pattern as above but resolves futures in multiple rounds to ensure
/// task switching between partially-resolved states works correctly.
#[test]
fn nested_gather_incremental_resolution() {
    let code = r"
import asyncio

async def process(x):
    a = await step1(x)
    b, c = await asyncio.gather(step2(x), step3(x))
    return a + b + c

async def main():
    r1, r2 = await asyncio.gather(process('x'), process('y'))
    return r1 + r2

await main()
";

    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    // Drive to get the initial step1 calls
    let (state, calls) = drive_collecting_calls(progress);
    assert_eq!(calls.len(), 2, "should have 2 step1 calls");

    // Resolve only the FIRST step1 call
    let results = vec![(calls[0].0, ExtFunctionResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // First task proceeds to inner gather (step2 + step3), second task still blocked
    let (state, new_calls) = drive_collecting_calls(progress);

    // We should see step2 and step3 for the first task
    assert_eq!(new_calls.len(), 2, "should have 2 inner calls from first task");

    // Now resolve the second step1 call AND the first task's inner calls
    let mut results: Vec<(u32, ExtFunctionResult)> = vec![
        // Second task's step1
        (calls[1].0, ExtFunctionResult::Return(MontyObject::Int(200))),
    ];
    // First task's inner calls
    for (id, name) in &new_calls {
        let val = if name == "step2" { 10 } else { 1 };
        results.push((*id, ExtFunctionResult::Return(MontyObject::Int(val))));
    }

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Second task now proceeds to inner gather
    let (state, final_calls) = drive_collecting_calls(progress);
    assert_eq!(final_calls.len(), 2, "should have 2 inner calls from second task");

    // Resolve second task's inner calls
    let results: Vec<(u32, ExtFunctionResult)> = final_calls
        .iter()
        .map(|(id, name)| {
            let val = if name == "step2" { 20 } else { 2 };
            (*id, ExtFunctionResult::Return(MontyObject::Int(val)))
        })
        .collect();

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // First task: 100 + 10 + 1 = 111
    // Second task: 200 + 20 + 2 = 222
    // Total: 111 + 222 = 333
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(333));
}
