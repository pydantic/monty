//! Builders for the `pb::ChildEvent`s this child emits.
//!
//! Timing fields are left zero: unlike the Monty subprocess child, the CPython
//! worker has no `ResourceTracker`, so it cannot report execution time or a
//! duration budget. The parent's watchdog still enforces wall-clock timeouts.

use monty::{ExcType, MontyException, MontyObject};
use monty_proto::{WireFunctionCall, pb};

/// Wraps an event kind into a `pb::ChildEvent` with zeroed timing fields.
pub fn event(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..Default::default()
    }
}

/// The generic acknowledgement for `Configure` / `Reset` / `Shutdown`.
pub fn ok_event() -> pb::ChildEvent {
    event(pb::child_event::Kind::Ok(pb::Ok {}))
}

/// A turn-ending `Error` for a recoverable protocol violation (wrong state, an
/// unsupported request arm, a bad payload). The session is left intact.
pub fn violation(message: &str) -> pb::ChildEvent {
    error_event(ExcType::RuntimeError, &format!("protocol violation: {message}"))
}

/// A turn-ending `Error` built from an exception type and message (no traceback).
pub fn error_event(exc_type: ExcType, message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(pb::RaisedException {
            exc_type: exc_type.to_string(),
            message: Some(message.to_owned()),
            traceback: vec![],
            data: None,
        }),
    }))
}

/// A turn-ending `Error` from a captured Monty exception (type + message).
pub fn error_from_exception(exc: &MontyException) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(exc.into()),
    }))
}

/// The `FatalError` last gasp: the child exits immediately after sending this.
pub fn fatal_event(message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::FatalError(pb::FatalError {
        message: message.to_owned(),
    }))
}

/// The turn-ending `Complete` carrying the snippet's value.
pub fn complete_event(value: MontyObject) -> pb::ChildEvent {
    event(pb::child_event::Kind::Complete(pb::Complete {
        value: Some(value.into()),
    }))
}

/// A `FunctionCall` suspension for an undefined name the sandbox called.
pub fn function_call_event(
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    call_id: u32,
) -> pb::ChildEvent {
    event(pb::child_event::Kind::FunctionCall(WireFunctionCall {
        function_name,
        args,
        kwargs,
        call_id,
        method_call: false,
    }))
}

/// A `NameLookup` suspension for an undefined name the sandbox referenced.
pub fn name_lookup_event(name: String) -> pb::ChildEvent {
    event(pb::child_event::Kind::NameLookup(pb::NameLookup { name }))
}

/// A streamed `print()` chunk on `stream` (stdout or stderr).
pub fn print_event(stream: pb::PrintStream, text: String) -> pb::ChildEvent {
    event(pb::child_event::Kind::Print(pb::Print {
        stream: stream as i32,
        text,
    }))
}
