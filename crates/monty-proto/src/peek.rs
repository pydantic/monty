//! Cheap frame classification for forwarding servers.
//!
//! A relay that bridges protocol frames between a client and a child (e.g.
//! `monty-server`) must know *which kind* of request/event a frame carries —
//! to spot turn-enders and to intercept requests during drain — but decoding
//! whole frames just for that would validate and materialize payloads of up
//! to [`crate::MAX_FRAME_LEN`] bytes per frame. These helpers read only the
//! leading field keys, so classification is O(header bytes) regardless of
//! payload size and the payload itself is never touched.
//!
//! The tag constants below duplicate the oneof numbering in
//! `proto/monty/v1/monty.proto`; `tests/frame.rs` pins them to the generated
//! code, exhaustively, so adding an arm without a tag here fails to compile.
//! An unrecognised tag is always reported as `None`, which callers must treat
//! as "opaque — forward verbatim, never intercept".

use std::ops::RangeInclusive;

use prost::encoding::{WireType, decode_key, decode_varint};

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

/// Field numbers reserved for `ParentRequest.kind` arms. The message is
/// nothing *but* the oneof, so this mirrors the range `ChildEvent` reserves
/// and new arms land inside it without a change here.
const PARENT_REQUEST_ONEOF: RangeInclusive<u32> = 1..=19;
/// Field numbers reserved for `ChildEvent.kind` arms. The message's own
/// fields start at 20 (see the schema), so new arms land inside this range
/// and need no change here.
const CHILD_EVENT_ONEOF: RangeInclusive<u32> = 1..=19;

/// Field number of the encoded `ParentRequest.kind` oneof arm, or `None` for
/// an empty, malformed, or unknown-arm frame.
///
/// `ParentRequest` contains *only* the oneof and every arm is a message
/// (length-delimited), so the discriminant is simply the first field key.
/// Callers must treat `None` as "opaque — forward verbatim, never intercept".
#[must_use]
pub fn parent_request_kind(frame: &[u8]) -> Option<u32> {
    let mut buf = frame;
    let (field, wire_type) = decode_key(&mut buf).ok()?;
    (wire_type == WireType::LengthDelimited && PARENT_REQUEST_ONEOF.contains(&field)).then_some(field)
}

/// Field number of the encoded `ChildEvent.kind` oneof arm, or `None` for an
/// empty, malformed, or unknown-arm frame.
///
/// Unlike `ParentRequest`, `ChildEvent` carries message-level fields (20+)
/// that prost emits *before* the oneof, so this walks field keys — skipping
/// non-oneof fields by wire type — until it hits an arm. The walk touches
/// only header bytes and varints, never a payload; it is bounded by the
/// handful of known non-oneof fields plus one arm.
#[must_use]
pub fn child_event_kind(frame: &[u8]) -> Option<u32> {
    let mut buf = frame;
    while !buf.is_empty() {
        let (field, wire_type) = decode_key(&mut buf).ok()?;
        if CHILD_EVENT_ONEOF.contains(&field) {
            return (wire_type == WireType::LengthDelimited).then_some(field);
        }
        // a non-oneof field (timing/script-name today, anything additive
        // tomorrow): skip its value and keep walking
        match wire_type {
            WireType::Varint => {
                decode_varint(&mut buf).ok()?;
            }
            WireType::LengthDelimited => {
                let len = usize::try_from(decode_varint(&mut buf).ok()?).ok()?;
                buf = buf.get(len..)?;
            }
            WireType::ThirtyTwoBit => buf = buf.get(4..)?,
            WireType::SixtyFourBit => buf = buf.get(8..)?,
            WireType::StartGroup | WireType::EndGroup => return None,
        }
    }
    None
}
