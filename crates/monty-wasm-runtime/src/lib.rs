//! The WebAssembly Component Model Monty worker.
//!
//! This is the browser analog of `monty subprocess`: a persistent [`Child`]
//! consumes semantic WIT requests and returns semantic events. The child still
//! shares `monty-proto`'s state machine, but protobuf bytes never cross the
//! component boundary or enter the TypeScript host. The conversions between
//! the two live in `monty-wasm-types`, shared with the codec component.
#![expect(unsafe_code, reason = "generated canonical-ABI exports require unsafe code")]

use std::{cell::RefCell, collections::hash_map::RandomState, io, time::Instant};

use monty_proto::{
    FrameError, MAX_FRAME_LEN, exceeds_max_frame_len, pb,
    worker::{Child, EventSink, HandleOutcome, protocol_violation},
};
use monty_types::memory_limit_with_headroom;
use monty_wasm_types::{
    EventError, convert_event, push_print, request_from_component,
    types::{DispatchResult, Event, Request, Status},
};

#[expect(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI list lifting uses Vec::from_raw_parts"
)]
mod bindings {
    wit_bindgen::generate!({
        world: "monty-runtime",
        path: "../monty-wasm-types/wit",
        with: { "pydantic:monty/types": monty_wasm_types::types },
    });
}

use bindings::exports::pydantic::monty::worker::Guest;

thread_local! {
    /// The session worker, retained for the lifetime of this component instance.
    static CHILD: RefCell<Child> = RefCell::new(new_child());
}

/// Builds the worker after warming the WASI adapter, so the adapter's buffers
/// join the memory baseline instead of the first session's budget.
///
/// The preview1 adapter allocates through the component's allocator on the
/// first clock read and the first `random_get`, both of which the interpreter
/// would otherwise make during its first feed, after the allocator is armed.
fn new_child() -> Child {
    let _ = Instant::now();
    let _ = RandomState::new();
    Child::default()
}

/// Counts component allocations against the session's `max_memory` limit.
///
/// Linear memory does not shrink, so the allocator tracks live allocations;
/// crossing its hard ceiling traps the instance and lets the pool replace it.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// Implements the typed component export over Monty's protocol child.
struct Component;

impl Guest for Component {
    fn dispatch(request: Request) -> DispatchResult {
        let (result, allocator_ready) = CHILD.with_borrow_mut(|child| {
            let mut result = dispatch(child, request);
            let budget = child.session_budget();
            result.max_suspensions = budget.max_suspensions.map(|limit| limit as u64);
            result.max_total_sleep_micros = budget
                .max_total_sleep
                .map(|limit| u64::try_from(limit.as_micros()).unwrap_or(u64::MAX));
            let hard_memory_limit = memory_limit_with_headroom(budget.max_memory, budget.type_check);
            let allocator_ready = monty_alloc::set_hard_limit(hard_memory_limit);
            (result, allocator_ready)
        });
        if let Err(error) = allocator_ready {
            DispatchResult {
                status: Status::Shutdown,
                events: vec![Event::FatalError(error.to_owned())],
                ..result
            }
        } else {
            result
        }
    }
}

/// Handles one semantic request while preserving the protocol child's limits.
fn dispatch(child: &mut Child, request: Request) -> DispatchResult {
    let request = match request_from_component(request) {
        Ok(request) => request,
        Err(error) => {
            return DispatchResult {
                status: Status::Continue,
                events: vec![own_event(protocol_violation(&format!(
                    "malformed component request: {error}"
                )))],
                max_suspensions: None,
                max_total_sleep_micros: None,
                feed_execution_micros: 0,
                max_feed_duration_micros: None,
                max_turn_duration_micros: None,
            };
        }
    };
    if let Some(len) = exceeds_max_frame_len(&request) {
        return DispatchResult {
            status: Status::Shutdown,
            events: vec![own_event(child.fatal_event(&format!(
                "request frame of {len} bytes exceeds maximum of {MAX_FRAME_LEN} bytes"
            )))],
            max_suspensions: None,
            max_total_sleep_micros: None,
            feed_execution_micros: 0,
            max_feed_duration_micros: None,
            max_turn_duration_micros: None,
        };
    }

    let mut sink = ComponentEventSink::default();
    let outcome = match child.handle(request, &mut sink) {
        Ok(outcome) => outcome,
        Err(FrameError::FrameTooLarge { len, max }) => {
            let _ =
                sink.send(&child.fatal_event(&format!("response frame of {len} bytes exceeds maximum of {max} bytes")));
            HandleOutcome::Shutdown
        }
        Err(error) => {
            let _ = sink.send(&child.fatal_event(&format!("component event sink failed: {error}")));
            HandleOutcome::Shutdown
        }
    };
    DispatchResult {
        status: if matches!(outcome, HandleOutcome::Continue) {
            Status::Continue
        } else {
            Status::Shutdown
        },
        events: sink.events,
        max_suspensions: None,
        max_total_sleep_micros: None,
        feed_execution_micros: sink.feed_execution_micros,
        max_feed_duration_micros: sink.max_feed_duration_micros,
        max_turn_duration_micros: sink.max_turn_duration_micros,
    }
}

/// Collects semantic component events while enforcing the protobuf frame cap.
#[derive(Default)]
struct ComponentEventSink {
    events: Vec<Event>,
    feed_execution_micros: u64,
    max_feed_duration_micros: Option<u64>,
    max_turn_duration_micros: Option<u64>,
}

impl EventSink for ComponentEventSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        if let Some(len) = exceeds_max_frame_len(event) {
            Err(FrameError::FrameTooLarge {
                len,
                max: MAX_FRAME_LEN,
            })
        } else if let Some(pb::child_event::Kind::Print(print)) = &event.kind {
            // checked before the clone below so print text is copied once, not twice
            push_print(print, &mut self.events);
            Ok(())
        } else {
            self.feed_execution_micros = event.feed_execution_micros;
            self.max_feed_duration_micros = event.max_feed_duration_micros;
            self.max_turn_duration_micros = event.max_turn_duration_micros;
            let component_event = match convert_event(event.kind.clone()) {
                Ok(event) => event,
                Err(EventError::OverBudget) => {
                    return Err(FrameError::Io(io::Error::other(
                        "component event values exceed the host-memory budget",
                    )));
                }
                Err(EventError::Invalid(message)) => invalid_event(&message),
            };
            self.events.push(component_event);
            Ok(())
        }
    }
}

/// Converts an event this worker built itself, outside any child turn.
fn own_event(event: pb::ChildEvent) -> Event {
    convert_event(event.kind).unwrap_or_else(|error| invalid_event(&format!("{error:?}")))
}

/// Creates a fatal semantic event for an impossible child output shape.
fn invalid_event(message: &str) -> Event {
    Event::FatalError(format!("worker produced a malformed event: {message}"))
}

bindings::export!(Component with_types_in bindings);
