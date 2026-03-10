//! Behavioural coverage for Track A compatibility invariants.

use monty::{
    MontyObject, MontyRepl, MontyRun, NoLimitTracker, NoopRuntimeObserver, PrintWriter, ReplProgress, RunInputs,
    RunProgress, RuntimeObserverHandle,
};
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};

#[derive(Debug, Clone, Copy)]
enum ObserverMode {
    DisabledHandle,
    NoopObserver,
}

impl ObserverMode {
    fn handle(self) -> RuntimeObserverHandle {
        match self {
            Self::DisabledHandle => RuntimeObserverHandle::disabled(),
            Self::NoopObserver => RuntimeObserverHandle::new(NoopRuntimeObserver),
        }
    }
}

#[derive(Default)]
struct TrackAInvariantsWorld {
    run_script: String,
    repl_init_script: String,
    repl_snippet: String,
    observer_mode: Option<ObserverMode>,
    baseline_run: Option<RunProgress<NoLimitTracker>>,
    observer_run: Option<RunProgress<NoLimitTracker>>,
    baseline_repl: Option<ReplProgress<NoLimitTracker>>,
    observer_repl: Option<ReplProgress<NoLimitTracker>>,
}

#[fixture]
fn world() -> TrackAInvariantsWorld {
    TrackAInvariantsWorld::default()
}

fn init_repl(code: &str) -> MontyRepl<NoLimitTracker> {
    let (repl, value) = MontyRepl::new(
        code.to_owned(),
        "track_a_bdd_repl.py",
        vec![],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Disabled,
    )
    .expect("repl creation should succeed");
    assert_eq!(value, MontyObject::None);
    repl
}

#[given("a run script that suspends at one external call")]
fn given_suspendable_run_script(world: &mut TrackAInvariantsWorld) {
    "print(ext_fn(1))".clone_into(&mut world.run_script);
}

#[given("the disabled observer-aware mode")]
fn given_disabled_mode(world: &mut TrackAInvariantsWorld) {
    world.observer_mode = Some(ObserverMode::DisabledHandle);
}

#[given("the no-op observer-aware mode")]
fn given_noop_mode(world: &mut TrackAInvariantsWorld) {
    world.observer_mode = Some(ObserverMode::NoopObserver);
}

#[given("a REPL snippet that completes without suspension")]
fn given_repl_completion_snippet(world: &mut TrackAInvariantsWorld) {
    "seed = 10".clone_into(&mut world.repl_init_script);
    "seed = seed + 1\nseed".clone_into(&mut world.repl_snippet);
}

#[given("a REPL snippet that suspends and survives dump and load")]
fn given_repl_snapshot_snippet(world: &mut TrackAInvariantsWorld) {
    "seed = 10".clone_into(&mut world.repl_init_script);
    "print(ext_fn(seed + 1))".clone_into(&mut world.repl_snippet);
}

#[when("baseline and observer-aware run execution both start")]
fn when_run_execution_starts(world: &mut TrackAInvariantsWorld) {
    let run =
        MontyRun::new(world.run_script.clone(), "track_a_bdd.py", vec![]).expect("runner creation should succeed");
    world.baseline_run = Some(
        run.clone()
            .start(vec![], NoLimitTracker, &mut PrintWriter::Disabled)
            .expect("baseline start should succeed"),
    );
    world.observer_run = Some(
        run.start_with_observer(
            RunInputs {
                inputs: vec![],
                resource_tracker: NoLimitTracker,
            },
            &mut PrintWriter::Disabled,
            world.observer_mode.expect("observer mode should be set").handle(),
        )
        .expect("observer-aware start should succeed"),
    );
}

#[when("baseline and observer-aware REPL execution both start")]
fn when_repl_execution_starts(world: &mut TrackAInvariantsWorld) {
    let baseline_repl = init_repl(&world.repl_init_script);
    world.baseline_repl = Some(
        baseline_repl
            .start_no_print(&world.repl_snippet)
            .expect("baseline repl start should succeed"),
    );

    let observer_repl = init_repl(&world.repl_init_script);
    world.observer_repl = Some(
        observer_repl
            .start_no_print_with_observer(
                &world.repl_snippet,
                world.observer_mode.expect("observer mode should be set").handle(),
            )
            .expect("observer-aware repl start should succeed"),
    );
}

#[when("the observer-aware REPL progress is dumped and loaded")]
fn when_observer_repl_is_dumped_and_loaded(world: &mut TrackAInvariantsWorld) {
    let baseline_progress = world.baseline_repl.take().expect("baseline repl progress should exist");
    let baseline_bytes = baseline_progress.dump().expect("baseline dump should succeed");
    world.baseline_repl = Some(ReplProgress::load(&baseline_bytes).expect("baseline load should succeed"));

    let observer_progress = world.observer_repl.take().expect("observer repl progress should exist");
    let observer_bytes = observer_progress.dump().expect("observer dump should succeed");
    world.observer_repl = Some(
        ReplProgress::load_with_observer(
            &observer_bytes,
            world.observer_mode.expect("observer mode should be set").handle(),
        )
        .expect("observer load should succeed"),
    );
}

#[then("both run modes suspend with matching external call payloads")]
fn then_run_suspensions_match(world: &TrackAInvariantsWorld) {
    let baseline = world.baseline_run.as_ref().expect("baseline run should exist");
    let observer = world.observer_run.as_ref().expect("observer run should exist");

    let RunProgress::FunctionCall(baseline_call) = baseline else {
        panic!("expected baseline function-call progress");
    };
    let RunProgress::FunctionCall(observer_call) = observer else {
        panic!("expected observer function-call progress");
    };

    assert_eq!(baseline_call.function_name, observer_call.function_name);
    assert_eq!(baseline_call.args, observer_call.args);
    assert_eq!(baseline_call.kwargs, observer_call.kwargs);
    assert_eq!(baseline_call.call_id, observer_call.call_id);
}

#[then("both REPL modes complete with the same observable result")]
fn then_repl_completions_match(world: &TrackAInvariantsWorld) {
    let baseline = world.baseline_repl.as_ref().expect("baseline repl should exist");
    let observer = world.observer_repl.as_ref().expect("observer repl should exist");

    let ReplProgress::Complete {
        repl: _baseline_repl,
        value: baseline_value,
    } = baseline
    else {
        panic!("expected baseline completion");
    };
    let ReplProgress::Complete {
        repl: _observer_repl,
        value: observer_value,
    } = observer
    else {
        panic!("expected observer completion");
    };

    assert_eq!(baseline_value, observer_value);
}

#[then("both REPL modes still suspend with matching external call payloads")]
fn then_repl_suspensions_match(world: &TrackAInvariantsWorld) {
    let baseline = world.baseline_repl.as_ref().expect("baseline repl should exist");
    let observer = world.observer_repl.as_ref().expect("observer repl should exist");

    let ReplProgress::FunctionCall(baseline_call) = baseline else {
        panic!("expected baseline function-call progress");
    };
    let ReplProgress::FunctionCall(observer_call) = observer else {
        panic!("expected observer function-call progress");
    };

    assert_eq!(baseline_call.function_name, observer_call.function_name);
    assert_eq!(baseline_call.args, observer_call.args);
    assert_eq!(baseline_call.call_id, observer_call.call_id);
}

#[scenario(
    path = "tests/features/track_a_invariants.feature",
    name = "Disabled observer-aware run execution matches baseline suspension"
)]
fn disabled_run_matches_baseline(world: TrackAInvariantsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/track_a_invariants.feature",
    name = "No-op observer-aware REPL completion matches baseline completion"
)]
fn noop_repl_completion_matches_baseline(world: TrackAInvariantsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/track_a_invariants.feature",
    name = "No-op observer-aware REPL snapshot survives dump and load like baseline"
)]
fn noop_repl_snapshot_matches_baseline(world: TrackAInvariantsWorld) {
    drop(world);
}
