//! Tests for snapshot extension byte round-trips.

use monty::{ExternalResult, MontyObject, MontyRepl, MontyRun, NoLimitTracker, PrintWriter, ReplProgress, RunProgress};

fn create_function_call_progress(script: &str) -> RunProgress<NoLimitTracker> {
    let runner = MontyRun::new(
        script.to_owned(),
        "test.py",
        vec![],
        vec!["ext_fn".to_owned(), "foo".to_owned()],
    )
    .expect("runner creation should succeed");
    runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should suspend")
}

fn create_repl() -> MontyRepl<NoLimitTracker> {
    let (repl, _result) = MontyRepl::new(
        "pass".to_owned(),
        "init.py",
        vec![],
        vec!["ext_fn".to_owned(), "foo".to_owned()],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    )
    .expect("repl creation should succeed");
    repl
}

fn attach_run_snapshot_extension(
    progress: RunProgress<NoLimitTracker>,
    snapshot_extension: Vec<u8>,
) -> RunProgress<NoLimitTracker> {
    match progress {
        RunProgress::FunctionCall {
            function_name,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            method_call,
            state,
        } => RunProgress::FunctionCall {
            function_name,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            method_call,
            state: state.with_snapshot_extension(snapshot_extension),
        },
        RunProgress::OsCall {
            function,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            state,
        } => RunProgress::OsCall {
            function,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            state: state.with_snapshot_extension(snapshot_extension),
        },
        RunProgress::ResolveFutures(state) => {
            RunProgress::ResolveFutures(state.with_snapshot_extension(snapshot_extension))
        }
        RunProgress::Complete(value) => RunProgress::Complete(value),
    }
}

fn attach_repl_snapshot_extension(
    progress: ReplProgress<NoLimitTracker>,
    snapshot_extension: Vec<u8>,
) -> ReplProgress<NoLimitTracker> {
    match progress {
        ReplProgress::FunctionCall {
            function_name,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            method_call,
            state,
        } => ReplProgress::FunctionCall {
            function_name,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            method_call,
            state: state.with_snapshot_extension(snapshot_extension),
        },
        ReplProgress::OsCall {
            function,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            state,
        } => ReplProgress::OsCall {
            function,
            args,
            arg_runtime_ids,
            kwargs,
            kwarg_runtime_ids,
            call_id,
            state: state.with_snapshot_extension(snapshot_extension),
        },
        ReplProgress::ResolveFutures(state) => {
            ReplProgress::ResolveFutures(state.with_snapshot_extension(snapshot_extension))
        }
        ReplProgress::Complete { repl, value } => ReplProgress::Complete { repl, value },
    }
}

fn run_progress_snapshot_extension(progress: &RunProgress<NoLimitTracker>) -> Option<&[u8]> {
    match progress {
        RunProgress::FunctionCall { state, .. } | RunProgress::OsCall { state, .. } => state.snapshot_extension(),
        RunProgress::ResolveFutures(state) => state.snapshot_extension(),
        RunProgress::Complete(_) => None,
    }
}

fn repl_progress_snapshot_extension(progress: &ReplProgress<NoLimitTracker>) -> Option<&[u8]> {
    match progress {
        ReplProgress::FunctionCall { state, .. } | ReplProgress::OsCall { state, .. } => state.snapshot_extension(),
        ReplProgress::ResolveFutures(state) => state.snapshot_extension(),
        ReplProgress::Complete { .. } => None,
    }
}

fn drive_to_resolve_futures(mut progress: RunProgress<NoLimitTracker>) -> RunProgress<NoLimitTracker> {
    loop {
        match progress {
            RunProgress::FunctionCall { state, .. } => {
                progress = state
                    .run_pending(&mut PrintWriter::Stdout)
                    .expect("run_pending should succeed");
            }
            RunProgress::ResolveFutures(_) => return progress,
            RunProgress::OsCall { function, .. } => {
                panic!("unexpected OsCall: {function:?}");
            }
            RunProgress::Complete(_) => panic!("unexpected Complete before ResolveFutures"),
        }
    }
}

fn drive_repl_to_resolve_futures(mut progress: ReplProgress<NoLimitTracker>) -> ReplProgress<NoLimitTracker> {
    loop {
        match progress {
            ReplProgress::FunctionCall { state, .. } => {
                progress = state
                    .run_pending(&mut PrintWriter::Stdout)
                    .expect("run_pending should succeed");
            }
            ReplProgress::ResolveFutures(_) => return progress,
            ReplProgress::OsCall { function, .. } => {
                panic!("unexpected OsCall: {function:?}");
            }
            ReplProgress::Complete { .. } => panic!("unexpected Complete before ResolveFutures"),
        }
    }
}

fn complete_resolve_futures(progress: RunProgress<NoLimitTracker>) -> RunProgress<NoLimitTracker> {
    let RunProgress::ResolveFutures(state) = progress else {
        panic!("expected resolve futures progress");
    };
    let results = state
        .pending_call_ids()
        .iter()
        .map(|call_id| (*call_id, ExternalResult::Return(MontyObject::Int(1))))
        .collect();
    state
        .resume(results, &mut PrintWriter::Stdout)
        .expect("resume should succeed")
}

fn complete_repl_resolve_futures(progress: ReplProgress<NoLimitTracker>) -> ReplProgress<NoLimitTracker> {
    let ReplProgress::ResolveFutures(state) = progress else {
        panic!("expected resolve futures progress");
    };
    let results = state
        .pending_call_ids()
        .iter()
        .map(|call_id| (*call_id, ExternalResult::Return(MontyObject::Int(1))))
        .collect();
    state
        .resume(results, &mut PrintWriter::Stdout)
        .expect("resume should succeed")
}

#[test]
fn run_progress_snapshot_extension_round_trips() {
    let progress = create_function_call_progress("ext_fn([])");
    let snapshot_extension = vec![1, 2, 3, 4];
    let progress = attach_run_snapshot_extension(progress, snapshot_extension.clone());

    let bytes = progress.dump().expect("run progress dump should succeed");
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");

    let loaded_extension = run_progress_snapshot_extension(&loaded).expect("expected snapshot extension");
    assert_eq!(loaded_extension, snapshot_extension.as_slice());
}

#[test]
fn future_snapshot_extension_round_trips() {
    let code = r"
import asyncio

async def main():
    return await foo()

await main()
";
    let progress = create_function_call_progress(code);
    let progress = drive_to_resolve_futures(progress);
    let snapshot_extension = vec![9, 8, 7];
    let progress = attach_run_snapshot_extension(progress, snapshot_extension.clone());

    let bytes = progress.dump().expect("run progress dump should succeed");
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");

    let loaded_extension = run_progress_snapshot_extension(&loaded).expect("expected snapshot extension");
    assert_eq!(loaded_extension, snapshot_extension.as_slice());

    let completed = complete_resolve_futures(progress);
    assert!(matches!(completed, RunProgress::Complete(_)));

    let completed_loaded = complete_resolve_futures(loaded);
    assert!(matches!(completed_loaded, RunProgress::Complete(_)));
}

#[test]
fn run_progress_snapshot_extension_defaults_to_none() {
    let progress = create_function_call_progress("ext_fn([])");
    let bytes = progress.dump().expect("run progress dump should succeed");
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");

    assert!(
        run_progress_snapshot_extension(&loaded).is_none(),
        "expected no snapshot extension by default"
    );
}

#[test]
fn repl_progress_snapshot_extension_round_trips() {
    let repl = create_repl();
    let progress = repl
        .start("ext_fn([])", &mut PrintWriter::Stdout)
        .expect("repl should suspend");
    let snapshot_extension = vec![5, 6, 7, 8];
    let progress = attach_repl_snapshot_extension(progress, snapshot_extension.clone());

    let bytes = progress.dump().expect("repl progress dump should succeed");
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).expect("repl progress load should succeed");

    let loaded_extension = repl_progress_snapshot_extension(&loaded).expect("expected snapshot extension");
    assert_eq!(loaded_extension, snapshot_extension.as_slice());
}

#[test]
fn repl_future_snapshot_extension_round_trips() {
    let repl = create_repl();
    let code = r"
import asyncio

async def main():
    return await foo()

await main()
";
    let progress = repl.start(code, &mut PrintWriter::Stdout).expect("repl should suspend");
    let progress = drive_repl_to_resolve_futures(progress);
    let snapshot_extension = vec![11, 12, 13];
    let progress = attach_repl_snapshot_extension(progress, snapshot_extension.clone());

    let bytes = progress.dump().expect("repl progress dump should succeed");
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).expect("repl progress load should succeed");

    let loaded_extension = repl_progress_snapshot_extension(&loaded).expect("expected snapshot extension");
    assert_eq!(loaded_extension, snapshot_extension.as_slice());

    let completed = complete_repl_resolve_futures(progress);
    assert!(matches!(completed, ReplProgress::Complete { .. }));

    let completed_loaded = complete_repl_resolve_futures(loaded);
    assert!(matches!(completed_loaded, ReplProgress::Complete { .. }));
}

#[test]
fn corrupted_run_progress_payload_fails_to_load() {
    let progress = create_function_call_progress("ext_fn([])");
    let progress = attach_run_snapshot_extension(progress, vec![1, 2]);
    let mut bytes = progress.dump().expect("run progress dump should succeed");

    bytes.pop();

    assert!(RunProgress::<NoLimitTracker>::load(&bytes).is_err());
}

#[test]
fn repl_future_snapshot_resume_ignores_extension_bytes() {
    let repl = create_repl();
    let code = r"
import asyncio

async def main():
    return await foo()

await main()
";
    let progress = repl.start(code, &mut PrintWriter::Stdout).expect("repl should suspend");
    let progress = drive_repl_to_resolve_futures(progress);
    let progress = attach_repl_snapshot_extension(progress, vec![99]);

    let ReplProgress::ResolveFutures(state) = progress else {
        panic!("expected resolve futures progress");
    };

    let results = vec![(state.pending_call_ids()[0], ExternalResult::Return(MontyObject::Int(3)))];
    let progress = state
        .resume(results, &mut PrintWriter::Stdout)
        .expect("resume should succeed");

    let ReplProgress::Complete { value, .. } = progress else {
        panic!("expected completion after resume");
    };
    assert_eq!(value, MontyObject::Int(3));
}
