//! Integration tests for generic runtime observer events.

use std::sync::{Arc, Mutex};

use monty::{
    ExcType, ExternalCallKind, ExternalCallReturnKind, MontyException, MontyFuture, MontyObject, MontyRepl, MontyRun,
    NoLimitTracker, NoopRuntimeObserver, OpInputIds, PrintWriter, ResourceTracker, RunProgress, RuntimeObserver,
    RuntimeObserverEvent, RuntimeObserverHandle, Snapshot,
};
use rstest::{fixture, rstest};

/// Captured observer events in test-friendly form.
///
/// Each variant stores raw runtime IDs (`RuntimeValueId::raw()`) so assertions can compare
/// stable primitive values without reconstructing runtime wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordedEvent {
    ValueCreated {
        value_id: usize,
    },
    OpResult {
        output_id: usize,
        input_ids: Vec<usize>,
    },
    ExternalCallRequested {
        call_id: u32,
        kind: ExternalCallKind,
        arg_runtime_ids: Vec<usize>,
        kwarg_runtime_ids: Vec<(usize, usize)>,
    },
    ExternalCallReturned {
        call_id: u32,
        kind: ExternalCallReturnKind,
    },
    ControlCondition {
        condition_id: usize,
        branch_taken: bool,
    },
}

impl RecordedEvent {
    fn from_runtime_event(event: RuntimeObserverEvent<'_>) -> Self {
        match event {
            RuntimeObserverEvent::ValueCreated(value_event) => Self::ValueCreated {
                value_id: value_event.value_id.raw(),
            },
            RuntimeObserverEvent::OpResult(op_event) => Self::OpResult {
                output_id: op_event.output_id.raw(),
                input_ids: match op_event.inputs {
                    OpInputIds::None => vec![],
                    OpInputIds::One(id) => vec![id.raw()],
                    OpInputIds::Two(first, second) => vec![first.raw(), second.raw()],
                },
            },
            RuntimeObserverEvent::ExternalCallRequested(call_event) => Self::ExternalCallRequested {
                call_id: call_event.call_id,
                kind: call_event.kind,
                arg_runtime_ids: call_event.arg_runtime_ids.iter().map(|id| id.raw()).collect(),
                kwarg_runtime_ids: call_event
                    .kwarg_runtime_ids
                    .iter()
                    .map(|(key, value)| (key.raw(), value.raw()))
                    .collect(),
            },
            RuntimeObserverEvent::ExternalCallReturned(call_event) => Self::ExternalCallReturned {
                call_id: call_event.call_id,
                kind: call_event.kind,
            },
            RuntimeObserverEvent::ControlCondition(control_event) => Self::ControlCondition {
                condition_id: control_event.condition_id.raw(),
                branch_taken: control_event.branch_taken,
            },
        }
    }
}

/// Observer implementation that records every event into shared test storage.
///
/// The event buffer is `Arc<Mutex<Vec<_>>>` so tests can resume execution across
/// owned snapshot values while still observing a single, thread-safe event stream.
#[derive(Clone)]
struct RecordingObserver {
    events: Arc<Mutex<Vec<RecordedEvent>>>,
}

impl RecordingObserver {
    fn new(events: Arc<Mutex<Vec<RecordedEvent>>>) -> Self {
        Self { events }
    }
}

impl RuntimeObserver for RecordingObserver {
    fn on_event(&mut self, event: RuntimeObserverEvent<'_>) {
        let mut events = self.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        events.push(RecordedEvent::from_runtime_event(event));
    }
}

/// Builds a recording observer handle plus shared event storage used by tests.
///
/// The returned buffer can be read after each resume step to assert event ordering and payloads.
fn build_recording_observer() -> (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = RuntimeObserverHandle::new(RecordingObserver::new(Arc::clone(&events)));
    (handle, events)
}

/// Clones the current recorded event stream.
///
/// Poisoned mutexes are recovered by taking the inner value so assertions can still inspect
/// partially recorded state from failing execution paths.
fn read_events(events: &Arc<Mutex<Vec<RecordedEvent>>>) -> Vec<RecordedEvent> {
    events.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[fixture]
fn recording() -> (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>) {
    build_recording_observer()
}

/// Selects how a suspended external call is resumed in return-kind tests.
///
/// Each case drives one `ExternalCallReturnKind` assertion path for the same
/// script so event ordering differences stay isolated to resume semantics.
#[derive(Debug, Clone, Copy)]
enum ExternalResumeCase {
    Return,
    Error,
    Future,
}

/// Captures the relevant `RunProgress::FunctionCall` payload for cross-run assertions.
///
/// The snapshot state is retained so tests can continue execution while
/// comparing function identity and argument payload stability.
struct FunctionCallPayload<T: ResourceTracker> {
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(String, MontyObject)>,
    call_id: u32,
    method_call: bool,
    state: Snapshot<T>,
}

/// Builds a stable frozen dataclass value used in observer payload assertions.
///
/// Keeping this fixture value centralized avoids repeating structural literals
/// across tests that compare serialized/runtime-observer object payloads.
fn build_dataclass_point() -> MontyObject {
    MontyObject::Dataclass {
        name: "Point".to_string(),
        type_id: 0,
        field_names: vec!["x".to_string(), "y".to_string()],
        attrs: vec![
            (MontyObject::String("x".to_string()), MontyObject::Int(1)),
            (MontyObject::String("y".to_string()), MontyObject::Int(2)),
        ]
        .into(),
        frozen: true,
    }
}

/// Destructures a `RunProgress::FunctionCall` with a context-rich panic on mismatch.
fn extract_function_call<T: ResourceTracker>(progress: RunProgress<T>, context: &str) -> FunctionCallPayload<T> {
    let RunProgress::FunctionCall {
        function_name,
        args,
        kwargs,
        call_id,
        method_call,
        state,
        ..
    } = progress
    else {
        panic!("{context}: expected function-call progress");
    };

    let kwargs = kwargs
        .into_iter()
        .map(|(key, value)| match key {
            MontyObject::String(name) => (name, value),
            other => panic!("{context}: expected string kwarg key, got {other:?}"),
        })
        .collect();

    FunctionCallPayload {
        function_name,
        args,
        kwargs,
        call_id,
        method_call,
        state,
    }
}

/// Asserts that two extracted function-call payloads match for name and arguments.
fn assert_function_calls_equal<T: ResourceTracker>(left: &FunctionCallPayload<T>, right: &FunctionCallPayload<T>) {
    assert_eq!(left.function_name, right.function_name);
    assert_eq!(left.args, right.args);
    assert_eq!(left.kwargs, right.kwargs);
    assert_eq!(left.call_id, right.call_id);
    assert_eq!(left.method_call, right.method_call);
}

#[rstest]
#[case::returns(ExternalResumeCase::Return, ExternalCallReturnKind::Return)]
#[case::error(ExternalResumeCase::Error, ExternalCallReturnKind::Error)]
#[case::future(ExternalResumeCase::Future, ExternalCallReturnKind::Future)]
fn runtime_observer_emits_external_return_kinds(
    #[case] resume_case: ExternalResumeCase,
    #[case] expected_kind: ExternalCallReturnKind,
    recording: (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>),
) {
    let (observer, events) = recording;
    let run = MontyRun::new("ext_fn(1)".to_owned(), "test.py", vec![], vec!["ext_fn".to_owned()])
        .expect("runner creation should succeed");

    let progress = run
        .start_with_observer(vec![], NoLimitTracker, &mut PrintWriter::Stdout, observer)
        .expect("start should pause at external call");

    let RunProgress::FunctionCall { call_id, state, .. } = progress else {
        panic!("expected function-call progress");
    };

    match resume_case {
        ExternalResumeCase::Return => {
            let completion = state
                .run(MontyObject::Int(7), &mut PrintWriter::Stdout)
                .expect("resume should complete");
            assert!(matches!(completion, RunProgress::Complete(MontyObject::Int(7))));
        }
        ExternalResumeCase::Error => {
            let error = state
                .run(
                    MontyException::new(ExcType::RuntimeError, Some("observer failure".to_owned())),
                    &mut PrintWriter::Stdout,
                )
                .expect_err("resume should return an error");
            assert!(error.to_string().contains("observer failure"));
        }
        ExternalResumeCase::Future => {
            let _ = state
                .run(MontyFuture, &mut PrintWriter::Stdout)
                .expect("future resume should continue execution");
        }
    }

    let events = read_events(&events);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallRequested {
                call_id: observed_call_id,
                kind: ExternalCallKind::Function,
                ..
            } if *observed_call_id == call_id
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallReturned {
                call_id: observed_call_id,
                kind,
            } if *observed_call_id == call_id && *kind == expected_kind
        )
    }));

    if matches!(resume_case, ExternalResumeCase::Return) {
        assert!(events.iter().any(|event| {
            matches!(
                event,
                RecordedEvent::OpResult {
                    input_ids,
                    ..
                } if input_ids.is_empty()
            )
        }));
    }
}

#[rstest]
fn runtime_observer_tracks_os_call_requests(recording: (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>)) {
    let (observer, events) = recording;
    let run = MontyRun::new(
        "from pathlib import Path\nPath('/tmp/observer-test').exists()".to_owned(),
        "test.py",
        vec![],
        vec![],
    )
    .expect("runner creation should succeed");

    let progress = run
        .start_with_observer(vec![], NoLimitTracker, &mut PrintWriter::Stdout, observer)
        .expect("start should pause at OS call");

    let RunProgress::OsCall { state, .. } = progress else {
        panic!("expected OS-call progress");
    };

    let completion = state
        .run(MontyObject::Bool(false), &mut PrintWriter::Stdout)
        .expect("OS call resume should complete");
    assert!(matches!(completion, RunProgress::Complete(MontyObject::Bool(false))));

    let events = read_events(&events);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallRequested {
                kind: ExternalCallKind::Os,
                ..
            }
        )
    }));
}

#[rstest]
fn runtime_observer_tracks_method_call_requests(recording: (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>)) {
    let (observer, events) = recording;
    let run = MontyRun::new("point.sum()".to_owned(), "test.py", vec!["point".to_owned()], vec![])
        .expect("runner creation should succeed");

    let progress = run
        .start_with_observer(
            vec![build_dataclass_point()],
            NoLimitTracker,
            &mut PrintWriter::Stdout,
            observer,
        )
        .expect("start should pause at method call");

    let RunProgress::FunctionCall {
        call_id,
        method_call,
        state,
        ..
    } = progress
    else {
        panic!("expected method-call progress");
    };
    assert!(method_call, "expected method_call=true for dataclass method dispatch");

    let completion = state
        .run(MontyObject::Int(3), &mut PrintWriter::Stdout)
        .expect("method-call resume should complete");
    assert!(matches!(completion, RunProgress::Complete(MontyObject::Int(3))));

    let events = read_events(&events);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallRequested {
                call_id: observed_call_id,
                kind: ExternalCallKind::Method,
                ..
            } if *observed_call_id == call_id
        )
    }));
}

#[rstest]
fn runtime_observer_emits_control_and_operation_events_for_branching_code(
    recording: (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>),
) {
    let (observer, events) = recording;
    let run = MontyRun::new(
        "if x > 0:\n    y = x + 2\nelse:\n    y = x - 2\ny".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        vec![],
    )
    .expect("runner creation should succeed");

    let progress = run
        .start_with_observer(
            vec![MontyObject::Int(1)],
            NoLimitTracker,
            &mut PrintWriter::Stdout,
            observer,
        )
        .expect("start should complete");
    assert!(matches!(progress, RunProgress::Complete(MontyObject::Int(3))));

    let events = read_events(&events);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RecordedEvent::ControlCondition { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::OpResult {
                input_ids,
                ..
            } if input_ids.len() == 2
        )
    }));
}

#[rstest]
fn runtime_observer_repl_paths_emit_external_control_and_op_events(
    recording: (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>),
) {
    let (observer, events) = recording;

    let (repl, init) = MontyRepl::new(
        String::new(),
        "repl.py",
        vec![],
        vec!["ext_fn".to_owned()],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    )
    .expect("REPL initialization should succeed");
    assert_eq!(init, MontyObject::None);

    let progress = repl
        .start_with_observer("ext_fn(1)", &mut PrintWriter::Stdout, observer.clone())
        .expect("repl start should pause at external call");
    let (_, _, _, _, _, call_id, _, state) = progress
        .into_function_call()
        .expect("expected REPL function-call progress");

    let progress = state
        .run(MontyObject::Int(9), &mut PrintWriter::Stdout)
        .expect("repl resume should complete");
    let (repl, value) = progress.into_complete().expect("expected REPL completion");
    assert_eq!(value, MontyObject::Int(9));

    let progress = repl
        .start_with_observer(
            "if 1 > 0:\n    y = 3\nelse:\n    y = 4\ny",
            &mut PrintWriter::Stdout,
            observer,
        )
        .expect("branching snippet should complete");
    let (_repl, value) = progress.into_complete().expect("expected REPL completion");
    assert_eq!(value, MontyObject::Int(3));

    let events = read_events(&events);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallRequested {
                call_id: observed_call_id,
                kind: ExternalCallKind::Function,
                ..
            } if *observed_call_id == call_id
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::ExternalCallReturned {
                call_id: observed_call_id,
                kind: ExternalCallReturnKind::Return,
            } if *observed_call_id == call_id
        )
    }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RecordedEvent::ControlCondition { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::OpResult {
                input_ids,
                ..
            } if input_ids.len() == 2
        )
    }));
}

#[test]
fn noop_observer_preserves_suspend_resume_semantics() {
    let script = "ext_fn(1); ext_fn(2); 3";

    let run_without_observer = MontyRun::new(script.to_owned(), "test.py", vec![], vec!["ext_fn".to_owned()])
        .expect("runner creation should succeed");
    let run_with_noop = MontyRun::new(script.to_owned(), "test.py", vec![], vec!["ext_fn".to_owned()])
        .expect("runner creation should succeed");

    let first_without = extract_function_call(
        run_without_observer
            .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
            .expect("start should pause at first call"),
        "without observer",
    );
    let first_with_noop = extract_function_call(
        run_with_noop
            .start_with_observer(
                vec![],
                NoLimitTracker,
                &mut PrintWriter::Stdout,
                RuntimeObserverHandle::new(NoopRuntimeObserver),
            )
            .expect("start should pause at first call"),
        "with no-op observer",
    );
    assert_function_calls_equal(&first_without, &first_with_noop);

    let second_without = extract_function_call(
        first_without
            .state
            .run(MontyObject::None, &mut PrintWriter::Stdout)
            .expect("resume should pause at second call"),
        "second call without observer",
    );
    let second_with_noop = extract_function_call(
        first_with_noop
            .state
            .run(MontyObject::None, &mut PrintWriter::Stdout)
            .expect("resume should pause at second call"),
        "second call with no-op observer",
    );
    assert_function_calls_equal(&second_without, &second_with_noop);

    let completion_without = second_without
        .state
        .run(MontyObject::None, &mut PrintWriter::Stdout)
        .expect("final resume should complete without observer");
    let completion_with_noop = second_with_noop
        .state
        .run(MontyObject::None, &mut PrintWriter::Stdout)
        .expect("final resume should complete with no-op observer");

    assert_eq!(completion_without.into_complete(), completion_with_noop.into_complete());
}
