//! Behavioural coverage for snapshot extension byte persistence.

use monty::{MontyRepl, MontyRun, NoLimitTracker, PrintWriter, ReplProgress, RunProgress};
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};

#[derive(Default)]
struct SnapshotExtensionsWorld {
    script: String,
    repl_snippet: String,
    snapshot_extension: Vec<u8>,
    loaded_snapshot_extension: Option<Vec<u8>>,
    load_failed: bool,
}

#[fixture]
fn world() -> SnapshotExtensionsWorld {
    SnapshotExtensionsWorld::default()
}

#[given("a suspendable script with one external call")]
fn given_suspendable_script(world: &mut SnapshotExtensionsWorld) {
    "ext_fn([])".clone_into(&mut world.script);
}

#[given("a REPL snippet with one external call")]
fn given_repl_snippet(world: &mut SnapshotExtensionsWorld) {
    "ext_fn([])".clone_into(&mut world.repl_snippet);
}

#[given("snapshot extension bytes")]
fn given_snapshot_extension_bytes(world: &mut SnapshotExtensionsWorld) {
    world.snapshot_extension = vec![1, 3, 5, 7];
}

#[when("run progress is dumped and loaded with snapshot extension bytes")]
fn when_run_progress_dumped_and_loaded(world: &mut SnapshotExtensionsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![], vec!["ext_fn".to_owned()])
        .expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should suspend");
    let progress = attach_run_snapshot_extension(progress, world.snapshot_extension.clone());

    let bytes = progress.dump().expect("run progress dump should succeed");
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");
    world.loaded_snapshot_extension = run_progress_snapshot_extension(&loaded).map(<[u8]>::to_vec);
}

#[when("run progress payload is corrupted")]
fn when_run_progress_payload_corrupted(world: &mut SnapshotExtensionsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![], vec!["ext_fn".to_owned()])
        .expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should suspend");

    let mut bytes = progress.dump().expect("run progress dump should succeed");
    bytes.pop();

    world.load_failed = RunProgress::<NoLimitTracker>::load(&bytes).is_err();
}

#[when("REPL progress is dumped and loaded with snapshot extension bytes")]
fn when_repl_progress_dumped_and_loaded(world: &mut SnapshotExtensionsWorld) {
    let (repl, _result) = MontyRepl::new(
        "pass".to_owned(),
        "init.py",
        vec![],
        vec!["ext_fn".to_owned()],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    )
    .expect("repl creation should succeed");

    let progress = repl
        .start(&world.repl_snippet, &mut PrintWriter::Stdout)
        .expect("repl should suspend");
    let progress = attach_repl_snapshot_extension(progress, world.snapshot_extension.clone());

    let bytes = progress.dump().expect("repl progress dump should succeed");
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).expect("repl progress load should succeed");

    world.loaded_snapshot_extension = repl_progress_snapshot_extension(&loaded).map(<[u8]>::to_vec);
}

#[then("the loaded snapshot extension bytes match")]
fn then_loaded_snapshot_extension_matches(world: &SnapshotExtensionsWorld) {
    assert_eq!(
        world.loaded_snapshot_extension.as_deref(),
        Some(world.snapshot_extension.as_slice()),
        "expected snapshot extension bytes to round-trip"
    );
}

#[then("loading the run progress fails")]
fn then_loading_run_progress_fails(world: &SnapshotExtensionsWorld) {
    assert!(world.load_failed, "expected corrupted payload to fail load");
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

#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "Run progress preserves snapshot extension bytes across dump/load"
)]
fn run_snapshot_extension_round_trip(world: SnapshotExtensionsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "Corrupted run progress payload fails to load"
)]
fn corrupted_run_progress_payload(world: SnapshotExtensionsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "REPL progress preserves snapshot extension bytes across dump/load"
)]
fn repl_snapshot_extension_round_trip(world: SnapshotExtensionsWorld) {
    drop(world);
}
