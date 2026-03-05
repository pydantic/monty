//! Behavioural coverage for stable host-facing runtime IDs.

use monty::{MontyObject, MontyRun, NoLimitTracker, OsCall, PrintWriter, RunProgress};
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};

#[derive(Default)]
struct RuntimeIdsWorld {
    script: String,
    first_runtime_ids: Vec<usize>,
    first_kwarg_runtime_ids: Vec<(usize, usize)>,
    first_kwargs: Vec<(MontyObject, MontyObject)>,
    second_runtime_ids: Vec<usize>,
    second_kwarg_runtime_ids: Vec<(usize, usize)>,
    second_kwargs: Vec<(MontyObject, MontyObject)>,
    load_failed: bool,
}

#[fixture]
fn world() -> RuntimeIdsWorld {
    RuntimeIdsWorld::default()
}

#[given("a suspendable script with repeated object external calls")]
fn given_repeated_object_calls(world: &mut RuntimeIdsWorld) {
    "x = []; ext_fn(x); ext_fn(x)".clone_into(&mut world.script);
}

#[given("a suspendable script with one external call")]
fn given_one_external_call(world: &mut RuntimeIdsWorld) {
    "ext_fn([])".clone_into(&mut world.script);
}

#[given("a suspendable script with one keyword-only external call")]
fn given_one_kwarg_external_call(world: &mut RuntimeIdsWorld) {
    "ext_fn(a=1, b=2)".clone_into(&mut world.script);
}

#[given("a suspendable script with one OS call")]
fn given_one_os_call(world: &mut RuntimeIdsWorld) {
    "from pathlib import Path\nPath('/tmp/runtime-id').exists()".clone_into(&mut world.script);
}

fn complete_single_call_progress(progress: RunProgress<NoLimitTracker>) {
    let completed = match progress {
        RunProgress::FunctionCall(call) => call
            .resume(MontyObject::None, &mut PrintWriter::Stdout)
            .expect("function call resume should succeed"),
        RunProgress::OsCall(call) => resume_os_call(call),
        other => panic!("expected call progress to complete, got {other:?}"),
    };
    assert!(matches!(completed, RunProgress::Complete(_)));
}

fn resume_os_call(call: OsCall<NoLimitTracker>) -> RunProgress<NoLimitTracker> {
    call.resume(MontyObject::Bool(false), &mut PrintWriter::Stdout)
        .expect("OS call resume should succeed")
}

fn extract_kwargs(progress: &RunProgress<NoLimitTracker>) -> Vec<(MontyObject, MontyObject)> {
    match progress {
        RunProgress::FunctionCall(call) => call.kwargs.clone(),
        RunProgress::OsCall(call) => call.kwargs.clone(),
        other => panic!("expected call progress, got {other:?}"),
    }
}

#[when("execution pauses and resumes through both external calls")]
fn when_pause_and_resume_twice(world: &mut RuntimeIdsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![]).expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("first run should pause at external call");

    let first_call = match progress {
        RunProgress::FunctionCall(call) => call,
        other => panic!("expected first function call pause, got {other:?}"),
    };
    world.first_runtime_ids = first_call.arg_runtime_ids.iter().map(|id| id.raw()).collect();

    let progress = first_call
        .resume(MontyObject::None, &mut PrintWriter::Stdout)
        .expect("resume should reach second function call");

    let second_call = match progress {
        RunProgress::FunctionCall(call) => call,
        other => panic!("expected second function call pause, got {other:?}"),
    };
    world.second_runtime_ids = second_call.arg_runtime_ids.iter().map(|id| id.raw()).collect();

    let completion = second_call
        .resume(MontyObject::None, &mut PrintWriter::Stdout)
        .expect("final resume should complete");
    assert!(matches!(completion, RunProgress::Complete(_)));
}

#[when("run progress is dumped and loaded")]
fn when_progress_is_dumped_and_loaded(world: &mut RuntimeIdsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![]).expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should pause at call");

    let runtime_ids = progress
        .runtime_ids()
        .expect("call progress should provide runtime ids");
    world.first_runtime_ids = runtime_ids.0.iter().map(|id| id.raw()).collect();
    world.first_kwarg_runtime_ids = runtime_ids
        .1
        .iter()
        .map(|(key_id, value_id)| (key_id.raw(), value_id.raw()))
        .collect();
    world.first_kwargs = extract_kwargs(&progress);

    let bytes = progress.dump().expect("run progress dump should succeed");
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).expect("run progress load should succeed");

    let loaded_runtime_ids = loaded
        .runtime_ids()
        .expect("loaded call progress should provide runtime ids");
    world.second_runtime_ids = loaded_runtime_ids.0.iter().map(|id| id.raw()).collect();
    world.second_kwarg_runtime_ids = loaded_runtime_ids
        .1
        .iter()
        .map(|(key_id, value_id)| (key_id.raw(), value_id.raw()))
        .collect();
    world.second_kwargs = extract_kwargs(&loaded);

    complete_single_call_progress(progress);
    complete_single_call_progress(loaded);
}

#[when("the serialized run progress payload is corrupted before load")]
fn when_progress_payload_is_corrupted(world: &mut RuntimeIdsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![]).expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should pause at external call");

    let mut bytes = progress.dump().expect("run progress dump should succeed");
    assert!(!bytes.is_empty(), "serialized run progress should not be empty");
    bytes[0] ^= 0xFF;

    complete_single_call_progress(progress);

    world.load_failed = RunProgress::<NoLimitTracker>::load(&bytes).is_err();
}

#[then("the first and second runtime ID vectors are equal")]
fn then_runtime_ids_match(world: &RuntimeIdsWorld) {
    assert_eq!(world.first_runtime_ids, world.second_runtime_ids);
}

#[then("the runtime ID vectors are non-empty")]
fn then_runtime_ids_non_empty(world: &RuntimeIdsWorld) {
    assert!(!world.first_runtime_ids.is_empty());
    assert!(!world.second_runtime_ids.is_empty());
}

#[then("the first and second kwarg runtime ID vectors are equal")]
fn then_kwarg_runtime_ids_match(world: &RuntimeIdsWorld) {
    assert_eq!(world.first_kwarg_runtime_ids, world.second_kwarg_runtime_ids);
}

#[then("the kwarg runtime ID vectors are non-empty")]
fn then_kwarg_runtime_ids_non_empty(world: &RuntimeIdsWorld) {
    assert!(!world.first_kwarg_runtime_ids.is_empty());
    assert!(!world.second_kwarg_runtime_ids.is_empty());
}

#[then("kwarg payload and runtime ID pairing remain aligned")]
fn then_kwarg_pairing_remains_aligned(world: &RuntimeIdsWorld) {
    assert_eq!(world.first_kwargs, world.second_kwargs);
    assert_eq!(world.first_kwargs.len(), world.first_kwarg_runtime_ids.len());
    assert_eq!(world.second_kwargs.len(), world.second_kwarg_runtime_ids.len());
}

#[then("loading the progress payload fails")]
fn then_payload_load_fails(world: &RuntimeIdsWorld) {
    assert!(world.load_failed, "expected corrupted payload load to fail");
}

#[scenario(
    path = "tests/features/runtime_ids.feature",
    name = "Runtime IDs remain stable across resume for persistent objects"
)]
fn runtime_ids_are_stable_across_resume(world: RuntimeIdsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/runtime_ids.feature",
    name = "Runtime IDs survive run progress dump and load"
)]
fn runtime_ids_survive_dump_and_load(world: RuntimeIdsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/runtime_ids.feature",
    name = "Keyword runtime IDs survive run progress dump and load"
)]
fn kwarg_runtime_ids_survive_dump_and_load(world: RuntimeIdsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/runtime_ids.feature",
    name = "OS-call runtime IDs survive run progress dump and load"
)]
fn os_call_runtime_ids_survive_dump_and_load(world: RuntimeIdsWorld) {
    drop(world);
}

#[scenario(
    path = "tests/features/runtime_ids.feature",
    name = "Corrupted run progress payload fails load"
)]
fn corrupted_payload_fails_load(world: RuntimeIdsWorld) {
    drop(world);
}
