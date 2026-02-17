//! Tests for stateful REPL execution with no replay.
//!
//! The REPL session keeps heap/global namespace state between snippets and executes
//! only the newly fed snippet each time.

use monty::{
    ExternalResult, MontyObject, MontyRepl, NoLimitTracker, PrintWriter, ReplContinuationMode, ReplProgress,
    detect_repl_continuation_mode,
};

fn init_repl(code: &str, external_functions: Vec<String>) -> (MontyRepl<NoLimitTracker>, MontyObject) {
    MontyRepl::new(
        code.to_owned(),
        "repl.py",
        vec![],
        external_functions,
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    )
    .unwrap()
}

#[test]
fn repl_executes_only_new_code() {
    let (mut repl, init_output) = init_repl("counter = 0", vec![]);
    assert_eq!(init_output, MontyObject::None);

    // Execute a snippet that mutates state.
    let output = repl.feed_no_print("counter = counter + 1").unwrap();
    assert_eq!(output, MontyObject::None);

    // Feed only the read expression. If replay happened, we'd get 2 instead of 1.
    let output = repl.feed_no_print("counter").unwrap();
    assert_eq!(output, MontyObject::Int(1));
}

#[test]
fn repl_persists_state_and_definitions() {
    let (mut repl, _) = init_repl("x = 10", vec![]);

    repl.feed_no_print("def add(v):\n    return x + v").unwrap();
    repl.feed_no_print("x = 20").unwrap();
    let output = repl.feed_no_print("add(22)").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_function_redefinition_uses_latest_definition() {
    let (mut repl, init_output) = init_repl("", vec![]);
    assert_eq!(init_output, MontyObject::None);

    repl.feed_no_print("def f():\n    return 1").unwrap();
    assert_eq!(repl.feed_no_print("f()").unwrap(), MontyObject::Int(1));

    repl.feed_no_print("def f():\n    return 2").unwrap();
    assert_eq!(repl.feed_no_print("f()").unwrap(), MontyObject::Int(2));
}

#[test]
fn repl_nested_function_redefinition_updates_callers() {
    let (mut repl, init_output) = init_repl("", vec![]);
    assert_eq!(init_output, MontyObject::None);

    repl.feed_no_print("def g():\n    return 10").unwrap();
    repl.feed_no_print("def f():\n    return g() + 1").unwrap();
    assert_eq!(repl.feed_no_print("f()").unwrap(), MontyObject::Int(11));

    repl.feed_no_print("def g():\n    return 41").unwrap();
    assert_eq!(repl.feed_no_print("f()").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_runtime_error_keeps_partial_state_consistent() {
    let (mut repl, init_output) = init_repl("", vec![]);
    assert_eq!(init_output, MontyObject::None);

    let result = repl.feed_no_print("def f():\n    return 41\nx = 1\nraise RuntimeError('boom')");
    assert!(result.is_err(), "snippet should raise RuntimeError");

    // Definitions and assignments that happened before the exception should remain valid.
    assert_eq!(repl.feed_no_print("f()").unwrap(), MontyObject::Int(41));
    assert_eq!(repl.feed_no_print("x").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_heap_mutations_are_not_replayed() {
    let (mut repl, _) = init_repl("items = []", vec![]);

    repl.feed_no_print("items.append(1)").unwrap();
    assert_eq!(
        repl.feed_no_print("items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1)])
    );

    repl.feed_no_print("items.append(2)").unwrap();
    assert_eq!(
        repl.feed_no_print("items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_detects_continuation_mode_for_common_cases() {
    assert_eq!(
        detect_repl_continuation_mode("value = 1\n"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("if True:\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(
        detect_repl_continuation_mode("[1,\n"),
        ReplContinuationMode::IncompleteImplicit
    );
}

#[test]
fn repl_tracebacks_use_incrementing_python_input_filenames() {
    let (mut repl, init_output) = init_repl("", vec![]);
    assert_eq!(init_output, MontyObject::None);

    let first = repl.feed_no_print("missing_name").unwrap_err();
    let second = repl.feed_no_print("missing_name").unwrap_err();

    assert_eq!(first.traceback().len(), 1);
    assert_eq!(second.traceback().len(), 1);
    assert_eq!(first.traceback()[0].filename, "<python-input-0>");
    assert_eq!(second.traceback()[0].filename, "<python-input-1>");
}

#[test]
fn repl_dump_load_survives_between_snippets() {
    let (mut repl, _) = init_repl("total = 1", vec![]);
    repl.feed_no_print("total = total + 1").unwrap();

    let bytes = repl.dump().unwrap();
    let mut loaded: MontyRepl<NoLimitTracker> = MontyRepl::load(&bytes).unwrap();

    loaded.feed_no_print("total = total * 21").unwrap();
    let output = loaded.feed_no_print("total").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_dump_load_preserves_heap_aliasing() {
    let (mut repl, _) = init_repl("a = []\nb = a", vec![]);

    repl.feed_no_print("a.append(1)").unwrap();

    let bytes = repl.dump().unwrap();
    let mut loaded: MontyRepl<NoLimitTracker> = MontyRepl::load(&bytes).unwrap();

    loaded.feed_no_print("b.append(2)").unwrap();
    assert_eq!(
        loaded.feed_no_print("a").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
    assert_eq!(
        loaded.feed_no_print("b").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_start_external_call_resumes_to_updated_repl() {
    let (repl, init_output) = init_repl("", vec!["ext_fn".to_owned()]);
    assert_eq!(init_output, MontyObject::None);

    let progress = repl.start("ext_fn(41) + 1", &mut PrintWriter::Stdout).unwrap();
    let (function_name, args, _kwargs, _call_id, state) =
        progress.into_function_call().expect("expected function call");
    assert_eq!(function_name, "ext_fn");
    assert_eq!(args, vec![MontyObject::Int(41)]);

    let progress = state.run(MontyObject::Int(41), &mut PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(repl.feed_no_print("x = 5").unwrap(), MontyObject::None);
    assert_eq!(repl.feed_no_print("x").unwrap(), MontyObject::Int(5));
}

#[test]
fn repl_progress_dump_load_roundtrip() {
    let (repl, _) = init_repl("", vec!["ext_fn".to_owned()]);

    let progress = repl.start("ext_fn(20) + 22", &mut PrintWriter::Stdout).unwrap();
    let bytes = progress.dump().unwrap();
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).unwrap();

    let (_function_name, args, _kwargs, _call_id, state) = loaded.into_function_call().expect("expected function call");
    assert_eq!(args, vec![MontyObject::Int(20)]);

    let progress = state.run(MontyObject::Int(20), &mut PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(repl.feed_no_print("z = 1").unwrap(), MontyObject::None);
    assert_eq!(repl.feed_no_print("z").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_start_run_pending_resolve_futures_roundtrip() {
    let (repl, _) = init_repl(
        r"
async def main():
    value = await foo()
    return value + 1
",
        vec!["foo".to_owned()],
    );

    let progress = repl.start("await main()", &mut PrintWriter::Stdout).unwrap();
    let (_function_name, _args, _kwargs, call_id, state) =
        progress.into_function_call().expect("expected function call");

    let progress = state.run_pending(&mut PrintWriter::Stdout).unwrap();
    let bytes = progress.dump().unwrap();
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).unwrap();
    let state = loaded.into_resolve_futures().expect("expected resolve futures");
    assert_eq!(state.pending_call_ids(), &[call_id]);

    let progress = state
        .resume(
            vec![(call_id, ExternalResult::Return(MontyObject::Int(41)))],
            &mut PrintWriter::Stdout,
        )
        .unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(repl.feed_no_print("final_value = 42").unwrap(), MontyObject::None);
    assert_eq!(repl.feed_no_print("final_value").unwrap(), MontyObject::Int(42));
}
