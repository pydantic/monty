//! Tests for host-facing runtime IDs on suspendable execution payloads.
//!
//! These tests validate that runtime IDs are available to the host and remain
//! stable across pause/resume and dump/load boundaries.

use std::collections::HashSet;

use monty::{
    FunctionCall, MontyObject, MontyRun, NoLimitTracker, PrintWriter, ResourceTracker, RunProgress, RuntimeValueId,
};
use rstest::{fixture, rstest};

#[fixture]
fn started_progress(#[default("ext_fn([])")] code: &str) -> RunProgress<NoLimitTracker> {
    MontyRun::new(code.to_owned(), "test.py", vec![])
        .expect("runner creation should succeed")
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should pause at external call")
}

fn into_function_call(progress: RunProgress<NoLimitTracker>, context: &str) -> FunctionCall<NoLimitTracker> {
    match progress {
        RunProgress::FunctionCall(call) => call,
        other => panic!("{context}: expected function call, got {other:?}"),
    }
}

fn extract_arg_runtime_ids(progress: &RunProgress<NoLimitTracker>) -> &[RuntimeValueId] {
    match progress {
        RunProgress::FunctionCall(call) => &call.arg_runtime_ids,
        other => panic!("expected function call, got {other:?}"),
    }
}

fn resume_with_none<T: ResourceTracker>(call: monty::FunctionCall<T>) -> RunProgress<T> {
    call.resume(MontyObject::None, &mut PrintWriter::Stdout)
        .expect("resume should succeed")
}

fn validate_kwarg_function_call_and_extract_ids(
    function_call: &FunctionCall<NoLimitTracker>,
    context: &str,
) -> Vec<(usize, usize)> {
    assert!(
        function_call.args.is_empty(),
        "{context}: expected keyword-only external call"
    );
    assert!(
        function_call.arg_runtime_ids.is_empty(),
        "{context}: expected no positional runtime IDs"
    );
    assert_eq!(
        function_call.kwargs.len(),
        2,
        "{context}: expected two keyword arguments"
    );
    assert_eq!(
        function_call.kwarg_runtime_ids.len(),
        function_call.kwargs.len(),
        "{context}: kwarg runtime IDs should align 1:1 with kwargs"
    );
    assert_eq!(
        function_call.kwargs,
        vec![
            (MontyObject::String("a".to_owned()), MontyObject::Int(1)),
            (MontyObject::String("b".to_owned()), MontyObject::Int(2)),
        ],
        "{context}: keyword payload should match expected pairs"
    );

    function_call
        .kwarg_runtime_ids
        .iter()
        .map(|(key_id, value_id)| (key_id.raw(), value_id.raw()))
        .collect()
}

#[rstest]
fn function_call_runtime_ids_are_unique_for_distinct_positional_arguments(
    #[with("ext_fn(1, 2, 'three', [4])")] started_progress: RunProgress<NoLimitTracker>,
) {
    let function_call = into_function_call(started_progress, "distinct positional IDs");

    assert_eq!(function_call.args.len(), function_call.arg_runtime_ids.len());
    assert!(function_call.kwargs.is_empty());
    assert!(function_call.kwarg_runtime_ids.is_empty());

    let unique_ids: HashSet<usize> = function_call.arg_runtime_ids.iter().map(|id| id.raw()).collect();
    assert_eq!(unique_ids.len(), function_call.arg_runtime_ids.len());

    let completion = resume_with_none(function_call);
    assert!(matches!(completion, RunProgress::Complete(_)));
}

#[rstest]
#[case("x = []; ext_fn(x, x)", true)]
#[case("ext_fn([], [])", false)]
fn function_call_runtime_id_identity_matches_object_identity(
    #[case] code: &str,
    #[case] should_match: bool,
    #[with(code)] started_progress: RunProgress<NoLimitTracker>,
) {
    let function_call = into_function_call(started_progress, "identity semantics");

    assert_eq!(
        function_call.arg_runtime_ids.len(),
        2,
        "{code}: expected two positional IDs"
    );
    let ids_match = function_call.arg_runtime_ids[0] == function_call.arg_runtime_ids[1];
    assert_eq!(
        ids_match, should_match,
        "{code}: runtime IDs should reflect object identity semantics"
    );

    let completion = resume_with_none(function_call);
    assert!(
        matches!(completion, RunProgress::Complete(_)),
        "single call script should complete after one resume"
    );
}

#[rstest]
fn function_call_kwarg_runtime_ids_match_kwargs_and_are_stable_across_dump_load(
    #[with("ext_fn(a=1, b=2)")] started_progress: RunProgress<NoLimitTracker>,
) {
    let bytes = started_progress.dump().expect("run progress dump should succeed");

    let first_call = into_function_call(started_progress, "initial state");
    let first_kwarg_runtime_ids = validate_kwarg_function_call_and_extract_ids(&first_call, "initial state");
    assert_ne!(
        first_kwarg_runtime_ids[0], first_kwarg_runtime_ids[1],
        "distinct kwargs should have distinct (key, value) runtime IDs"
    );
    let first_completion = resume_with_none(first_call);
    assert!(matches!(first_completion, RunProgress::Complete(_)));

    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");
    let second_call = into_function_call(loaded, "after dump/load");
    let second_kwarg_runtime_ids = validate_kwarg_function_call_and_extract_ids(&second_call, "after dump/load");
    assert_eq!(
        second_kwarg_runtime_ids, first_kwarg_runtime_ids,
        "kwarg runtime IDs should remain stable across dump/load"
    );
    let second_completion = resume_with_none(second_call);
    assert!(matches!(second_completion, RunProgress::Complete(_)));
}

#[rstest]
fn runtime_ids_round_trip_with_run_progress_dump_load(
    #[with("ext_fn([])")] started_progress: RunProgress<NoLimitTracker>,
) {
    let expected_ids: Vec<usize> = started_progress
        .runtime_ids()
        .expect("function call should expose runtime ids")
        .0
        .iter()
        .map(|id| id.raw())
        .collect();

    let bytes = started_progress.dump().expect("run progress dump should succeed");

    let original_completion = resume_with_none(into_function_call(started_progress, "original call"));
    assert!(matches!(original_completion, RunProgress::Complete(_)));

    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");
    let loaded_ids: Vec<usize> = loaded
        .runtime_ids()
        .expect("loaded function call should expose runtime ids")
        .0
        .iter()
        .map(|id| id.raw())
        .collect();
    assert_eq!(loaded_ids, expected_ids);

    let loaded_completion = resume_with_none(into_function_call(loaded, "loaded call"));
    assert!(matches!(loaded_completion, RunProgress::Complete(_)));
}

#[rstest]
fn into_function_call_includes_runtime_ids(#[with("ext_fn(a=1, b=2)")] started_progress: RunProgress<NoLimitTracker>) {
    let expected_runtime_ids = started_progress
        .runtime_ids()
        .expect("function call should expose runtime IDs");
    let expected_arg_runtime_ids = expected_runtime_ids.0.to_vec();
    let expected_kwarg_runtime_ids = expected_runtime_ids.1.to_vec();

    let function_call = into_function_call(started_progress, "into_function_call");

    assert_eq!(function_call.arg_runtime_ids, expected_arg_runtime_ids);
    assert_eq!(function_call.kwarg_runtime_ids, expected_kwarg_runtime_ids);
    assert_eq!(function_call.args.len(), function_call.arg_runtime_ids.len());
    assert_eq!(function_call.kwargs.len(), function_call.kwarg_runtime_ids.len());

    let completion = resume_with_none(function_call);
    assert!(matches!(completion, RunProgress::Complete(_)));
}

#[rstest]
fn runtime_ids_are_unavailable_for_non_call_progress() {
    let progress = RunProgress::<NoLimitTracker>::Complete(MontyObject::None);
    assert!(progress.runtime_ids().is_none());
}

#[rstest]
fn runtime_ids_remain_stable_across_resume_boundaries(
    #[with("x = []; ext_fn(x); ext_fn(x)")] started_progress: RunProgress<NoLimitTracker>,
) {
    let first_call = into_function_call(started_progress, "first function call");
    let first_id = first_call
        .arg_runtime_ids
        .first()
        .expect("first call should include one arg id")
        .raw();

    let progress = resume_with_none(first_call);
    let second_call = into_function_call(progress, "second function call");
    let second_id = second_call
        .arg_runtime_ids
        .first()
        .expect("second call should include one arg id")
        .raw();

    assert_eq!(first_id, second_id);

    let completion = resume_with_none(second_call);
    assert!(matches!(completion, RunProgress::Complete(_)));
}

#[rstest]
fn runtime_ids_remain_stable_across_run_progress_dump_load_and_resume(
    #[with("x = []; ext_fn(x); ext_fn(x)")] started_progress: RunProgress<NoLimitTracker>,
) {
    let bytes = started_progress.dump().expect("run progress dump should succeed");

    let first_call = into_function_call(started_progress, "first function call");
    let first_id = first_call
        .arg_runtime_ids
        .first()
        .expect("first call should include one arg id")
        .raw();

    let second_call_original = into_function_call(resume_with_none(first_call), "original second function call");
    let completion_original = resume_with_none(second_call_original);
    assert!(matches!(completion_original, RunProgress::Complete(_)));

    let loaded_progress: RunProgress<NoLimitTracker> =
        RunProgress::load(&bytes).expect("run progress load should succeed");

    let first_loaded_call = into_function_call(loaded_progress, "loaded first function call");
    let second_loaded_call = into_function_call(resume_with_none(first_loaded_call), "loaded second function call");
    let second_id = second_loaded_call
        .arg_runtime_ids
        .first()
        .expect("second call should include one arg id")
        .raw();

    assert_eq!(first_id, second_id);

    let completion_loaded = resume_with_none(second_loaded_call);
    assert!(matches!(completion_loaded, RunProgress::Complete(_)));
}

#[rstest]
fn corrupted_run_progress_payload_fails_to_load(#[with("ext_fn([])")] started_progress: RunProgress<NoLimitTracker>) {
    let mut bytes = started_progress.dump().expect("run progress dump should succeed");
    assert!(
        RunProgress::<NoLimitTracker>::load(&bytes).is_ok(),
        "unmodified run progress payload should load"
    );

    let completion = resume_with_none(into_function_call(started_progress, "corrupted payload original call"));
    assert!(matches!(completion, RunProgress::Complete(_)));

    assert!(!bytes.is_empty(), "serialized run progress should not be empty");
    bytes[0] ^= 0xFF;

    assert!(RunProgress::<NoLimitTracker>::load(&bytes).is_err());
}

#[rstest]
fn extract_arg_runtime_ids_returns_runtime_ids(#[with("ext_fn(1, 2)")] started_progress: RunProgress<NoLimitTracker>) {
    assert_eq!(extract_arg_runtime_ids(&started_progress).len(), 2);

    let completion = resume_with_none(into_function_call(started_progress, "extract helper"));
    assert!(matches!(completion, RunProgress::Complete(_)));
}
