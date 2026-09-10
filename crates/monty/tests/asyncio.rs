//! Tests for async edge cases around ResolveFutures::resume behavior.
//!
//! These tests verify the behavior of the async execution model, specifically around
//! resolving external futures incrementally via `ResolveFutures::resume()`.

use std::thread;

use monty::{MontyRun, ResolveFutures, RunProgress};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, NameLookupResult, PrintWriter,
    ResourceTracker,
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
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap()
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
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap()
}

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
fn resolve_name_lookups(mut progress: RunProgress) -> Result<RunProgress, MontyException> {
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
fn drive_to_resolve_futures(mut progress: RunProgress) -> (ResolveFutures, Vec<u32>) {
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
                panic!("unexpected OsCall: {:?}", call.function_call.name());
            }
        }
    }
}

// === Test: Suspended task stack stays alive across GC ===

#[test]
#[cfg(feature = "test-hooks")]
fn suspended_task_stack_survives_forced_gc() {
    let code = r"
import asyncio

async def parked():
    x = [1, 2, 3]
    _ = await async_call(7)
    assert x == [1, 2, 3]
    return len(x)

async def ready():
    return 10

await asyncio.gather(parked(), ready())
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    assert_eq!(
        call_ids.len(),
        1,
        "expected the parked task to yield one external future"
    );

    let state = state.__force_gc_for_tests();
    let progress = state
        .resume(
            vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(99)))],
            PrintWriter::Stdout,
        )
        .unwrap();

    let result = progress
        .into_complete()
        .expect("should complete after resuming parked task");
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(3), MontyObject::Int(10)]),
    );
}

// === Test: Resume with all call_ids at once ===

#[test]
fn resume_with_all_call_ids() {
    let runner = create_gather_two_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap()
}

#[test]
fn sequential_awaits_second_fails() {
    let runner = create_sequential_awaits_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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

// === Test: Gather - a coroutine child whose external call fails is dropped ===

/// A gather child that is a coroutine gets its own task, and its failing
/// external call is settled against the gather rather than raised inside the
/// child. Nothing would then deliver the failure to that child, so it must be
/// dropped here — otherwise it stays `Blocked` on a future that has just been
/// failed and unregistered, holding its coroutine and the gather for the rest
/// of the session. Its siblings are unaffected and keep running.
#[test]
#[cfg(feature = "test-hooks")]
fn gather_coroutine_child_dropped_when_its_external_fails() {
    let code = r"
import asyncio

async def child():
    return await foo()

async def sibling():
    return await bar()

async def main():
    try:
        await asyncio.gather(child(), sibling())
    except ValueError as exc:
        assert str(exc) == 'foo failed'
    return await baz()

await main()
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    assert_eq!(call_ids.len(), 2, "child and sibling each yield one external call");
    // main, child, sibling.
    assert_eq!(state.__live_task_count_for_tests(), 3);

    // Fail the child's call, leaving the sibling's outstanding.
    let progress = state
        .resume(
            vec![(
                call_ids[0],
                ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("foo failed".to_string()))),
            )],
            PrintWriter::Stdout,
        )
        .unwrap();

    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected main to reach `baz` after catching the error");
    };
    let baz_id = call.call_id;
    let RunProgress::ResolveFutures(state) = call.resume_pending(PrintWriter::Stdout).unwrap() else {
        panic!("expected to suspend on `baz`");
    };

    // The child is gone; the sibling is still parked on `bar`, as CPython
    // leaves it on the loop.
    assert_eq!(state.__live_task_count_for_tests(), 2);

    let progress = state
        .resume(
            vec![(baz_id, ExtFunctionResult::Return(MontyObject::Int(11)))],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(progress.into_complete().expect("should complete"), MontyObject::Int(11));
}

// === Test: Gather - second external fails ===

#[test]
fn gather_second_external_fails() {
    let runner = create_gather_two_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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

// === Test: Partial resolution with mixed coroutine task + direct external call ===
// This reproduces a panic ("no active frame") when a gather mixes a coroutine
// task (which itself awaits an external call) with a direct external call,
// and only the task's external call is resolved first.

#[test]
fn gather_mixed_coroutine_and_direct_external_partial_resolve() {
    let code = r"
import asyncio

async def double(x):
    val = await async_call(x)
    return val * 2

results = await asyncio.gather(double(5), async_call(100))
results
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    // Use drive_collecting_calls so we know which call_id maps to which invocation.
    // Call order: async_call(100) (gather direct) then async_call(5) (double's inner).
    let (state, calls) = drive_collecting_calls(progress);
    assert_eq!(calls.len(), 2, "should have 2 external calls");
    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Resolve the gather's direct external call first: async_call(100) → returns 100.
    // This is a partial resolution — double(5) is still blocked on its own async_call(5).
    let results = vec![(calls[0].0, ExtFunctionResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Should return ResolveFutures with the remaining call (async_call(5) for double)
    let state = progress
        .into_resolve_futures()
        .expect("should need more futures (double's async_call(5) still pending)");

    assert_eq!(
        state.pending_call_ids().len(),
        1,
        "should have 1 remaining pending call"
    );

    // Resolve double's inner call: async_call(5) → returns 5.
    // double(5) will then compute 5 * 2 = 10.
    let results = vec![(calls[1].0, ExtFunctionResult::Return(MontyObject::Int(5)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // gather(double(5), async_call(100)) = [10, 100]
    let result = progress.into_complete().expect("should complete");
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(10), MontyObject::Int(100)])
    );
}

// === Test: Gather with all at once, mixed success/failure ===

#[test]
fn gather_three_all_at_once_mixed() {
    let runner = create_gather_three_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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
fn drive_collecting_calls(mut progress: RunProgress) -> (ResolveFutures, Vec<(u32, String)>) {
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
                panic!("unexpected OsCall: {:?}", call.function_call.name());
            }
        }
    }
}

// === Test: regression for https://github.com/pydantic/monty/issues/240 ===

#[test]
fn gather_three_tasks_with_direct_external() {
    let code = r"
import asyncio

async def slow_a():
    val = await async_call(1)
    return val

async def slow_b():
    val = await async_call(2)
    return val

results = await asyncio.gather(slow_a(), slow_b(), async_call(999))
results
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);
    // 3 calls: async_call(999) from gather, async_call(1) from slow_a, async_call(2) from slow_b
    assert_eq!(call_ids.len(), 3, "should have 3 external calls");
    assert_eq!(state.pending_call_ids().len(), 3, "should have 3 pending calls");

    // A previous implementation of Monty had an issue where resolving the direct external call
    // first would corrupt the pending calls state in the gather.
    let results = vec![(call_ids[0], ExtFunctionResult::Return(MontyObject::Int(999)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    let state = progress
        .into_resolve_futures()
        .expect("should need more futures (slow_a and slow_b still pending)");

    let remaining = state.pending_call_ids();
    assert_eq!(remaining.len(), 2, "should have 2 remaining calls");

    // Resolve one of the remaining calls
    let results = vec![(remaining[0], ExtFunctionResult::Return(MontyObject::Int(42)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // After the first coroutine completes, we should still need the second coroutine's result
    let state = progress
        .into_resolve_futures()
        .expect("should need more futures (one coroutine still pending)");

    assert_eq!(state.pending_call_ids().len(), 1, "should have 1 remaining call");

    // Resolve the last call
    let last_id = state.pending_call_ids()[0];
    let results = vec![(last_id, ExtFunctionResult::Return(MontyObject::Int(42)))];
    let progress = state.resume(results, PrintWriter::Stdout).unwrap();

    // Should complete with all three results: [slow_a=42, slow_b=42, direct=999]
    let result = progress.into_complete().expect("should complete");
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(42), MontyObject::Int(42), MontyObject::Int(999),])
    );
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

    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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

    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

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

// === Test: Gathers nested directly inside one another commit without recursing ===

/// Nesting a gather as an *item* of another costs no Python frames
/// (`g = asyncio.gather(g)` in a loop), so the recursion limit never sees the
/// commit walk that descends through it. Committing 5,000 levels must work.
///
/// Runs on a 2 MiB thread — a worker's budget, and where the abort was seen;
/// libtest's 8 MiB would only move the depth at which it aborts.
#[test]
fn deeply_nested_gather_commit_does_not_overflow_the_stack() {
    run_on_a_worker_stack(await_deeply_nested_gathers);
}

/// Wraps `leaf()` in 5,000 gathers, awaits the outermost, and unwraps the
/// 5,000 single-item result lists back down to the leaf's `1`.
///
/// The leaf coroutine parks the whole chain (it is spawned as a task), so this
/// covers both directions: the commit walk down, and the resolution walk back
/// up through 5,000 `GatherSlot` links.
fn await_deeply_nested_gathers() {
    let code = r"
import asyncio

async def leaf():
    return 1

g = leaf()
for _ in range(5000):
    g = asyncio.gather(g)
result = await g
for _ in range(5000):
    result = result[0]
result
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let result = runner.run_no_limits(vec![]).expect("a deep gather nest should resolve");
    assert_eq!(result, MontyObject::Int(1));
}

/// Companion to the parked chain: every level settles *during* the commit walk,
/// which is the path that hands each nested result straight back to the frame
/// holding its slot.
#[test]
fn deeply_nested_gather_settling_synchronously_does_not_overflow_the_stack() {
    run_on_a_worker_stack(|| {
        // An empty `gather()` completes on the spot, so all 5,000 levels settle
        // as the walk unwinds rather than parking on a task.
        let code = r"
import asyncio

g = asyncio.gather()
for _ in range(5000):
    g = asyncio.gather(g)
result = await g
for _ in range(5000):
    result = result[0]
result
";
        let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

        let result = runner
            .run_no_limits(vec![])
            .expect("a synchronously settling nest should resolve");
        assert_eq!(result, MontyObject::List(vec![]));
    });
}

/// The error path unwinds the same depth: the innermost gather holds an
/// already-awaited coroutine, so the commit fails 5,000 levels down and every
/// level above must be rolled back.
#[test]
fn deeply_nested_gather_commit_failure_does_not_overflow_the_stack() {
    run_on_a_worker_stack(|| {
        let code = r"
import asyncio

async def leaf():
    return 1

spent = leaf()
await spent

g = asyncio.gather(spent)
for _ in range(5000):
    g = asyncio.gather(g)

caught = ''
try:
    await g
except RuntimeError as exc:
    caught = str(exc)
caught
";
        let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

        let result = runner.run_no_limits(vec![]).expect("the reuse error should be caught");
        assert_eq!(
            result,
            MontyObject::String("cannot reuse already awaited coroutine".to_owned())
        );
    });
}

/// Runs `body` on a 2 MiB thread, the stack a worker gets — libtest's own 8 MiB
/// would hide an overflow that a real session hits.
fn run_on_a_worker_stack(body: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(body)
        .expect("spawning the bounded-stack thread")
        .join()
        .expect("the bounded stack must be enough");
}

// === Test: Deep blocked task chains are torn down without recursing ===

/// A chain of blocked tasks costs no native stack to *build*, so teardown must
/// not turn that stored depth back into frames.
#[test]
fn deep_blocked_task_chain_teardown_does_not_overflow_the_stack() {
    run_on_a_worker_stack(fail_sibling_of_deep_task_chain);
}

/// Wraps `leaf()` in 20,000 nested gathers and awaits that chain alongside a
/// `sibling()`, so both park on external calls: the chain's `parked` (never
/// resolved) and the sibling's `doomed`. Resolving `doomed` with an error
/// fails the outer gather, which cancels all 20,000 blocked tasks in one walk,
/// and asserts that error surfaces as the run's `ValueError`.
///
/// Failing the *sibling* is what makes it a single deep walk — failing the
/// chain's own future would instead unwind it level by level.
fn fail_sibling_of_deep_task_chain() {
    let code = r"
import asyncio

async def leaf():
    return await parked(1)

async def wrap(g):
    return await g

async def sibling():
    return await doomed(2)

g = leaf()
for _ in range(20000):
    g = asyncio.gather(wrap(g))
await asyncio.gather(g, sibling())
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, calls) = drive_collecting_calls(progress);
    let doomed_id = calls
        .iter()
        .find_map(|(id, name)| (name == "doomed").then_some(*id))
        .expect("the sibling should have parked on an external call");
    assert_eq!(calls.len(), 2, "the chain's leaf and the sibling should both park");

    // Failing the sibling tears down the enclosing gather, cancelling the
    // chain top-down; the exception itself only walks up to the main task.
    let error = MontyException::new(ExcType::ValueError, Some("sibling failed".to_string()));
    let result = state.resume(vec![(doomed_id, ExtFunctionResult::Error(error))], PrintWriter::Stdout);

    let exc = result.expect_err("the failed sibling should surface as an exception");
    assert_eq!(exc.exc_type(), ExcType::ValueError);
}

/// Propagating a failure through deeply nested gather waiters must not recurse
/// on the native Rust stack.
#[test]
fn deeply_nested_gather_failure_does_not_overflow_stack() {
    let code = r"
import asyncio

async def leaf():
    raise ValueError('boom')

async def chain(n):
    if n == 0:
        return await asyncio.gather(leaf())
    return await asyncio.gather(chain(n - 1))

caught = False
try:
    await asyncio.gather(chain(4000))
except ValueError:
    caught = True
caught
";

    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let result = runner.run_no_limits(vec![]).expect("should complete");
    assert_eq!(result, MontyObject::Bool(true));
}

// === Test: external call whose result nothing is waiting for ===

/// Leaves a pending external whose result nobody wants: `boom()` raises
/// synchronously, failing the gather while `slow()`'s call is outstanding.
/// `slow()` keeps running, but the gather it would return to has settled.
fn create_orphaned_external_runner() -> MontyRun {
    let code = r"
import asyncio

async def slow():
    return await foo()

async def boom():
    raise ValueError('cancels slow')

async def main():
    try:
        await asyncio.gather(slow(), boom())
    except ValueError:
        pass
    return await bar()

await main()
";
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap()
}

/// Drives the runner above to the suspension where both externals are pending,
/// returning their call ids keyed by name: the order the scheduler hands them
/// over is not part of the contract under test, so positions must not be relied on.
fn orphan_and_live_ids(progress: RunProgress) -> (ResolveFutures, u32, u32) {
    let (state, calls) = drive_collecting_calls(progress);
    assert_eq!(calls.len(), 2, "orphaned foo() and live bar() should both be pending");

    let id_of = |name: &str| {
        calls
            .iter()
            .find_map(|(id, called)| (called == name).then_some(*id))
            .unwrap_or_else(|| panic!("{name}() should be pending"))
    };
    (state, id_of("foo"), id_of("bar"))
}

#[test]
fn orphaned_external_resolved_alongside_live_one() {
    let runner = create_orphaned_external_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, orphan, live) = orphan_and_live_ids(progress);

    // Resolving `bar()` readies `main`. `foo()`'s result reaches `slow()`,
    // whose own result the failed gather discards; the scheduler must still
    // find `main` to run.
    let results = vec![
        (orphan, ExtFunctionResult::Return(MontyObject::Int(1))),
        (live, ExtFunctionResult::Return(MontyObject::Int(7))),
    ];

    let progress = state.resume(results, PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(7));
}

#[test]
fn orphaned_external_failed_alongside_live_one() {
    let runner = create_orphaned_external_runner();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, orphan, live) = orphan_and_live_ids(progress);

    // The orphan's failure raises inside `slow()`, where nothing awaits it,
    // and dies there; only `bar()`'s failure reaches a task.
    let results = vec![
        (
            orphan,
            ExtFunctionResult::Error(MontyException::new(ExcType::RuntimeError, Some("orphan".to_string()))),
        ),
        (
            live,
            ExtFunctionResult::Error(MontyException::new(ExcType::RuntimeError, Some("live".to_string()))),
        ),
    ];

    let err = state
        .resume(results, PrintWriter::Stdout)
        .expect_err("the live failure should surface");
    assert_eq!(err.message(), Some("live"));
}

// === Test: a gather failure leaves siblings running, externals and all ===

/// The sibling of a failed gather child is parked on an external call of its
/// own. That call must still be served, and the sibling must still be holding
/// the frames to resume into — it is no longer torn down with the gather, so
/// switching away from it has to save its context rather than drop it.
#[test]
fn detached_sibling_still_receives_its_external_result() {
    let code = r"
import asyncio

log = []

async def parked():
    log.append(await parked_call(1))

async def doomed():
    return await doomed_call(2)

async def main():
    try:
        await asyncio.gather(parked(), doomed())
    except ValueError as e:
        log.append(str(e))
    # Suspending again is what gives the detached sibling a turn; a run whose
    # main task never waits again ends with it still parked.
    log.append(await tail_call(3))
    return log

await main()
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, calls) = drive_collecting_calls(progress);
    let call_id = |name: &str| {
        calls
            .iter()
            .find_map(|(id, call_name)| (call_name == name).then_some(*id))
            .unwrap_or_else(|| panic!("{name} should have parked"))
    };

    // Fail the gather through one child while the other is still parked.
    let error = MontyException::new(ExcType::ValueError, Some("doomed failed".to_string()));
    let progress = state
        .resume(
            vec![(call_id("doomed_call"), ExtFunctionResult::Error(error))],
            PrintWriter::Stdout,
        )
        .unwrap();

    // The main task carries on to its own call, and the sibling's is still
    // outstanding alongside it.
    let (state, tail_calls) = drive_collecting_calls(progress);
    let tail_id = tail_calls
        .iter()
        .find_map(|(id, name)| (name == "tail_call").then_some(*id))
        .expect("the main task should have parked on its own call");

    // Answer the sibling's call alone: with the main task still parked, the
    // sibling is the one ready task, so it resumes and finishes its work.
    let progress = state
        .resume(
            vec![(call_id("parked_call"), ExtFunctionResult::Return(MontyObject::Int(7)))],
            PrintWriter::Stdout,
        )
        .unwrap();
    let state = progress
        .into_resolve_futures()
        .expect("the main task's own call should still be pending");
    let progress = state
        .resume(
            vec![(tail_id, ExtFunctionResult::Return(MontyObject::Int(3)))],
            PrintWriter::Stdout,
        )
        .unwrap();

    let result = progress.into_complete().expect("should complete");
    assert_eq!(
        result,
        MontyObject::List(vec![
            MontyObject::String("doomed failed".to_string()),
            MontyObject::Int(7),
            MontyObject::Int(3)
        ]),
        "the main task caught the failure and the sibling still logged its result"
    );
}

/// A task raising with nothing left awaiting it is discarded silently.
///
/// The exception has nowhere to go: the gather that was waiting on the task
/// settled on its sibling's error. CPython's `gather` retrieves each child's
/// exception through a done-callback even after settling, so it prints nothing
/// either. The run must carry on, and its output must stay clean.
#[test]
fn discarded_task_exception_is_silent() {
    let code = r"
import asyncio

async def parked():
    await parked_call(1)
    raise ValueError('nobody is waiting')

async def doomed():
    return await doomed_call(2)

async def main():
    try:
        await asyncio.gather(parked(), doomed())
    except ValueError:
        pass
    return await tail_call(3)

await main()
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, calls) = drive_collecting_calls(progress);
    let call_id = |name: &str| {
        calls
            .iter()
            .find_map(|(id, call_name)| (call_name == name).then_some(*id))
            .unwrap_or_else(|| panic!("{name} should have parked"))
    };

    // Fail the gather, detaching `parked()`.
    let error = MontyException::new(ExcType::ValueError, Some("doomed failed".to_string()));
    let progress = state
        .resume(
            vec![(call_id("doomed_call"), ExtFunctionResult::Error(error))],
            PrintWriter::Stdout,
        )
        .unwrap();
    let (state, tail_calls) = drive_collecting_calls(progress);
    let tail_id = tail_calls
        .iter()
        .find_map(|(id, name)| (name == "tail_call").then_some(*id))
        .expect("the main task should have parked on its own call");

    // Resume only the sibling's call, so it runs into its `raise` while the
    // main task is still parked — the run would otherwise finish first and
    // never give the detached task a turn.
    let mut output = String::new();
    let progress = state
        .resume(
            vec![(call_id("parked_call"), ExtFunctionResult::Return(MontyObject::Int(1)))],
            PrintWriter::collect_string(&mut output),
        )
        .unwrap();
    assert_eq!(output, "");

    let RunProgress::ResolveFutures(state) = progress else {
        panic!("the main task should still be parked on its own call")
    };
    let result = state
        .resume(
            vec![(tail_id, ExtFunctionResult::Return(MontyObject::Int(3)))],
            PrintWriter::Stdout,
        )
        .unwrap();
    let RunProgress::Complete(complete) = result else {
        panic!("the discarded exception must not fail the run")
    };
    assert_eq!(complete, MontyObject::Int(3));
}

/// A detached sibling's own call failing raises inside that sibling, where it
/// can be caught. Uncaught, it has nowhere left to go — the gather that was
/// waiting on the sibling has already settled — so it must not surface as the
/// run's error.
#[test]
fn detached_sibling_failure_does_not_surface() {
    let code = r"
import asyncio

log = []

async def parked():
    await parked_call(1)
    log.append('not reached')

async def doomed():
    return await doomed_call(2)

async def main():
    try:
        await asyncio.gather(parked(), doomed())
    except ValueError as e:
        log.append(str(e))
    log.append(await tail_call(3))
    return log

await main()
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let (state, calls) = drive_collecting_calls(progress);
    let call_id = |name: &str| {
        calls
            .iter()
            .find_map(|(id, call_name)| (call_name == name).then_some(*id))
            .unwrap_or_else(|| panic!("{name} should have parked"))
    };

    let error = MontyException::new(ExcType::ValueError, Some("doomed failed".to_string()));
    let progress = state
        .resume(
            vec![(call_id("doomed_call"), ExtFunctionResult::Error(error))],
            PrintWriter::Stdout,
        )
        .unwrap();

    let (state, tail_calls) = drive_collecting_calls(progress);
    let tail_id = tail_calls
        .iter()
        .find_map(|(id, name)| (name == "tail_call").then_some(*id))
        .expect("the main task should have parked on its own call");

    // Reject the detached sibling's call: it raises in a task nobody is
    // waiting on, and dies there.
    let error = MontyException::new(ExcType::KeyError, Some("nobody is listening".to_string()));
    let progress = state
        .resume(
            vec![
                (call_id("parked_call"), ExtFunctionResult::Error(error)),
                (tail_id, ExtFunctionResult::Return(MontyObject::Int(3))),
            ],
            PrintWriter::Stdout,
        )
        .unwrap();

    let result = progress
        .into_complete()
        .expect("the detached failure must not end the run");
    assert_eq!(
        result,
        MontyObject::List(vec![
            MontyObject::String("doomed failed".to_string()),
            MontyObject::Int(3)
        ]),
        "only the awaited failure reached the main task"
    );
}
