//! Raw protobuf turns: an unframed `ClientRequest` in, its unframed
//! `WorkerEvent`s out, one transport message each.
//!
//! This serves hosts that relay the wire protocol as is, such as a WebSocket
//! server whose messages are exactly these bytes, so no WIT conversion runs.

use monty_proto::{
    FrameError, decode_frame, encode_to_capped_vec, pb,
    worker::{EventSink, HandleOutcome, SessionBudget, Worker, protocol_violation},
};
use monty_types::memory_limit_with_headroom;

use crate::bindings::exports::pydantic::monty::worker::{FrameResult, Status};

/// Decodes and handles one request message, mirroring `monty subprocess`'s
/// recovery rules: an invalid payload is a recoverable protocol violation, and
/// an oversize event or broken frame ends the worker with a fatal event.
pub(crate) fn dispatch_frame(worker: &mut Worker, request: &[u8]) -> FrameResult {
    let mut sink = MessageSink::default();
    let outcome = match decode_frame::<pb::ClientRequest>(request) {
        Ok(request) => match worker.handle(request, &mut sink) {
            Ok(outcome) => outcome,
            Err(FrameError::FrameTooLarge { len, max }) => {
                let message = format!("response frame of {len} bytes exceeds maximum of {max} bytes");
                let _ = sink.send(&worker.fatal_event(&message));
                HandleOutcome::Shutdown
            }
            Err(error) => {
                let _ = sink.send(&worker.fatal_event(&format!("event sink failed: {error}")));
                HandleOutcome::Shutdown
            }
        },
        Err(FrameError::Decode(error)) => {
            let _ = sink.send(&protocol_violation(&format!("malformed request: {error}")));
            HandleOutcome::Continue
        }
        Err(error) => {
            let _ = sink.send(&worker.fatal_event(&format!("malformed request frame: {error}")));
            HandleOutcome::Shutdown
        }
    };
    FrameResult {
        status: if outcome == HandleOutcome::Continue {
            Status::Continue
        } else {
            Status::Shutdown
        },
        events: sink.messages,
    }
}

/// Encodes events the host emits outside a turn, such as a last-gasp fatal error.
pub(crate) fn encode_events(events: &[pb::WorkerEvent]) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|event| encode_to_capped_vec(event).ok())
        .collect()
}

/// Collects each event as one unframed message.
#[derive(Default)]
struct MessageSink {
    messages: Vec<Vec<u8>>,
}

impl EventSink for MessageSink {
    fn send(&mut self, event: &pb::WorkerEvent) -> Result<(), FrameError> {
        self.messages.push(encode_to_capped_vec(event)?);
        Ok(())
    }

    /// Arms the session's ceiling before its first request runs, as the subprocess shell does.
    fn session_opened(&mut self, budget: SessionBudget) {
        // only fails when the allocator is not global, which `ALLOC` rules out
        let _ = monty_alloc::set_hard_limit(memory_limit_with_headroom(budget.max_memory, budget.type_check));
    }
}
