//! Converts protocol child events into semantic component events.
//!
//! Every payload is validated as if it came from an untrusted remote, which
//! for the codec it does: arenas are checked into graphs and the roots a call
//! or result names must exist, so JavaScript never indexes a bad arena.

use std::mem;

use monty_proto::{DEFAULT_MAX_DECODE_BYTES, os_call_from_proto, pb};
use monty_types::{
    CallArgs, MontyException, MontyObject, MontyUuid, OsFunctionCall, SourceRange as MontySourceRange,
    unstable::{self, MontyNode},
};

use crate::{
    types::{
        CompleteEvent, Event, FunctionCallEvent, NameLookupEvent, OsCallEvent, PrintEvent, RaisedException,
        ResolveFuturesEvent, SourceRange, StackFrame,
    },
    value,
};

/// Why a child event cannot cross into the component.
#[derive(Debug)]
pub enum EventError {
    /// Its values would expand past the host-memory budget once lifted into JS.
    OverBudget,
    /// A payload no well-behaved child produces; the message says what was wrong.
    Invalid(String),
}

/// Appends one component print per stream run of `print`: a `Print` event
/// carries a run per stream switch, while the component's names one stream.
pub fn push_print(print: &pb::Print, events: &mut Vec<Event>) {
    events.extend(print.segments.iter().map(|segment| {
        Event::Print(PrintEvent {
            stderr: segment.stream == i32::from(pb::PrintStream::Stderr),
            text: segment.text.clone(),
        })
    }));
}

/// Validates and converts the kind of a non-print child event; the caller
/// reads the event's budget fields and expands prints with [`push_print`].
pub fn convert_event(kind: Option<pb::child_event::Kind>) -> Result<Event, EventError> {
    Ok(match kind {
        Some(pb::child_event::Kind::Print(_)) => return Err(invalid("Print event bypassed segment expansion")),
        Some(pb::child_event::Kind::FunctionCall(mut call)) => {
            check_budget(nodes_decoded_size(&call.values.0))?;
            let function_name = mem::take(&mut call.function_name);
            let (call_id, object_id, allow_eager_await) = (call.call_id, call.object_id, call.allow_eager_await);
            let position = component_source_range(call.position.take().unwrap_or_else(MontySourceRange::unknown));
            let args = call
                .into_call_args()
                .map_err(|error| invalid(&format!("invalid FunctionCall payload: {error}")))?;
            let (graph, args, kwargs) = unstable::into_call_args_parts(args);
            Event::FunctionCall(FunctionCallEvent {
                function_name,
                values: value::into_component(graph.into_nodes()),
                args: value::raw_ids(args),
                kwargs: value::raw_pairs(kwargs),
                call_id,
                object_id: object_id.map(|uuid| uuid.to_string()),
                allow_eager_await,
                position,
            })
        }
        Some(pb::child_event::Kind::OsCall(call)) => {
            let call = PreparedOsEvent::from_proto(call).map_err(EventError::Invalid)?;
            check_budget(call.values_decoded_size())?;
            call.into_component()
        }
        Some(pb::child_event::Kind::NameLookup(lookup)) => Event::NameLookup(NameLookupEvent {
            name: lookup.name,
            object_id: match lookup.object_id {
                None => None,
                Some(uuid) => Some(
                    MontyUuid::try_from_slice(&uuid.data)
                        .ok_or_else(|| invalid("NameLookup.object_id is not a 16-byte uuid"))?
                        .to_string(),
                ),
            },
            position: source_range_from_proto(lookup.position),
        }),
        Some(pb::child_event::Kind::ResolveFutures(futures)) => Event::ResolveFutures(ResolveFuturesEvent {
            pending_call_ids: futures.pending_call_ids.into_inner(),
            position: source_range_from_proto(futures.position),
        }),
        Some(pb::child_event::Kind::Complete(complete)) => {
            check_budget(complete.values.as_ref().map_or(0, |arena| nodes_decoded_size(&arena.0)))?;
            let object = MontyObject::try_from(complete)
                .map_err(|error| invalid(&format!("invalid Complete payload: {error}")))?;
            let (graph, root) = unstable::into_graph_parts(object);
            Event::Complete(CompleteEvent {
                values: value::into_component(graph.into_nodes()),
                value: root.0,
            })
        }
        Some(pb::child_event::Kind::Error(error)) => {
            let exception = error
                .exception
                .ok_or_else(|| invalid("Error event carried no exception"))?;
            Event::Error(exception_from_proto(exception)?)
        }
        Some(pb::child_event::Kind::TypingError(error)) => Event::TypingError(error.diagnostics),
        Some(pb::child_event::Kind::DumpResult(result)) => Event::DumpResult(result.state.into_inner()),
        Some(pb::child_event::Kind::Ok(_)) => Event::Ok,
        Some(pb::child_event::Kind::FatalError(error)) => Event::FatalError(error.message),
        Some(pb::child_event::Kind::Shutdown(shutdown)) => Event::Shutdown(shutdown.dump.map(Into::into)),
        None => return Err(invalid("ChildEvent carried no kind")),
    })
}

/// An [`EventError::Invalid`] naming what was wrong.
fn invalid(message: &str) -> EventError {
    EventError::Invalid(message.to_owned())
}

/// Rejects an event whose values exceed the expanded-memory budget.
fn check_budget(size: usize) -> Result<(), EventError> {
    if size > DEFAULT_MAX_DECODE_BYTES {
        Err(EventError::OverBudget)
    } else {
        Ok(())
    }
}

/// Totals the host footprint of an arena's nodes.
fn nodes_decoded_size(nodes: &[MontyNode]) -> usize {
    nodes
        .iter()
        .fold(0, |size, node| size.saturating_add(node.decoded_size()))
}

/// Converts a protocol exception and renders its canonical traceback once.
fn exception_from_proto(exception: pb::RaisedException) -> Result<RaisedException, EventError> {
    let exception =
        MontyException::try_from(exception).map_err(|error| invalid(&format!("invalid exception payload: {error}")))?;
    Ok(RaisedException {
        exc_type: exception.exc_type().to_string(),
        message: exception.message().unwrap_or("").to_owned(),
        traceback: exception.to_string(),
        frames: exception
            .traceback()
            .iter()
            .map(|frame| StackFrame {
                filename: frame.filename.clone(),
                line: frame.start.line,
                column: frame.start.column,
                end_line: frame.end.line,
                end_column: frame.end.column,
                frame_name: frame.frame_name.clone(),
                preview_line: frame.preview_line.as_ref().map(ToString::to_string),
                hide_caret: frame.hide_caret,
                hide_frame_name: frame.hide_frame_name,
            })
            .collect(),
    })
}

/// An OS call projected once into the generic callback values lifted to JS.
struct PreparedOsEvent {
    function_name: String,
    args: CallArgs,
    call_id: u32,
    allow_eager_await: bool,
    /// System sleep duration for the host to await directly.
    system_sleep_secs: Option<f64>,
    /// Where the call expression is in the source.
    position: SourceRange,
}

impl PreparedOsEvent {
    /// Validates and projects a typed protocol call without building WIT
    /// arenas; the error names what was wrong with the call.
    fn from_proto(mut call: pb::OsCall) -> Result<Self, String> {
        let eager_bit = call.allow_eager_await;
        let position = source_range_from_proto(call.position.take());
        let (call_id, call) = os_call_from_proto(call).map_err(|error| format!("invalid OS call: {error}"))?;
        Ok(Self {
            function_name: call.name().to_owned(),
            // The eager bit is only meaningful on a call a future may answer.
            allow_eager_await: eager_bit && OsFunctionCall::accepts_future(call.name()),
            system_sleep_secs: match call {
                OsFunctionCall::SystemSleep(delay) | OsFunctionCall::AsyncSystemSleep(delay) => {
                    Some(delay.as_secs_f64())
                }
                _ => None,
            },
            args: call.to_args(),
            call_id,
            position,
        })
    }

    /// Returns the host footprint of the call's arena.
    fn values_decoded_size(&self) -> usize {
        unstable::call_args_parts(&self.args).0.decoded_size()
    }

    /// Moves the already-budgeted values into the semantic component arena.
    fn into_component(self) -> Event {
        let (graph, args, kwargs) = unstable::into_call_args_parts(self.args);
        Event::OsCall(OsCallEvent {
            function_name: self.function_name,
            allow_eager_await: self.allow_eager_await,
            system_sleep_secs: self.system_sleep_secs,
            values: value::into_component(graph.into_nodes()),
            args: value::raw_ids(args),
            kwargs: value::raw_pairs(kwargs),
            call_id: self.call_id,
            position: self.position,
        })
    }
}

/// Lifts an already-validated position into the component record.
fn component_source_range(range: MontySourceRange) -> SourceRange {
    SourceRange {
        filename: range.filename,
        start: range.start,
        end: range.end,
    }
}

/// Lifts a suspension's position; a missing one reads as an empty range, as in the pool.
fn source_range_from_proto(position: Option<pb::SourceRange>) -> SourceRange {
    component_source_range(position.map_or_else(MontySourceRange::unknown, MontySourceRange::from))
}
