//! Shared helpers for runtime-observer integration tests.

use monty::{FunctionCall, OsCall, ResourceTracker, RunProgress};

/// Extracts a function-call progress variant with a contextual panic message.
///
/// # Panics
/// Panics when `progress` is not `RunProgress::FunctionCall`.
pub fn as_function_call<T: ResourceTracker>(progress: RunProgress<T>, context: &str) -> FunctionCall<T> {
    match progress {
        RunProgress::FunctionCall(call) => call,
        other => panic!("{context}: expected function-call progress, got {other:?}"),
    }
}

/// Extracts an OS-call progress variant with a contextual panic message.
///
/// # Panics
/// Panics when `progress` is not `RunProgress::OsCall`.
pub fn as_os_call<T: ResourceTracker>(progress: RunProgress<T>, context: &str) -> OsCall<T> {
    match progress {
        RunProgress::OsCall(call) => call,
        other => panic!("{context}: expected OS-call progress, got {other:?}"),
    }
}

/// Test helper for runtime-observer integration tests that performs deep
/// equality assertions across significant `FunctionCall<T>` fields.
///
/// This compares `function_name`, `args`, `kwargs`, `call_id`, `method_call`,
/// `arg_runtime_ids`, and `kwarg_runtime_ids` for two call snapshots.
/// The generic `T: ResourceTracker` matches the tracker used by each call.
///
/// The helper panics on any mismatch via `assert_eq!` and intentionally does
/// not return a `Result`.
pub fn assert_function_calls_equal<T: ResourceTracker>(left: &FunctionCall<T>, right: &FunctionCall<T>) {
    assert_eq!(left.function_name, right.function_name);
    assert_eq!(left.args, right.args);
    assert_eq!(left.kwargs, right.kwargs);
    assert_eq!(left.call_id, right.call_id);
    assert_eq!(left.method_call, right.method_call);
    assert_eq!(left.arg_runtime_ids, right.arg_runtime_ids);
    assert_eq!(left.kwarg_runtime_ids, right.kwarg_runtime_ids);
}
