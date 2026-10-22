//! The protocol codec component for remote Monty workers.
//!
//! A browser cannot run `monty-pool`, so `@pydantic/monty`'s WebSocket client
//! drives the protocol from TypeScript and uses this component to turn semantic
//! requests into protobuf frames and frames back into semantic events. It holds
//! no interpreter and no state: every call is one conversion.
//!
//! Frames from the remote are untrusted. They are decoded under the protocol's
//! frame cap and allocation budget, and the events are validated as
//! `monty-pool` validates a worker's (see `monty_wasm_types::convert_event`),
//! so a malformed frame becomes an error string rather than a bad arena.
#![expect(unsafe_code, reason = "generated canonical-ABI exports require unsafe code")]

use monty_proto::{FrameError, decode_frame, encode_to_capped_vec, pb};
use monty_wasm_types::{EventError, convert_event, push_print, request_from_component, types::Request};

#[expect(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI list lifting uses Vec::from_raw_parts"
)]
mod bindings {
    wit_bindgen::generate!({
        world: "monty-codec",
        path: "../monty-wasm-types/wit",
        with: { "pydantic:monty/types": monty_wasm_types::types },
    });
}

use bindings::exports::pydantic::monty::codec::{DecodedFrame, Guest};

/// Implements the typed codec export.
struct Codec;

impl Guest for Codec {
    /// The error is the message of the session-preserving `RuntimeError` the
    /// caller raises instead: the request was never sent.
    fn encode_request(request: Request) -> Result<Vec<u8>, String> {
        let request = request_from_component(request)
            .map_err(|error| format!("protocol violation: malformed component request: {error}"))?;
        encode_to_capped_vec(&request).map_err(|error| match error {
            FrameError::FrameTooLarge { len, max } => {
                format!("request frame of {len} bytes exceeds the maximum of {max} bytes")
            }
            error => format!("request could not be encoded: {error}"),
        })
    }

    /// The error describes a protocol violation; the caller discards the connection.
    fn decode_event(frame: Vec<u8>) -> Result<DecodedFrame, String> {
        let event: pb::ChildEvent =
            decode_frame(&frame).map_err(|error| format!("invalid payload from worker: {error}"))?;
        let mut events = Vec::new();
        if let Some(pb::child_event::Kind::Print(print)) = &event.kind {
            push_print(print, &mut events);
        } else {
            events.push(convert_event(event.kind).map_err(|error| match error {
                EventError::OverBudget => "worker event values exceed the host-memory budget".to_owned(),
                EventError::Invalid(message) => format!("invalid payload from worker: {message}"),
            })?);
        }
        Ok(DecodedFrame {
            events,
            session_id: event.session_id.map(Into::into),
            feed_execution_micros: event.feed_execution_micros,
            max_feed_duration_micros: event.max_feed_duration_micros,
            max_turn_duration_micros: event.max_turn_duration_micros,
            max_suspensions: event.max_suspensions,
            max_total_sleep_micros: event.max_total_sleep_micros,
        })
    }
}

bindings::export!(Codec with_types_in bindings);
