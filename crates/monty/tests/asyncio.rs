//! Tests for async edge cases around FutureSnapshot::resume behavior.
//!
//! These tests verify the behavior of the async execution model, specifically around
//! resolving external futures incrementally via `FutureSnapshot::resume()`.

use monty::{ExcType, ExternalResult, MontyException, MontyObject, MontyRun, NoLimitTracker, PrintWriter, RunProgress};

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
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec![],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap()
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
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec![],
        vec!["foo".to_owned(), "bar".to_owned(), "baz".to_owned()],
    )
    .unwrap()
}

/// Helper to drive execution through external calls until we get ResolveFutures.
///
/// Returns (pending_call_ids, state, collected_call_ids) where collected_call_ids
/// are the call_ids from all the FunctionCalls we processed with run_pending().
fn drive_to_resolve_futures<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> (monty::FutureSnapshot<T>, Vec<u32>) {
    let mut collected_call_ids = Vec::new();

    loop {
        match progress {
            RunProgress::FunctionCall { call_id, state, .. } => {
                collected_call_ids.push(call_id);
                progress = state.run_pending(&mut PrintWriter::Stdout).unwrap();
            }
            RunProgress::ResolveFutures(state) => {
                return (state, collected_call_ids);
            }
            RunProgress::Complete(_) => {
                panic!("unexpected Complete before ResolveFutures");
            }
            RunProgress::OsCall { function, .. } => {
                panic!("unexpected OsCall: {function:?}");
            }
        }
    }
}

// === Test: Resume with all call_ids at once ===

#[test]
fn resume_with_all_call_ids() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Should have two pending calls
    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");
    assert_eq!(call_ids.len(), 2, "should have collected 2 call_ids");

    // Resolve both at once: foo() returns 10, bar() returns 32
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(10))),
        (call_ids[1], ExternalResult::Return(MontyObject::Int(32))),
    ];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should complete with 10 + 32 = 42
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with partial call_ids (incremental resolution) ===

#[test]
fn resume_with_partial_call_ids() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Resolve only the first one
    let results = vec![(call_ids[0], ExternalResult::Return(MontyObject::Int(10)))];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should return ResolveFutures with the remaining call
    let state = progress.into_resolve_futures().expect("should need more futures");

    assert_eq!(
        state.pending_call_ids().len(),
        1,
        "should have 1 remaining pending call"
    );
    assert_eq!(
        state.pending_call_ids()[0],
        call_ids[1],
        "remaining should be the second call"
    );

    // Now resolve the second one
    let results = vec![(call_ids[1], ExternalResult::Return(MontyObject::Int(32)))];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should complete with 10 + 32 = 42
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with unknown call_id errors ===

#[test]
fn resume_with_unknown_call_id_errors() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, _call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Try to resolve with an unknown call_id (9999)
    let results = vec![(9999, ExternalResult::Return(MontyObject::Int(10)))];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should error on unknown call_id");
    let exc = result.unwrap_err();
    assert!(
        exc.message().unwrap_or("").contains("unknown call_id 9999"),
        "error message should mention the unknown call_id: {:?}",
        exc.message()
    );
}

// === Test: Resume with empty results ===

#[test]
fn resume_with_empty_results() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Resume with empty results - should return same pending list
    let results: Vec<(u32, ExternalResult)> = vec![];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should return ResolveFutures with the same pending calls
    let state = progress.into_resolve_futures().expect("should still need futures");

    assert_eq!(state.pending_call_ids().len(), 2, "should still have 2 pending calls");
    assert!(
        state.pending_call_ids().contains(&call_ids[0]),
        "should contain first call_id"
    );
    assert!(
        state.pending_call_ids().contains(&call_ids[1]),
        "should contain second call_id"
    );

    // Now resolve both to complete
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(10))),
        (call_ids[1], ExternalResult::Return(MontyObject::Int(32))),
    ];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume with mixed success and failure ===

#[test]
fn resume_with_mixed_success_and_failure() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // First succeeds, second fails with an exception
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(10))),
        (
            call_ids[1],
            ExternalResult::Error(MontyException::new(
                ExcType::ValueError,
                Some("external error".to_string()),
            )),
        ),
    ];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    // Should propagate the exception
    assert!(result.is_err(), "should propagate the error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("external error"));
}

// === Test: Resume order independence ===

#[test]
fn resume_order_independence() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Resolve in REVERSE order - second call first, first call second
    let results = vec![
        (call_ids[1], ExternalResult::Return(MontyObject::Int(32))), // bar() = 32
        (call_ids[0], ExternalResult::Return(MontyObject::Int(10))), // foo() = 10
    ];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should still complete with foo() + bar() = 10 + 32 = 42
    // (gather preserves order of original awaitables, not resolution order)
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Resume multiple rounds ===

#[test]
fn resume_multiple_rounds() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 3, "should have 3 pending calls");
    assert_eq!(call_ids.len(), 3, "should have collected 3 call_ids");

    // Round 1: resolve first call only
    let results = vec![(call_ids[0], ExternalResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need more futures");
    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 remaining");

    // Round 2: resolve second call only
    let results = vec![(call_ids[1], ExternalResult::Return(MontyObject::Int(200)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need more futures");
    assert_eq!(state.pending_call_ids().len(), 1, "should have 1 remaining");

    // Round 3: resolve third call
    let results = vec![(call_ids[2], ExternalResult::Return(MontyObject::Int(300)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should complete with 100 + 200 + 300 = 600
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(600));
}

// === Test: Resume with duplicate call_id ===

#[test]
fn resume_with_duplicate_call_id() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // Provide the same call_id twice with different values.
    // The first resolution wins because after resolving, the call_id is removed
    // from gather_waiters, so subsequent resolutions for the same call_id are ignored.
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(10))),
        (call_ids[0], ExternalResult::Return(MontyObject::Int(99))), // duplicate - ignored!
        (call_ids[1], ExternalResult::Return(MontyObject::Int(32))),
    ];

    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Should complete with first value used: 10 + 32 = 42
    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// =============================================================================
// External Function Error Tests
// =============================================================================
// These tests verify that errors from external functions are properly propagated,
// especially important after the scheduler optimizations that changed how
// pending_calls is used for O(1) task lookup.

/// Helper to create a runner that awaits a single external function (non-gather).
fn create_single_await_runner() -> MontyRun {
    let code = r"
async def main():
    result = await foo()
    return result

await main()
";
    MontyRun::new(code.to_owned(), "test.py", vec![], vec!["foo".to_owned()]).unwrap()
}

/// Helper to create a runner with sequential awaits (not gather).
fn create_sequential_awaits_runner() -> MontyRun {
    let code = r"
async def main():
    a = await foo()
    b = await bar()
    return a + b

await main()
";
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec![],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap()
}

// === Test: Single external await success (non-gather baseline) ===

#[test]
fn single_external_await_success() {
    let runner = create_single_await_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 1, "should have 1 pending call");
    assert_eq!(call_ids.len(), 1, "should have collected 1 call_id");

    // Resolve with success
    let results = vec![(call_ids[0], ExternalResult::Return(MontyObject::Int(42)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    let result = progress.into_complete().expect("should complete");
    assert_eq!(result, MontyObject::Int(42));
}

// === Test: Single external await with error (non-gather) ===
// This is the critical test that was failing before the fix to fail_future().
// When a single external function (not in a gather) raises an exception,
// it must propagate correctly through fail_for_call() which uses pending_calls
// for O(1) task lookup.

#[test]
fn single_external_await_error() {
    let runner = create_single_await_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 1, "should have 1 pending call");

    // Fail with an exception
    let results = vec![(
        call_ids[0],
        ExternalResult::Error(MontyException::new(
            ExcType::ValueError,
            Some("single await error".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate the error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("single await error"));
}

// === Test: Single external await with RuntimeError ===

#[test]
fn single_external_await_runtime_error() {
    let runner = create_single_await_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    let results = vec![(
        call_ids[0],
        ExternalResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("runtime failure".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate RuntimeError");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("runtime failure"));
}

// === Test: Single external await with TypeError ===

#[test]
fn single_external_await_type_error() {
    let runner = create_single_await_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    let results = vec![(
        call_ids[0],
        ExternalResult::Error(MontyException::new(
            ExcType::TypeError,
            Some("type mismatch".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate TypeError");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TypeError);
    assert_eq!(exc.message(), Some("type mismatch"));
}

// === Test: Sequential awaits - first succeeds, second fails ===

#[test]
fn sequential_awaits_second_fails() {
    let runner = create_sequential_awaits_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    // First external call (foo)
    let RunProgress::FunctionCall { call_id, state, .. } = progress else {
        panic!("expected FunctionCall for foo");
    };
    let foo_call_id = call_id;
    let progress = state.run_pending(&mut PrintWriter::Stdout).unwrap();

    // Should yield for resolution
    let state = progress.into_resolve_futures().expect("should need foo resolved");
    assert_eq!(state.pending_call_ids(), vec![foo_call_id]);

    // Resolve foo successfully
    let results = vec![(foo_call_id, ExternalResult::Return(MontyObject::Int(10)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    // Second external call (bar)
    let RunProgress::FunctionCall { call_id, state, .. } = progress else {
        panic!("expected FunctionCall for bar");
    };
    let bar_call_id = call_id;
    let progress = state.run_pending(&mut PrintWriter::Stdout).unwrap();

    // Should yield for resolution
    let state = progress.into_resolve_futures().expect("should need bar resolved");
    assert_eq!(state.pending_call_ids(), vec![bar_call_id]);

    // Fail bar with an exception
    let results = vec![(
        bar_call_id,
        ExternalResult::Error(MontyException::new(ExcType::ValueError, Some("bar failed".to_string()))),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate bar's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("bar failed"));
}

// === Test: Sequential awaits - first fails ===

#[test]
fn sequential_awaits_first_fails() {
    let runner = create_sequential_awaits_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    // First external call (foo)
    let RunProgress::FunctionCall { call_id, state, .. } = progress else {
        panic!("expected FunctionCall for foo");
    };
    let foo_call_id = call_id;
    let progress = state.run_pending(&mut PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need foo resolved");

    // Fail foo with an exception - bar should never be called
    let results = vec![(
        foo_call_id,
        ExternalResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("foo failed early".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate foo's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("foo failed early"));
}

// === Test: Gather - first external fails before second is resolved ===

#[test]
fn gather_first_external_fails_immediately() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 pending calls");

    // First fails, second not provided (simulates first returning error before second)
    let results = vec![(
        call_ids[0],
        ExternalResult::Error(MontyException::new(
            ExcType::ValueError,
            Some("first failed".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    // Should propagate the error immediately
    assert!(result.is_err(), "should propagate first's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("first failed"));
}

// === Test: Gather - second external fails, first not yet resolved ===

#[test]
fn gather_second_external_fails_first_pending() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Only resolve second with an error, leave first pending
    let results = vec![(
        call_ids[1],
        ExternalResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("second failed first".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate second's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("second failed first"));
}

// === Test: Gather - all external futures fail ===

#[test]
fn gather_all_externals_fail() {
    let runner = create_gather_two_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Both fail - first error should be reported
    let results = vec![
        (
            call_ids[0],
            ExternalResult::Error(MontyException::new(
                ExcType::ValueError,
                Some("first error".to_string()),
            )),
        ),
        (
            call_ids[1],
            ExternalResult::Error(MontyException::new(
                ExcType::RuntimeError,
                Some("second error".to_string()),
            )),
        ),
    ];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    // First error in the list should be propagated
    assert!(result.is_err(), "should propagate an error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("first error"));
}

// === Test: Gather with three - middle one fails ===

#[test]
fn gather_three_middle_fails() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    assert_eq!(call_ids.len(), 3, "should have 3 call_ids");

    // First and third succeed, middle fails
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(100))),
        (
            call_ids[1],
            ExternalResult::Error(MontyException::new(
                ExcType::ValueError,
                Some("middle failed".to_string()),
            )),
        ),
        (call_ids[2], ExternalResult::Return(MontyObject::Int(300))),
    ];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate middle's error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("middle failed"));
}

// === Test: Error in incremental resolution (resolve one, then error on next) ===

#[test]
fn gather_incremental_error_after_success() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Round 1: resolve first successfully
    let results = vec![(call_ids[0], ExternalResult::Return(MontyObject::Int(100)))];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need more");
    assert_eq!(state.pending_call_ids().len(), 2, "should have 2 remaining");

    // Round 2: second fails
    let results = vec![(
        call_ids[1],
        ExternalResult::Error(MontyException::new(
            ExcType::ValueError,
            Some("delayed failure".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate delayed error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("delayed failure"));
}

// === Test: Error in last incremental resolution ===

#[test]
fn gather_incremental_error_on_last() {
    let runner = create_gather_three_runner();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let (state, call_ids) = drive_to_resolve_futures(progress);

    // Round 1: first two succeed
    let results = vec![
        (call_ids[0], ExternalResult::Return(MontyObject::Int(100))),
        (call_ids[1], ExternalResult::Return(MontyObject::Int(200))),
    ];
    let progress = state.resume(results, &mut PrintWriter::Stdout).unwrap();

    let state = progress.into_resolve_futures().expect("should need last one");
    assert_eq!(state.pending_call_ids().len(), 1, "should have 1 remaining");

    // Round 2: last one fails
    let results = vec![(
        call_ids[2],
        ExternalResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("last one failed".to_string()),
        )),
    )];

    let result = state.resume(results, &mut PrintWriter::Stdout);

    assert!(result.is_err(), "should propagate last error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("last one failed"));
}
