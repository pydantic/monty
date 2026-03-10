//! Compatibility and overhead checks for Track A observer modes.

use std::{hint::black_box, time::Instant};

#[path = "support/test_utils.rs"]
mod test_utils;

use monty::{
    ExtFunctionResult, MontyException, MontyObject, MontyRepl, MontyRun, NoLimitTracker, NoopRuntimeObserver,
    PrintWriter, ReplProgress, ReplStartError, RunInputs, RunProgress, RuntimeObserverHandle,
};
use rstest::rstest;
use test_utils::{
    assert_exceptions_equal, assert_function_calls_equal, assert_os_calls_equal, assert_repl_function_calls_equal,
};

const FUNCTION_CALL_SCRIPT: &str = "print(ext_fn(1))";
const ERROR_SCRIPT: &str = "ext_fn(1)";
const OS_CALL_SCRIPT: &str = "from pathlib import Path\nprint(Path('/tmp/track-a').exists())";
const REPL_INIT_SCRIPT: &str = "seed = 10";
const REPL_COMPLETE_SNIPPET: &str = "seed = seed + 1\nseed";
const REPL_SNAPSHOT_SNIPPET: &str = "print(ext_fn(seed + 1))";
const BENCHMARK_SCRIPT: &str = r"
total = 0
for i in range(2_000):
    if i % 3 == 0:
        total = total + i
    else:
        total = total - 1
total
";
const SNAPSHOT_EXTENSION_BYTES: &[u8] = &[1, 3, 5, 7];
const BENCHMARK_WARMUP_RUNS: usize = 5;
const BENCHMARK_SAMPLES: usize = 11;

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

#[derive(Debug, Clone, Copy)]
enum BenchmarkMode {
    Baseline,
    Observer(ObserverMode),
}

fn build_run(script: &str) -> MontyRun {
    MontyRun::new(script.to_owned(), "track_a.py", vec![]).expect("runner creation should succeed")
}

fn init_repl(code: &str) -> MontyRepl<NoLimitTracker> {
    let (repl, value) = MontyRepl::new(
        code.to_owned(),
        "track_a_repl.py",
        vec![],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Disabled,
    )
    .expect("repl creation should succeed");
    assert_eq!(value, MontyObject::None, "init script should not produce a value");
    repl
}

fn start_run_with_mode(
    run: &MontyRun,
    mode: BenchmarkMode,
    print: &mut PrintWriter<'_>,
) -> RunProgress<NoLimitTracker> {
    match mode {
        BenchmarkMode::Baseline => run
            .clone()
            .start(vec![], NoLimitTracker, print)
            .expect("baseline start should succeed"),
        BenchmarkMode::Observer(observer_mode) => run
            .clone()
            .start_with_observer(
                RunInputs {
                    inputs: vec![],
                    resource_tracker: NoLimitTracker,
                },
                print,
                observer_mode.handle(),
            )
            .expect("observer-aware start should succeed"),
    }
}

fn start_repl_with_mode(
    repl: MontyRepl<NoLimitTracker>,
    snippet: &str,
    mode: ObserverMode,
) -> Result<ReplProgress<NoLimitTracker>, Box<ReplStartError<NoLimitTracker>>> {
    match mode {
        ObserverMode::DisabledHandle => repl.start_no_print_with_observer(snippet, RuntimeObserverHandle::disabled()),
        ObserverMode::NoopObserver => {
            repl.start_no_print_with_observer(snippet, RuntimeObserverHandle::new(NoopRuntimeObserver))
        }
    }
}

fn assert_complete_progress(progress: RunProgress<NoLimitTracker>, expected: &MontyObject) {
    let Some(value) = progress.into_complete() else {
        panic!("expected RunProgress::Complete");
    };
    assert_eq!(&value, expected);
}

fn assert_repl_complete_progress(
    progress: ReplProgress<NoLimitTracker>,
    expected_value: &MontyObject,
    follow_up: &str,
    expected_follow_up: &MontyObject,
) {
    let Some((mut repl, value)) = progress.into_complete() else {
        panic!("expected ReplProgress::Complete");
    };
    assert_eq!(&value, expected_value);
    assert_eq!(
        repl.feed_no_print(follow_up).expect("follow-up snippet should succeed"),
        *expected_follow_up
    );
}

fn take_output(writer: &PrintWriter<'_>) -> String {
    writer
        .collected_output()
        .map(str::to_owned)
        .expect("writer should collect output")
}

fn median_ns(mode: BenchmarkMode) -> u128 {
    let run = build_run(BENCHMARK_SCRIPT);
    let mut durations = Vec::with_capacity(BENCHMARK_SAMPLES);

    for _ in 0..BENCHMARK_WARMUP_RUNS {
        let _ = black_box(run_benchmark_iteration(&run, mode));
    }

    for _ in 0..BENCHMARK_SAMPLES {
        let start = Instant::now();
        let result = run_benchmark_iteration(&run, mode);
        let elapsed = start.elapsed().as_nanos();
        assert_eq!(result, MontyObject::Int(665_000));
        durations.push(elapsed);
    }

    durations.sort_unstable();
    durations[durations.len() / 2]
}

fn run_benchmark_iteration(run: &MontyRun, mode: BenchmarkMode) -> MontyObject {
    match mode {
        BenchmarkMode::Baseline => run
            .run(vec![], NoLimitTracker, &mut PrintWriter::Disabled)
            .expect("baseline benchmark run should succeed"),
        BenchmarkMode::Observer(observer_mode) => {
            let progress = start_run_with_mode(run, BenchmarkMode::Observer(observer_mode), &mut PrintWriter::Disabled);
            let Some(value) = progress.into_complete() else {
                panic!("benchmark script should complete without suspension");
            };
            value
        }
    }
}

#[rstest]
#[case(ObserverMode::DisabledHandle)]
#[case(ObserverMode::NoopObserver)]
fn run_observer_modes_match_baseline_function_call_and_completion(#[case] mode: ObserverMode) {
    let run = build_run(FUNCTION_CALL_SCRIPT);
    let mut baseline_print = PrintWriter::Collect(String::new());
    let baseline_progress = start_run_with_mode(&run, BenchmarkMode::Baseline, &mut baseline_print);
    let baseline_call = baseline_progress
        .into_function_call()
        .expect("baseline should suspend at function call");

    let mut observer_print = PrintWriter::Collect(String::new());
    let observer_progress = start_run_with_mode(&run, BenchmarkMode::Observer(mode), &mut observer_print);
    let observer_call = observer_progress
        .into_function_call()
        .expect("observer-aware mode should suspend at function call");

    assert_function_calls_equal(&baseline_call, &observer_call);

    let baseline_resume = baseline_call
        .resume(MontyObject::Int(7), &mut baseline_print)
        .expect("baseline resume should succeed");
    let observer_resume = observer_call
        .resume(MontyObject::Int(7), &mut observer_print)
        .expect("observer-aware resume should succeed");

    assert_complete_progress(baseline_resume, &MontyObject::None);
    assert_complete_progress(observer_resume, &MontyObject::None);
    assert_eq!(take_output(&baseline_print), take_output(&observer_print));
}

#[rstest]
#[case(ObserverMode::DisabledHandle)]
#[case(ObserverMode::NoopObserver)]
fn run_observer_modes_match_baseline_error_path(#[case] mode: ObserverMode) {
    let run = build_run(ERROR_SCRIPT);
    let baseline_progress = start_run_with_mode(&run, BenchmarkMode::Baseline, &mut PrintWriter::Disabled);
    let baseline_call = baseline_progress
        .into_function_call()
        .expect("baseline should suspend at function call");

    let observer_progress = start_run_with_mode(&run, BenchmarkMode::Observer(mode), &mut PrintWriter::Disabled);
    let observer_call = observer_progress
        .into_function_call()
        .expect("observer-aware mode should suspend at function call");

    assert_function_calls_equal(&baseline_call, &observer_call);

    let exception = MontyException::new(monty::ExcType::RuntimeError, Some("track-a failure".to_owned()));
    let baseline_error = baseline_call
        .resume(ExtFunctionResult::Error(exception.clone()), &mut PrintWriter::Disabled)
        .expect_err("baseline resume should error");
    let observer_error = observer_call
        .resume(ExtFunctionResult::Error(exception), &mut PrintWriter::Disabled)
        .expect_err("observer-aware resume should error");

    assert_exceptions_equal(&baseline_error, &observer_error);
}

#[rstest]
#[case(ObserverMode::DisabledHandle)]
#[case(ObserverMode::NoopObserver)]
fn run_observer_modes_match_baseline_os_call_path(#[case] mode: ObserverMode) {
    let run = build_run(OS_CALL_SCRIPT);
    let mut baseline_print = PrintWriter::Collect(String::new());
    let baseline_progress = start_run_with_mode(&run, BenchmarkMode::Baseline, &mut baseline_print);
    let baseline_call = baseline_progress
        .into_os_call()
        .expect("baseline should suspend at OS call");

    let mut observer_print = PrintWriter::Collect(String::new());
    let observer_progress = start_run_with_mode(&run, BenchmarkMode::Observer(mode), &mut observer_print);
    let observer_call = observer_progress
        .into_os_call()
        .expect("observer-aware mode should suspend at OS call");

    assert_os_calls_equal(&baseline_call, &observer_call);

    let baseline_resume = baseline_call
        .resume(MontyObject::Bool(false), &mut baseline_print)
        .expect("baseline resume should succeed");
    let observer_resume = observer_call
        .resume(MontyObject::Bool(false), &mut observer_print)
        .expect("observer-aware resume should succeed");

    assert_complete_progress(baseline_resume, &MontyObject::None);
    assert_complete_progress(observer_resume, &MontyObject::None);
    assert_eq!(take_output(&baseline_print), take_output(&observer_print));
}

#[rstest]
#[case(ObserverMode::DisabledHandle)]
#[case(ObserverMode::NoopObserver)]
fn repl_observer_modes_match_baseline_completion(#[case] mode: ObserverMode) {
    let baseline_repl = init_repl(REPL_INIT_SCRIPT);
    let baseline_progress = baseline_repl
        .start_no_print(REPL_COMPLETE_SNIPPET)
        .expect("baseline REPL start should succeed");

    let observer_repl = init_repl(REPL_INIT_SCRIPT);
    let observer_progress = start_repl_with_mode(observer_repl, REPL_COMPLETE_SNIPPET, mode)
        .expect("observer-aware REPL start should succeed");

    assert_repl_complete_progress(
        baseline_progress,
        &MontyObject::Int(11),
        "seed + 1",
        &MontyObject::Int(12),
    );
    assert_repl_complete_progress(
        observer_progress,
        &MontyObject::Int(11),
        "seed + 1",
        &MontyObject::Int(12),
    );
}

#[rstest]
#[case(ObserverMode::DisabledHandle)]
#[case(ObserverMode::NoopObserver)]
fn repl_snapshot_round_trip_matches_baseline(#[case] mode: ObserverMode) {
    let baseline_repl = init_repl(REPL_INIT_SCRIPT);
    let mut baseline_print = PrintWriter::Collect(String::new());
    let baseline_progress = baseline_repl
        .start(REPL_SNAPSHOT_SNIPPET, &mut baseline_print)
        .expect("baseline REPL should suspend at function call");
    let baseline_call = baseline_progress
        .into_function_call()
        .expect("baseline should suspend at function call");

    let observer_repl = init_repl(REPL_INIT_SCRIPT);
    let mut observer_print = PrintWriter::Collect(String::new());
    let observer_progress = match mode {
        ObserverMode::DisabledHandle => observer_repl.start_with_observer(
            REPL_SNAPSHOT_SNIPPET,
            &mut observer_print,
            RuntimeObserverHandle::disabled(),
        ),
        ObserverMode::NoopObserver => observer_repl.start_with_observer(
            REPL_SNAPSHOT_SNIPPET,
            &mut observer_print,
            RuntimeObserverHandle::new(NoopRuntimeObserver),
        ),
    }
    .expect("observer-aware REPL should suspend at function call");
    let observer_call = observer_progress
        .into_function_call()
        .expect("observer-aware REPL should suspend at function call");

    assert_repl_function_calls_equal(&baseline_call, &observer_call);

    let baseline_bytes =
        ReplProgress::FunctionCall(baseline_call.with_snapshot_extension(SNAPSHOT_EXTENSION_BYTES.to_vec()))
            .dump()
            .expect("baseline progress should dump");
    let baseline_loaded = ReplProgress::<NoLimitTracker>::load(&baseline_bytes).expect("baseline load should succeed");
    let baseline_loaded_call = baseline_loaded
        .into_function_call()
        .expect("loaded baseline should stay suspended");
    assert_eq!(
        baseline_loaded_call
            .snapshot_extension()
            .map(monty::SnapshotExtension::as_slice),
        Some(SNAPSHOT_EXTENSION_BYTES)
    );

    let observer_bytes =
        ReplProgress::FunctionCall(observer_call.with_snapshot_extension(SNAPSHOT_EXTENSION_BYTES.to_vec()))
            .dump()
            .expect("observer-aware progress should dump");
    let observer_loaded = ReplProgress::<NoLimitTracker>::load_with_observer(&observer_bytes, mode.handle())
        .expect("observer load should succeed");
    let observer_loaded_call = observer_loaded
        .into_function_call()
        .expect("loaded observer-aware progress should stay suspended");
    assert_eq!(
        observer_loaded_call
            .snapshot_extension()
            .map(monty::SnapshotExtension::as_slice),
        Some(SNAPSHOT_EXTENSION_BYTES)
    );

    let baseline_resume = baseline_loaded_call
        .resume(MontyObject::Int(11), &mut baseline_print)
        .expect("baseline resume should succeed");
    let observer_resume = observer_loaded_call
        .resume(MontyObject::Int(11), &mut observer_print)
        .expect("observer-aware resume should succeed");

    assert_repl_complete_progress(baseline_resume, &MontyObject::None, "seed", &MontyObject::Int(10));
    assert_repl_complete_progress(observer_resume, &MontyObject::None, "seed", &MontyObject::Int(10));
    assert_eq!(take_output(&baseline_print), take_output(&observer_print));
}

#[test]
fn track_a_overhead_disabled_within_budget() {
    let baseline = median_ns(BenchmarkMode::Baseline);
    let disabled = median_ns(BenchmarkMode::Observer(ObserverMode::DisabledHandle));
    println!("track_a_overhead disabled baseline_ns={baseline} observed_ns={disabled}");
    assert!(disabled * 100 <= baseline * 120);
}

#[test]
fn track_a_overhead_noop_within_budget() {
    let baseline = median_ns(BenchmarkMode::Baseline);
    let noop = median_ns(BenchmarkMode::Observer(ObserverMode::NoopObserver));
    println!("track_a_overhead noop baseline_ns={baseline} observed_ns={noop}");
    assert!(noop * 100 <= baseline * 215);
}
