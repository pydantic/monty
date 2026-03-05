//! Behavioural coverage for generic runtime observer events.

use std::sync::{Arc, Mutex};

use monty::{
    ExcType, ExternalCallKind, ExternalCallReturnKind, MontyException, MontyObject, MontyRun, NoLimitTracker,
    OpInputIds, PrintWriter, RunInputs, RunProgress, RuntimeObserver, RuntimeObserverEvent, RuntimeObserverHandle,
};
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};

#[expect(
    dead_code,
    reason = "shared helper module defines utilities consumed by sibling integration tests"
)]
#[path = "support/test_utils.rs"]
mod test_utils;

/// Test-friendly observer event projection used by BDD steps.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordedEvent {
    ExternalCallRequested { call_id: u32, kind: ExternalCallKind },
    ExternalCallReturned { call_id: u32, kind: ExternalCallReturnKind },
    OpResult { input_count: usize },
    ControlCondition,
}

impl RecordedEvent {
    /// Converts a runtime observer callback payload into the reduced BDD event model.
    fn from_runtime_event(event: RuntimeObserverEvent<'_>) -> Option<Self> {
        match event {
            RuntimeObserverEvent::ExternalCallRequested(call_event) => Some(Self::ExternalCallRequested {
                call_id: call_event.call_id,
                kind: call_event.kind,
            }),
            RuntimeObserverEvent::ExternalCallReturned(call_event) => Some(Self::ExternalCallReturned {
                call_id: call_event.call_id,
                kind: call_event.kind,
            }),
            RuntimeObserverEvent::ControlCondition(_) => Some(Self::ControlCondition),
            RuntimeObserverEvent::OpResult(op_event) => {
                let input_count = match op_event.inputs {
                    OpInputIds::None => 0,
                    OpInputIds::One(_) => 1,
                    OpInputIds::Two(_, _) => 2,
                };
                Some(Self::OpResult { input_count })
            }
            RuntimeObserverEvent::ValueCreated(_) => None,
        }
    }
}

/// Observer that records events into a shared `Arc<Mutex<Vec<RecordedEvent>>>` buffer.
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
        if let Some(record) = RecordedEvent::from_runtime_event(event) {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(record);
        }
    }
}

/// Shared mutable world state for runtime-observer BDD scenarios.
#[derive(Default)]
struct RuntimeObserverWorld {
    script: String,
    events: Vec<RecordedEvent>,
    call_id: Option<u32>,
}

impl RuntimeObserverWorld {
    /// Asserts at least one recorded event matches `predicate` for the stored call ID.
    fn assert_has_event<F>(&self, predicate: F, error_message: &str)
    where
        F: Fn(&RecordedEvent, u32) -> bool,
    {
        let Some(call_id) = self.call_id else {
            panic!("call id should be recorded");
        };
        assert!(
            self.events.iter().any(|event| predicate(event, call_id)),
            "{error_message}"
        );
    }

    /// Asserts the observer stream contains an external-call request of `kind`.
    fn assert_has_external_call_requested(&self, kind: ExternalCallKind) {
        self.assert_has_event(
            |event, cid| {
                matches!(
                    event,
                    RecordedEvent::ExternalCallRequested { call_id: id, kind: k }
                        if *id == cid && *k == kind
                )
            },
            &format!("expected ExternalCallRequested event with kind {kind:?}"),
        );
    }

    /// Asserts the observer stream contains an external-call return of `kind`.
    fn assert_has_external_call_returned(&self, kind: ExternalCallReturnKind) {
        self.assert_has_event(
            |event, cid| {
                matches!(
                    event,
                    RecordedEvent::ExternalCallReturned { call_id: id, kind: k }
                        if *id == cid && *k == kind
                )
            },
            &format!("expected ExternalCallReturned event with kind {kind:?}"),
        );
    }
}

/// Captures the handles returned by a start call with a recording observer.
#[derive(Debug)]
struct RecordingRunFixture {
    events: Arc<Mutex<Vec<RecordedEvent>>>,
    observer: RuntimeObserverHandle,
    progress: RunProgress<NoLimitTracker>,
}

/// Fixture that creates a fresh world per BDD scenario.
#[fixture]
fn world() -> RuntimeObserverWorld {
    RuntimeObserverWorld::default()
}

/// Fixture that provides a recording observer and shared event storage.
#[fixture]
fn recording_observer_fixture() -> (RuntimeObserverHandle, Arc<Mutex<Vec<RecordedEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observer = RuntimeObserverHandle::new(RecordingObserver::new(Arc::clone(&events)));
    (observer, events)
}

/// Starts a run with a recording observer and returns progress plus event handles.
fn recording_start_with_observer(
    script: String,
    input_names: Vec<String>,
    inputs: Vec<MontyObject>,
) -> RecordingRunFixture {
    let (observer, events) = recording_observer_fixture();
    let run = MontyRun::new(script, "test.py", input_names).expect("runner creation should succeed");
    let progress = run
        .start_with_observer(
            RunInputs {
                inputs,
                resource_tracker: NoLimitTracker,
            },
            &mut PrintWriter::Stdout,
            observer.clone(),
        )
        .expect("start_with_observer should succeed");

    RecordingRunFixture {
        events,
        observer,
        progress,
    }
}

/// Starts execution with a recording observer, resumes with a host result, and
/// stores call/event state on the BDD world.
fn start_and_resume_generic<R, A>(world: &mut RuntimeObserverWorld, resume_value: R, assert_result: A)
where
    R: Into<monty::ExtFunctionResult>,
    A: FnOnce(Result<RunProgress<NoLimitTracker>, MontyException>),
{
    let fixture = recording_start_with_observer(world.script.clone(), vec![], vec![]);

    let function_call = match fixture.progress {
        progress @ RunProgress::FunctionCall(_) => test_utils::as_function_call(progress, "start and resume generic"),
        RunProgress::OsCall(_) => {
            panic!("start and resume generic: expected function-call progress, got os-call progress")
        }
        other => panic!("start and resume generic: expected function-call progress, got {other:?}"),
    };
    assert_eq!(function_call.function_name, "ext_fn");
    assert_eq!(function_call.args, vec![MontyObject::Int(1)]);
    assert!(function_call.kwargs.is_empty());
    world.call_id = Some(function_call.call_id);

    let result = function_call.resume(resume_value, &mut PrintWriter::Stdout);
    assert_result(result);

    drop(fixture.observer);
    world
        .events
        .clone_from(&fixture.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
}

/// Provides a script that pauses at one external call.
#[given("a suspendable script with one external function call")]
fn given_external_function_script(world: &mut RuntimeObserverWorld) {
    "ext_fn(1)".clone_into(&mut world.script);
}

/// Provides a script that emits branch-control and operation-result events.
#[given("a script with arithmetic and branch control flow")]
fn given_branching_script(world: &mut RuntimeObserverWorld) {
    "if x > 0:\n    y = x + 2\nelse:\n    y = x - 2\ny".clone_into(&mut world.script);
}

/// Starts execution with an observer and resumes with a concrete return value.
#[when("execution starts with a recording observer and resumes with integer return value")]
fn when_start_and_resume_with_return(world: &mut RuntimeObserverWorld) {
    start_and_resume_generic(world, MontyObject::Int(9), |result| {
        let completion = result.expect("resume should complete");
        assert!(matches!(completion, RunProgress::Complete(MontyObject::Int(9))));
    });
}

/// Starts execution with an observer and runs a branch snippet to completion.
#[when("execution starts with a recording observer and runs to completion")]
fn when_start_and_complete(world: &mut RuntimeObserverWorld) {
    let fixture = recording_start_with_observer(world.script.clone(), vec!["x".to_owned()], vec![MontyObject::Int(1)]);

    assert!(matches!(fixture.progress, RunProgress::Complete(MontyObject::Int(3))));

    drop(fixture.observer);
    world
        .events
        .clone_from(&fixture.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
}

/// Starts execution with an observer and resumes with an external exception.
#[when("execution starts with a recording observer and resumes with raised exception")]
fn when_start_and_resume_with_exception(world: &mut RuntimeObserverWorld) {
    start_and_resume_generic(
        world,
        MontyException::new(ExcType::RuntimeError, Some("bdd failure".to_owned())),
        |result| {
            let error = result.expect_err("resume should return an error");
            assert!(error.to_string().contains("bdd failure"));
        },
    );
}

/// Asserts that an external function request event exists for the recorded call.
#[then("observer events include an external function request")]
fn then_has_external_request(world: &RuntimeObserverWorld) {
    world.assert_has_external_call_requested(ExternalCallKind::Function);
}

/// Asserts that a successful external return event exists for the recorded call.
#[then("observer events include an external function return")]
fn then_has_external_return(world: &RuntimeObserverWorld) {
    world.assert_has_external_call_returned(ExternalCallReturnKind::Return);
}

/// Asserts that an external error return event exists for the recorded call.
#[then("observer events include an external error return")]
fn then_has_external_error_return(world: &RuntimeObserverWorld) {
    world.assert_has_external_call_returned(ExternalCallReturnKind::Error);
}

/// Asserts that at least one control-condition event was emitted.
#[then("observer events include a control condition event")]
fn then_has_control_condition(world: &RuntimeObserverWorld) {
    assert!(
        world
            .events
            .iter()
            .any(|event| matches!(event, RecordedEvent::ControlCondition))
    );
}

/// Asserts that at least one operation-result event had tracked inputs.
#[then("observer events include an operation-result event with inputs")]
fn then_has_op_result_with_inputs(world: &RuntimeObserverWorld) {
    assert!(world.events.iter().any(|event| {
        matches!(
            event,
            RecordedEvent::OpResult {
                input_count,
            } if *input_count >= 1
        )
    }));
}

/// Scenario: external function calls emit request and return observer events.
#[scenario(
    path = "tests/features/runtime_observer_events.feature",
    name = "Function call emits request and return events"
)]
fn function_call_emits_request_and_return(world: RuntimeObserverWorld) {
    drop(world);
}

/// Scenario: branching code emits control-condition and op-result observer events.
#[scenario(
    path = "tests/features/runtime_observer_events.feature",
    name = "Branching code emits control and operation-result events"
)]
fn branching_code_emits_control_and_operation_result(world: RuntimeObserverWorld) {
    drop(world);
}

/// Scenario: failed external calls emit error return observer events.
#[scenario(
    path = "tests/features/runtime_observer_events.feature",
    name = "Failed external call emits error return event"
)]
fn failed_call_emits_error_return(world: RuntimeObserverWorld) {
    drop(world);
}
