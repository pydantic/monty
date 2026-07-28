//! Cheap frame classification for forwarding servers.
//!
//! A relay that bridges protocol frames between a client and a child (e.g.
//! `monty-server`) must know *which kind* of request/event a frame carries —
//! to spot turn-enders and to intercept requests during drain — but decoding
//! whole frames just for that would validate and materialize payloads of up
//! to [`crate::MAX_FRAME_LEN`] bytes per frame. These helpers walk only the
//! field keys, skipping payloads by their length, so classification is
//! O(field count) regardless of payload size and no payload byte is ever
//! examined.
//!
//! Classification must agree with what a prost endpoint decodes from the
//! same bytes — a frame the relay reads one way and the client another is a
//! desync a hostile child could mint deliberately. So the walk mirrors
//! prost's `merge` semantics exactly: the *last* oneof arm wins, values are
//! skipped with the same routine prost uses for unknown fields, and any
//! frame prost would reject (wire-type mismatch on an arm, truncated value)
//! reports `None`.
//!
//! The tag constants below duplicate the oneof numbering in
//! `proto/monty/v1/monty.proto`; `tests/frame.rs` pins them to the generated
//! code, exhaustively, so adding an arm without a tag here fails to compile.
//! An unrecognised tag is always reported as `None`, which callers must treat
//! as "opaque — forward verbatim, never intercept".

use std::ops::RangeInclusive;

use prost::encoding::{DecodeContext, WireType, decode_key, skip_field};

/// `ChildEvent` oneof tag for `Print` — the only non-turn-ending event.
pub const CHILD_EVENT_PRINT: u32 = 1;
/// `ChildEvent` oneof tag for `FunctionCall`.
pub const CHILD_EVENT_FUNCTION_CALL: u32 = 2;
/// `ChildEvent` oneof tag for `OsCall`.
pub const CHILD_EVENT_OS_CALL: u32 = 3;
/// `ChildEvent` oneof tag for `NameLookup`.
pub const CHILD_EVENT_NAME_LOOKUP: u32 = 4;
/// `ChildEvent` oneof tag for `ResolveFutures`.
pub const CHILD_EVENT_RESOLVE_FUTURES: u32 = 5;
/// `ChildEvent` oneof tag for `Complete`.
pub const CHILD_EVENT_COMPLETE: u32 = 6;
/// `ChildEvent` oneof tag for `Error`.
pub const CHILD_EVENT_ERROR: u32 = 7;
/// `ChildEvent` oneof tag for `TypingError`.
pub const CHILD_EVENT_TYPING_ERROR: u32 = 8;
/// `ChildEvent` oneof tag for `DumpResult`.
pub const CHILD_EVENT_DUMP_RESULT: u32 = 9;
/// `ChildEvent` oneof tag for `Ok`.
pub const CHILD_EVENT_OK: u32 = 10;
/// `ChildEvent` oneof tag for `FatalError`.
pub const CHILD_EVENT_FATAL_ERROR: u32 = 11;
/// `ChildEvent` oneof tag for `ShutdownDump`.
pub const CHILD_EVENT_SHUTDOWN: u32 = 12;

/// `ParentRequest` oneof tag for `Configure`.
pub const PARENT_REQUEST_CONFIGURE: u32 = 1;
/// `ParentRequest` oneof tag for `InstallDependencies`.
pub const PARENT_REQUEST_INSTALL_DEPENDENCIES: u32 = 2;
/// `ParentRequest` oneof tag for `Feed`.
pub const PARENT_REQUEST_FEED: u32 = 3;
/// `ParentRequest` oneof tag for `ResumeCall`.
pub const PARENT_REQUEST_RESUME_CALL: u32 = 4;
/// `ParentRequest` oneof tag for `ResumeNameLookup`.
pub const PARENT_REQUEST_RESUME_NAME_LOOKUP: u32 = 5;
/// `ParentRequest` oneof tag for `ResumeFutures`.
pub const PARENT_REQUEST_RESUME_FUTURES: u32 = 6;
/// `ParentRequest` oneof tag for `Dump`.
pub const PARENT_REQUEST_DUMP: u32 = 7;
/// `ParentRequest` oneof tag for `Load`.
pub const PARENT_REQUEST_LOAD: u32 = 8;
/// `ParentRequest` oneof tag for `Reset`.
pub const PARENT_REQUEST_RESET: u32 = 9;
/// `ParentRequest` oneof tag for `Shutdown`.
pub const PARENT_REQUEST_SHUTDOWN: u32 = 10;

/// Field numbers reserved for `ParentRequest.kind` arms. The message's own
/// fields start at 20 (`trace_parent` today), mirroring `ChildEvent`, so new
/// arms land inside this range and need no change here.
const PARENT_REQUEST_ONEOF: RangeInclusive<u32> = 1..=19;
/// Field numbers reserved for `ChildEvent.kind` arms. The message's own
/// fields start at 20 (see the schema), so new arms land inside this range
/// and need no change here.
const CHILD_EVENT_ONEOF: RangeInclusive<u32> = 1..=19;

/// Field number of the encoded `ParentRequest.kind` oneof arm, or `None` for
/// an empty, malformed, or unknown-arm frame.
///
/// Callers must treat `None` as "opaque — forward verbatim, never intercept".
#[must_use]
pub fn parent_request_kind(frame: &[u8]) -> Option<u32> {
    last_oneof_arm(frame, PARENT_REQUEST_ONEOF)
}

/// Field number of the encoded `ChildEvent.kind` oneof arm, or `None` for an
/// empty, malformed, or unknown-arm frame. Message-level fields (20+) are
/// skipped without being read.
///
/// Callers must treat `None` as "opaque — forward verbatim, never intercept".
#[must_use]
pub fn child_event_kind(frame: &[u8]) -> Option<u32> {
    last_oneof_arm(frame, CHILD_EVENT_ONEOF)
}

/// Walks every field key in `frame` and returns the last oneof arm, matching
/// prost's merge semantics (each occurrence of a oneof field replaces the
/// previous one) so a relay classifying by peek and a client decoding with
/// prost can never disagree about a frame that decodes.
///
/// Walking, rather than reading the first key, is what makes this safe
/// against frames the *sender* laid out differently. Prost orders fields by
/// tag, so its own output always leads with the oneof arm — but the wire
/// format permits any order, and a child speaking another implementation (or
/// deliberately probing the relay) may put a message-level field first.
/// Reading one key would call such a frame opaque while the endpoint decoded
/// it normally: precisely the classification desync this module must avoid.
///
/// Values are skipped with prost's own [`skip_field`] — the same routine its
/// decoder applies to unknown fields — and an arm that is not
/// length-delimited (which fails prost's decode) is `None` here too. The
/// guarantee is one-directional: every frame that *decodes* classifies
/// identically; a frame prost would reject may still report an arm (e.g. a
/// wire-type mismatch on a non-oneof field peek doesn't model), which is
/// harmless because the endpoint rejects the frame itself.
fn last_oneof_arm(frame: &[u8], oneof: RangeInclusive<u32>) -> Option<u32> {
    let mut buf = frame;
    let mut arm = None;
    while !buf.is_empty() {
        let (field, wire_type) = decode_key(&mut buf).ok()?;
        if oneof.contains(&field) {
            if wire_type != WireType::LengthDelimited {
                return None;
            }
            arm = Some(field);
        }
        skip_field(wire_type, field, &mut buf, DecodeContext::default()).ok()?;
    }
    arm
}
