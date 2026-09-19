//! The WebAssembly Component Model Monty worker.
//!
//! This is the browser analog of `monty subprocess`: a persistent [`Child`]
//! consumes semantic WIT requests and returns semantic events. The child still
//! shares `monty-proto`'s state machine, but protobuf bytes never cross the
//! component boundary or enter the TypeScript host.
#![expect(unsafe_code, reason = "generated canonical-ABI exports require unsafe code")]

use std::{cell::RefCell, io};

use monty_proto::{
    BudgetVec, DEFAULT_MAX_DECODE_BYTES, FrameError, MAX_FRAME_LEN, PROTOCOL_VERSION, WireArena, exceeds_max_frame_len,
    os_call_from_proto, pb,
    worker::{Child, EventSink, HandleOutcome, protocol_violation},
};
use monty_types::{
    CallArgs, ExcType, MONTY_VERSION, MontyException, MontyUuid, OsFunctionCall, memory_limit_with_headroom,
    unstable::{self, MontyNode},
};

#[expect(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI list lifting uses Vec::from_raw_parts"
)]
mod bindings {
    wit_bindgen::generate!({ world: "monty-runtime" });
}

mod value;

use bindings::exports::pydantic::monty::worker::{
    AutoOsCalls, CallResult, CompleteEvent, ConfigureRequest, DatetimeSource, DispatchResult, Event, FunctionCallEvent,
    Guest, NameLookupEvent, NameLookupResult, OsCallEvent, PrintEvent, RaisedError, RaisedException, RandomSeed,
    RandomStart, Request, SleepMode, StackFrame, Status, TimeZone, TypeCheckFormat,
};

thread_local! {
    /// The session worker, retained for the lifetime of this component instance.
    static CHILD: RefCell<Child> = RefCell::new(Child::default());
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
                max_suspensions: result.max_suspensions,
                max_total_sleep_micros: result.max_total_sleep_micros,
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
                events: vec![event_from_proto(protocol_violation(&format!(
                    "malformed component request: {error}"
                )))],
                max_suspensions: None,
                max_total_sleep_micros: None,
            };
        }
    };
    if let Some(len) = exceeds_max_frame_len(&request) {
        return DispatchResult {
            status: Status::Shutdown,
            events: vec![event_from_proto(child.fatal_event(&format!(
                "request frame of {len} bytes exceeds maximum of {MAX_FRAME_LEN} bytes"
            )))],
            max_suspensions: None,
            max_total_sleep_micros: None,
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
    }
}

/// Collects semantic component events while enforcing the protobuf frame cap.
#[derive(Default)]
struct ComponentEventSink {
    events: Vec<Event>,
}

impl EventSink for ComponentEventSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        if let Some(len) = exceeds_max_frame_len(event) {
            Err(FrameError::FrameTooLarge {
                len,
                max: MAX_FRAME_LEN,
            })
        } else if let Some(pb::child_event::Kind::Print(print)) = &event.kind {
            // A `Print` event carries a run per stream switch, while the
            // component's `PrintEvent` names one stream, so it expands into one
            // event per run rather than converting whole. Checked before the
            // clone below so print text is copied once, not twice.
            for segment in &print.segments {
                self.events.push(Event::Print(PrintEvent {
                    stderr: segment.stream == i32::from(pb::PrintStream::Stderr),
                    text: segment.text.clone(),
                }));
            }
            Ok(())
        } else {
            let mut event = event.clone();
            let component_event = match event.kind.take() {
                Some(pb::child_event::Kind::OsCall(call)) => match PreparedOsEvent::from_proto(call) {
                    Ok(event) => {
                        check_event_value_budget(event.values_decoded_size())?;
                        event.into_component()
                    }
                    Err(message) => invalid_event(&message),
                },
                kind => {
                    event.kind = kind;
                    check_event_value_budget(event_values_decoded_size(&event))?;
                    event_from_proto(event)
                }
            };
            self.events.push(component_event);
            Ok(())
        }
    }
}

/// Rejects a component event whose values exceed the expanded-memory budget.
fn check_event_value_budget(size: usize) -> Result<(), FrameError> {
    if size > DEFAULT_MAX_DECODE_BYTES {
        Err(FrameError::Io(io::Error::other(
            "component event values exceed the host-memory budget",
        )))
    } else {
        Ok(())
    }
}

/// Estimates the expanded host size of an event's arena before lifting it
/// into JS.
fn event_values_decoded_size(event: &pb::ChildEvent) -> usize {
    match &event.kind {
        Some(pb::child_event::Kind::Complete(complete)) => {
            complete.values.as_ref().map_or(0, |arena| nodes_decoded_size(&arena.0))
        }
        Some(pb::child_event::Kind::FunctionCall(call)) => nodes_decoded_size(&call.values.0),
        _ => 0,
    }
}

/// Totals the host footprint of an arena's nodes.
fn nodes_decoded_size(nodes: &[MontyNode]) -> usize {
    nodes
        .iter()
        .fold(0, |size, node| size.saturating_add(node.decoded_size()))
}

/// An OS call projected once into the generic callback values lifted to JS.
struct PreparedOsEvent {
    function_name: String,
    args: CallArgs,
    call_id: u32,
    allow_eager_await: bool,
    /// The wait a `'system'` sleep asks the host to perform itself, in seconds.
    system_sleep_secs: Option<f64>,
}

impl PreparedOsEvent {
    /// Validates and projects a typed protocol call without building WIT
    /// arenas; the error names what was wrong with the call.
    fn from_proto(call: pb::OsCall) -> Result<Self, String> {
        let eager_bit = call.allow_eager_await;
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
        })
    }
}

/// Converts a semantic component request into the child state machine's
/// type. A request's arena is validated here; the roots the request names
/// are checked by the child, like any wire frame's.
fn request_from_component(request: Request) -> Result<pb::ParentRequest, String> {
    let mut budget = value::DecodeBudget::default();
    let kind = match request {
        Request::Configure(request) => pb::parent_request::Kind::Configure(configure_from_component(request)),
        Request::Feed(request) => pb::parent_request::Kind::Feed(pb::Feed {
            code: request.code,
            inputs: request
                .inputs
                .into_iter()
                .map(|input| pb::NamedRef {
                    name: input.name,
                    value: input.value,
                })
                .collect(),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
            skip_type_check: request.skip_type_check,
            cwd: request.cwd,
        }),
        Request::ResumeCall(request) => pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: request.call_id,
            result: Some(call_result_from_component(request.outcome)),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
        }),
        Request::ResumeNameLookup(request) => {
            let kind = match request.outcome {
                NameLookupResult::Value(root) => pb::resume_name_lookup::Kind::Value(root),
                NameLookupResult::Undefined => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
                NameLookupResult::Error(error) => {
                    pb::resume_name_lookup::Kind::Error(raised_exception_from_component(error))
                }
            };
            pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
                values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
                kind: Some(kind),
            })
        }
        Request::ResumeFutures(request) => pb::parent_request::Kind::ResumeFutures(pb::ResumeFutures {
            results: request
                .results
                .into_iter()
                .map(|result| pb::FutureResult {
                    call_id: result.call_id,
                    result: Some(call_result_from_component(result.outcome)),
                })
                .collect(),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
        }),
        Request::AbortFeed(error) => pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
            exception: Some(raised_exception_from_component(error)),
        }),
        Request::Dump => pb::parent_request::Kind::Dump(pb::Dump {}),
        Request::Load(state) => pb::parent_request::Kind::Load(pb::Load { state: state.into() }),
        Request::Reset => pb::parent_request::Kind::Reset(pb::Reset {}),
    };
    Ok(pb::ParentRequest {
        kind: Some(kind),
        trace_parent: None,
    })
}

/// Converts session options into the protocol child's configuration type.
fn configure_from_component(request: ConfigureRequest) -> pb::Configure {
    pb::Configure {
        script_name: request.script_name,
        limits: request.limits.map(|limits| pb::ResourceLimits {
            max_feed_duration_micros: limits.max_feed_duration_micros,
            max_turn_duration_micros: limits.max_turn_duration_micros,
            max_memory_bytes: limits.max_memory_bytes,
            gc_interval: limits.gc_interval,
            max_recursion_depth: limits.max_recursion_depth,
            max_suspensions: limits.max_suspensions,
            max_total_sleep_micros: limits.max_total_sleep_micros,
        }),
        type_check: request.type_check,
        type_check_stubs: request.type_check_stubs,
        monty_version: MONTY_VERSION.to_owned(),
        assert_message_annotations: request.assert_message_annotations,
        type_check_format: i32::from(type_check_format_from_component(request.type_check_format)),
        type_check_color: request.type_check_color,
        protocol_version: PROTOCOL_VERSION,
        // Frames arrive as one batch at the end of a turn, but their
        // boundaries survive it: the host gets one print callback per frame,
        // and a print collector charges its cap per frame.
        print_flush_interval_ms: request.print_flush_interval_ms,
        auto_os_calls: request.auto_os_calls.map(auto_os_calls_from_component),
    }
}

/// Converts the component's `auto-os-calls` record into the protocol message;
/// the protocol conversion validates it as it would from any parent.
fn auto_os_calls_from_component(calls: AutoOsCalls) -> pb::AutoOsCalls {
    let datetime = calls.datetime.map(|source| match source {
        DatetimeSource::System => pb::auto_os_calls::Datetime::System(pb::Unit {}),
        DatetimeSource::CallHost => pb::auto_os_calls::Datetime::CallHost(pb::Unit {}),
        DatetimeSource::Fixed(fixed) => pb::auto_os_calls::Datetime::Fixed(pb::FixedDateTime {
            unix_seconds: fixed.unix_seconds,
            microsecond: fixed.microsecond,
        }),
    });
    let timezone = calls.timezone.map(|zone| pb::SandboxTimeZone {
        zone: Some(match zone {
            TimeZone::System => pb::sandbox_time_zone::Zone::System(pb::Unit {}),
            TimeZone::CallHost => pb::sandbox_time_zone::Zone::CallHost(pb::Unit {}),
            TimeZone::Fixed(fixed) => pb::sandbox_time_zone::Zone::Fixed(pb::TimeZone {
                offset_seconds: fixed.offset_seconds,
                name: fixed.name,
            }),
        }),
    });
    let sleep = calls.sleep.map(|mode| pb::SleepMode {
        mode: Some(match mode {
            SleepMode::System(max_micros) => pb::sleep_mode::Mode::System(pb::SystemSleep { max_micros }),
            SleepMode::CallHost => pb::sleep_mode::Mode::CallHost(pb::Unit {}),
            SleepMode::Zero => pb::sleep_mode::Mode::Zero(pb::Unit {}),
        }),
    });
    let random_start = calls.random_start.map(|start| match start {
        RandomStart::System => pb::auto_os_calls::RandomStart::RandomSystem(pb::Unit {}),
        RandomStart::CallHost => pb::auto_os_calls::RandomStart::RandomCallHost(pb::Unit {}),
        RandomStart::Seed(seed) => pb::auto_os_calls::RandomStart::Seed(pb::RandomSeed {
            value: Some(match seed {
                RandomSeed::Int(bytes) => pb::random_seed::Value::Int(bytes.into()),
                RandomSeed::Float(f) => pb::random_seed::Value::Float(f),
                RandomSeed::Str(s) => pb::random_seed::Value::Str(s),
                RandomSeed::Bytes(b) => pb::random_seed::Value::Bytes(b.into()),
            }),
        }),
    });
    pb::AutoOsCalls {
        datetime,
        timezone,
        sleep,
        random_start,
    }
}

/// Converts the component's diagnostic format into the protocol enum.
fn type_check_format_from_component(format: TypeCheckFormat) -> pb::TypeCheckFormat {
    match format {
        TypeCheckFormat::Full => pb::TypeCheckFormat::Full,
        TypeCheckFormat::Concise => pb::TypeCheckFormat::Concise,
        TypeCheckFormat::Azure => pb::TypeCheckFormat::Azure,
        TypeCheckFormat::Json => pb::TypeCheckFormat::Json,
        TypeCheckFormat::JsonLines => pb::TypeCheckFormat::JsonLines,
        TypeCheckFormat::Rdjson => pb::TypeCheckFormat::Rdjson,
        TypeCheckFormat::Pylint => pb::TypeCheckFormat::Pylint,
        TypeCheckFormat::Gitlab => pb::TypeCheckFormat::Gitlab,
        TypeCheckFormat::Github => pb::TypeCheckFormat::Github,
    }
}

/// Converts a host call outcome into the child state machine's result type;
/// a returned value stays the index into the request's arena.
fn call_result_from_component(result: CallResult) -> pb::ExtFunctionResult {
    let kind = match result {
        CallResult::ReturnValue(root) => pb::ext_function_result::Kind::ReturnValue(root),
        CallResult::Error(error) => pb::ext_function_result::Kind::Error(raised_exception_from_component(error)),
        CallResult::PendingFuture(call_id) => pb::ext_function_result::Kind::Future(call_id),
        CallResult::NotFound(name) => pb::ext_function_result::Kind::NotFound(name),
        CallResult::NotHandled => pb::ext_function_result::Kind::NotHandled(pb::Unit {}),
    };
    pb::ExtFunctionResult { kind: Some(kind) }
}

/// Converts a host-raised error into the protocol's exception message; the
/// host supplies no traceback or structured data.
fn raised_exception_from_component(error: RaisedError) -> pb::RaisedException {
    pb::RaisedException {
        exc_type: error.exc_type,
        message: Some(error.message),
        traceback: BudgetVec::new(),
        data: None,
    }
}

/// Converts one child event into its semantic component representation.
fn event_from_proto(event: pb::ChildEvent) -> Event {
    match event.kind {
        Some(pb::child_event::Kind::Print(_)) => invalid_event("Print event bypassed segment expansion"),
        Some(pb::child_event::Kind::FunctionCall(call)) => {
            let object_id = call.object_id.map(|uuid| uuid.to_string());
            Event::FunctionCall(FunctionCallEvent {
                function_name: call.function_name,
                values: value::into_component(call.values.0.into_inner()),
                args: value::raw_ids(call.args.into_inner()),
                kwargs: value::raw_pairs(call.kwargs.into_inner()),
                call_id: call.call_id,
                object_id,
                allow_eager_await: call.allow_eager_await,
            })
        }
        Some(pb::child_event::Kind::OsCall(_)) => invalid_event("OsCall event bypassed component budget preparation"),
        Some(pb::child_event::Kind::NameLookup(lookup)) => Event::NameLookup(NameLookupEvent {
            name: lookup.name,
            // Self-produced by this worker, so always a valid 16-byte uuid.
            object_id: lookup
                .object_id
                .and_then(|uuid| MontyUuid::try_from_slice(&uuid.data))
                .map(|uuid| uuid.to_string()),
        }),
        Some(pb::child_event::Kind::ResolveFutures(futures)) => {
            Event::ResolveFutures(futures.pending_call_ids.into_inner())
        }
        Some(pb::child_event::Kind::Complete(complete)) => complete.values.map_or_else(
            || invalid_event("Complete event carried no values"),
            |arena| {
                Event::Complete(CompleteEvent {
                    values: value::into_component(arena.0.into_inner()),
                    value: complete.value,
                })
            },
        ),
        Some(pb::child_event::Kind::Error(error)) => error
            .exception
            .map(exception_from_proto)
            .map_or_else(|| invalid_event("Error event carried no exception"), Event::Error),
        Some(pb::child_event::Kind::TypingError(error)) => Event::TypingError(error.diagnostics),
        Some(pb::child_event::Kind::DumpResult(result)) => Event::DumpResult(result.state.into_inner()),
        Some(pb::child_event::Kind::Ok(_)) => Event::Ok,
        Some(pb::child_event::Kind::FatalError(error)) => Event::FatalError(error.message),
        Some(pb::child_event::Kind::Shutdown(shutdown)) => Event::Shutdown(shutdown.dump.map(Into::into)),
        None => invalid_event("ChildEvent carried no kind"),
    }
}

/// Converts a protocol exception and renders its canonical traceback once.
fn exception_from_proto(exception: pb::RaisedException) -> RaisedException {
    match MontyException::try_from(exception) {
        Ok(exception) => RaisedException {
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
        },
        Err(error) => RaisedException {
            exc_type: ExcType::RuntimeError.to_string(),
            message: format!("invalid exception from worker: {error}"),
            traceback: format!("RuntimeError: invalid exception from worker: {error}"),
            frames: vec![],
        },
    }
}

/// Creates a fatal semantic event for an impossible child output shape.
fn invalid_event(message: &str) -> Event {
    Event::FatalError(format!("worker produced a malformed event: {message}"))
}

bindings::export!(Component with_types_in bindings);
