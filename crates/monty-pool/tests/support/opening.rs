//! How the mock servers present a `Configure` carrying a first request to a
//! script: as the `Configure` and then the request alone, so the script
//! answers the two in turn. An `Ok` to the `Configure` is held back and merged
//! into the request's reply, as a real worker sends one reply for both; any
//! other answer is the refusal and drops the request.

use std::mem;

use bytes::Bytes;
use monty_proto::{decode_frame, encode_to_capped_vec, pb};

/// The split state of one session.
#[derive(Default)]
pub struct Opening {
    /// The request behind a handed-out `Configure`, for the next `read`.
    request: Option<Bytes>,
    /// Set from handing out the `Configure` until the script answers it.
    awaiting_reply: bool,
    /// The `Configure`'s `Ok`, whose fields the request's reply takes on.
    ack: Option<pb::WorkerEvent>,
}

impl Opening {
    /// The request held back behind a `Configure`, if one is pending.
    pub fn take_pending(&mut self) -> Option<Bytes> {
        self.request.take()
    }

    /// What the script reads for `message`: a `Configure` carrying a first
    /// request is returned without it, the request held for the next read.
    pub fn split(&mut self, message: Bytes) -> Bytes {
        let mut request = decode_frame::<pb::ClientRequest>(message.as_ref()).expect("decode request");
        let Some(pb::client_request::Kind::Configure(configure)) = &mut request.kind else {
            return message;
        };
        let first = match configure.first.take() {
            Some(pb::configure::First::Feed(feed)) => pb::client_request::Kind::Feed(feed),
            Some(pb::configure::First::Load(load)) => pb::client_request::Kind::Load(load),
            None => return message,
        };
        let first = pb::ClientRequest {
            kind: Some(first),
            ..pb::ClientRequest::default()
        };
        self.request = Some(encode_to_capped_vec(&first).expect("encode request").into());
        self.awaiting_reply = true;
        encode_to_capped_vec(&request).expect("encode Configure").into()
    }

    /// What goes on the wire for the script's event `body`; `None` when it is
    /// the `Configure`'s `Ok`, held back for the request's reply.
    pub fn reply(&mut self, body: Bytes) -> Option<Bytes> {
        if !self.awaiting_reply && self.ack.is_none() {
            return Some(body);
        }
        let mut event = decode_frame::<pb::WorkerEvent>(body.as_ref()).expect("decode event");
        if matches!(
            event.kind,
            Some(
                pb::worker_event::Kind::Print(_)
                    | pb::worker_event::Kind::Ping(_)
                    | pb::worker_event::Kind::StreamedCalls(_)
            )
        ) {
            // not a reply: sent as written, since re-encoding drops unknown fields
            return Some(body);
        }
        if mem::take(&mut self.awaiting_reply) {
            if matches!(event.kind, Some(pb::worker_event::Kind::Ok(_))) {
                self.ack = Some(event);
                return None;
            }
            // the `Configure` was refused, so the request behind it never runs
            self.request = None;
        } else if let Some(ack) = self.ack.take() {
            event.execution_clock = event.execution_clock.or(ack.execution_clock);
            event.session_id = event.session_id.or(ack.session_id);
            event.max_suspensions = event.max_suspensions.or(ack.max_suspensions);
            event.max_feed_duration_micros = event.max_feed_duration_micros.or(ack.max_feed_duration_micros);
            event.max_turn_duration_micros = event.max_turn_duration_micros.or(ack.max_turn_duration_micros);
            event.max_total_sleep_micros = event.max_total_sleep_micros.or(ack.max_total_sleep_micros);
        }
        Some(encode_to_capped_vec(&event).expect("encode event").into())
    }
}
