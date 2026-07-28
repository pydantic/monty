use std::io::{self, Read};

use monty_proto::{
    FrameError, FrameReader, WireFunctionCall, encode_to_capped_vec, pb,
    pb::{child_event::Kind as EventKind, parent_request::Kind as RequestKind},
    peek, write_frame,
};
use prost::{
    Message,
    encoding::{WireType, encode_key, encode_varint},
};

/// A reader that returns at most one byte per `read` call, exercising the
/// partial-read loop in the frame reader.
struct OneByteReader<R: Read>(R);

impl<R: Read> Read for OneByteReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(1);
        self.0.read(&mut buf[..len])
    }
}

fn feed() -> pb::ParentRequest {
    pb::ParentRequest {
        kind: Some(RequestKind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![],
            skip_type_check: false,
        })),
        trace_parent: None,
    }
}

#[test]
fn frames_round_trip() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    write_frame(
        &mut buf,
        &pb::ChildEvent {
            kind: Some(EventKind::Ok(pb::Ok {})),
            ..Default::default()
        },
    )
    .unwrap();

    let mut reader = FrameReader::new(buf.as_slice());
    let req: pb::ParentRequest = reader.read().unwrap().expect("first frame");
    assert_eq!(req, feed());
    let event: pb::ChildEvent = reader.read().unwrap().expect("second frame");
    assert!(matches!(event.kind, Some(EventKind::Ok(_))));
    // clean EOF at the frame boundary
    assert!(reader.read::<pb::ChildEvent>().unwrap().is_none());
}

#[test]
fn frames_round_trip_through_chunked_reads() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    let mut reader = FrameReader::new(OneByteReader(buf.as_slice()));
    let req: pb::ParentRequest = reader.read().unwrap().expect("frame");
    assert_eq!(req, feed());
}

#[test]
fn oversized_frame_is_rejected_without_allocating() {
    // length prefix claims 4GiB-1; the reader must refuse before allocating
    let bytes = [0xFF, 0xFF, 0xFF, 0xFF];
    let mut reader = FrameReader::with_max_frame_len(bytes.as_slice(), 1024);
    let err = reader.read::<pb::ParentRequest>().expect_err("oversized frame");
    assert!(matches!(
        err,
        FrameError::FrameTooLarge {
            len: 0xFFFF_FFFF,
            max: 1024
        }
    ));
}

#[test]
fn truncated_length_prefix_is_an_error() {
    let bytes = [1, 0]; // 2 of 4 length bytes, then EOF
    let mut reader = FrameReader::new(bytes.as_slice());
    let err = reader.read::<pb::ParentRequest>().expect_err("truncated prefix");
    assert!(matches!(err, FrameError::Truncated));
}

#[test]
fn truncated_body_is_an_error() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    buf.truncate(buf.len() - 1); // drop the last body byte
    let mut reader = FrameReader::new(buf.as_slice());
    let err = reader.read::<pb::ParentRequest>().expect_err("truncated body");
    assert!(matches!(err, FrameError::Truncated));
}

#[test]
fn garbage_body_is_a_decode_error() {
    let body = [0xFFu8; 8]; // not a valid Request message
    let mut buf = Vec::new();
    buf.extend_from_slice(&8u32.to_le_bytes());
    buf.extend_from_slice(&body);
    let mut reader = FrameReader::new(buf.as_slice());
    let err = reader.read::<pb::ParentRequest>().expect_err("garbage body");
    assert!(matches!(err, FrameError::Decode(_)));
}

// === peek: classify frames without decoding payloads ===
//
// `peek`'s tag constants duplicate the schema's oneof numbering, so these
// tests are what keeps the copy honest. Both expectations are expressed as an
// exhaustive `match` on the generated `Kind` enum with no `_` arm: adding an
// arm to the schema then fails to *compile* here, forcing whoever adds it to
// state its tag — and, for a child event, to decide whether it streams (like
// `Print`) or ends a turn, which forwarding servers classify by tag alone.

/// Encodes a `ParentRequest` with the given kind plus the message-level
/// `trace_parent` set, so classification has a non-oneof field to step over.
/// Returns raw frame bytes (no length prefix — the WebSocket message form).
fn request_frame(kind: RequestKind) -> Vec<u8> {
    encode_to_capped_vec(&pb::ParentRequest {
        kind: Some(kind),
        trace_parent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned()),
    })
    .unwrap()
}

/// Encodes a `ChildEvent` with the given kind plus the message-level timing
/// and script-name fields set, so classification has non-oneof fields (20–22)
/// to step over.
fn event_frame(kind: EventKind) -> Vec<u8> {
    encode_to_capped_vec(&pb::ChildEvent {
        kind: Some(kind),
        total_execution_micros: 123,
        max_duration_micros: Some(456),
        restored_script_name: Some("main.py".to_owned()),
    })
    .unwrap()
}

/// The tag `peek` must report for each request arm. Exhaustive by design —
/// see the note above.
fn expected_request_tag(kind: &RequestKind) -> u32 {
    match kind {
        RequestKind::Configure(_) => peek::PARENT_REQUEST_CONFIGURE,
        RequestKind::InstallDependencies(_) => peek::PARENT_REQUEST_INSTALL_DEPENDENCIES,
        RequestKind::Feed(_) => peek::PARENT_REQUEST_FEED,
        RequestKind::ResumeCall(_) => peek::PARENT_REQUEST_RESUME_CALL,
        RequestKind::ResumeNameLookup(_) => peek::PARENT_REQUEST_RESUME_NAME_LOOKUP,
        RequestKind::ResumeFutures(_) => peek::PARENT_REQUEST_RESUME_FUTURES,
        RequestKind::Dump(_) => peek::PARENT_REQUEST_DUMP,
        RequestKind::Load(_) => peek::PARENT_REQUEST_LOAD,
        RequestKind::Reset(_) => peek::PARENT_REQUEST_RESET,
        RequestKind::Shutdown(_) => peek::PARENT_REQUEST_SHUTDOWN,
    }
}

/// The tag `peek` must report for each child-event arm. Exhaustive by design —
/// see the note above. Arms without a `peek` constant are ones no forwarding
/// server special-cases; they still need a tag so a renumber is caught.
fn expected_event_tag(kind: &EventKind) -> u32 {
    match kind {
        EventKind::Print(_) => peek::CHILD_EVENT_PRINT,
        EventKind::FunctionCall(_) => peek::CHILD_EVENT_FUNCTION_CALL,
        EventKind::OsCall(_) => peek::CHILD_EVENT_OS_CALL,
        EventKind::NameLookup(_) => peek::CHILD_EVENT_NAME_LOOKUP,
        EventKind::ResolveFutures(_) => peek::CHILD_EVENT_RESOLVE_FUTURES,
        EventKind::Complete(_) => peek::CHILD_EVENT_COMPLETE,
        EventKind::Error(_) => peek::CHILD_EVENT_ERROR,
        EventKind::TypingError(_) => peek::CHILD_EVENT_TYPING_ERROR,
        EventKind::DumpResult(_) => peek::CHILD_EVENT_DUMP_RESULT,
        EventKind::Ok(_) => peek::CHILD_EVENT_OK,
        EventKind::FatalError(_) => peek::CHILD_EVENT_FATAL_ERROR,
        EventKind::Shutdown(_) => peek::CHILD_EVENT_SHUTDOWN,
    }
}

#[test]
fn peek_classifies_every_parent_request_kind() {
    let kinds = [
        RequestKind::Configure(pb::Configure::default()),
        RequestKind::InstallDependencies(pb::InstallDependencies::default()),
        RequestKind::Feed(pb::Feed::default()),
        RequestKind::ResumeCall(pb::ResumeCall::default()),
        RequestKind::ResumeNameLookup(pb::ResumeNameLookup::default()),
        RequestKind::ResumeFutures(pb::ResumeFutures::default()),
        RequestKind::Dump(pb::Dump {}),
        RequestKind::Load(pb::Load::default()),
        RequestKind::Reset(pb::Reset {}),
        RequestKind::Shutdown(pb::Shutdown {}),
    ];
    // every arm covered: one case per tag the match above can return
    assert_eq!(kinds.len(), 10);
    for kind in kinds {
        let expected = expected_request_tag(&kind);
        let frame = request_frame(kind);
        assert_eq!(peek::parent_request_kind(&frame), Some(expected));
    }
}

#[test]
fn peek_classifies_every_child_event_kind() {
    let kinds = [
        EventKind::Print(pb::Print::default()),
        EventKind::FunctionCall(WireFunctionCall::default()),
        EventKind::OsCall(pb::OsCall::default()),
        EventKind::NameLookup(pb::NameLookup::default()),
        EventKind::ResolveFutures(pb::ResolveFutures::default()),
        EventKind::Complete(pb::Complete::default()),
        EventKind::Error(pb::Error::default()),
        EventKind::TypingError(pb::TypingError::default()),
        EventKind::DumpResult(pb::DumpResult::default()),
        EventKind::Ok(pb::Ok {}),
        EventKind::FatalError(pb::FatalError::default()),
        EventKind::Shutdown(pb::ShutdownDump::default()),
    ];
    // every arm covered: one case per tag the match above can return
    assert_eq!(kinds.len(), 12);
    for kind in kinds {
        let expected = expected_event_tag(&kind);
        let frame = event_frame(kind);
        assert_eq!(peek::child_event_kind(&frame), Some(expected));
    }
}

#[test]
fn peek_tags_are_distinct_and_within_the_reserved_range() {
    // a copy-paste in the constants above would otherwise let two arms share a
    // tag, and every arm must fall inside the range `peek` scans
    let tags = [
        peek::PARENT_REQUEST_CONFIGURE,
        peek::PARENT_REQUEST_INSTALL_DEPENDENCIES,
        peek::PARENT_REQUEST_FEED,
        peek::PARENT_REQUEST_RESUME_CALL,
        peek::PARENT_REQUEST_RESUME_NAME_LOOKUP,
        peek::PARENT_REQUEST_RESUME_FUTURES,
        peek::PARENT_REQUEST_DUMP,
        peek::PARENT_REQUEST_LOAD,
        peek::PARENT_REQUEST_RESET,
        peek::PARENT_REQUEST_SHUTDOWN,
    ];
    let mut sorted = tags.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), tags.len(), "duplicate ParentRequest tag");
    assert!(tags.iter().all(|tag| (1..=19).contains(tag)));

    let tags = [
        peek::CHILD_EVENT_PRINT,
        peek::CHILD_EVENT_FUNCTION_CALL,
        peek::CHILD_EVENT_OS_CALL,
        peek::CHILD_EVENT_NAME_LOOKUP,
        peek::CHILD_EVENT_RESOLVE_FUTURES,
        peek::CHILD_EVENT_COMPLETE,
        peek::CHILD_EVENT_ERROR,
        peek::CHILD_EVENT_TYPING_ERROR,
        peek::CHILD_EVENT_DUMP_RESULT,
        peek::CHILD_EVENT_OK,
        peek::CHILD_EVENT_FATAL_ERROR,
        peek::CHILD_EVENT_SHUTDOWN,
    ];
    let mut sorted = tags.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), tags.len(), "duplicate ChildEvent tag");
    assert!(tags.iter().all(|tag| (1..=19).contains(tag)));
}

#[test]
fn peek_matches_prost_when_a_frame_carries_two_arms() {
    // protobuf message concatenation merges, and prost's oneof merge is
    // last-arm-wins — peek must agree, or a hostile child could mint a frame
    // that a peek-classifying relay and a prost-decoding client read as two
    // different kinds
    let mut frame = event_frame(EventKind::Complete(pb::Complete::default()));
    frame.extend(event_frame(EventKind::Print(pb::Print::default())));
    let decoded = pb::ChildEvent::decode(frame.as_slice()).expect("concatenated events decode");
    assert!(matches!(decoded.kind, Some(EventKind::Print(_))));
    assert_eq!(peek::child_event_kind(&frame), Some(peek::CHILD_EVENT_PRINT));

    let mut frame = request_frame(RequestKind::Feed(pb::Feed::default()));
    frame.extend(request_frame(RequestKind::Reset(pb::Reset {})));
    let decoded = pb::ParentRequest::decode(frame.as_slice()).expect("concatenated requests decode");
    assert!(matches!(decoded.kind, Some(RequestKind::Reset(_))));
    assert_eq!(peek::parent_request_kind(&frame), Some(peek::PARENT_REQUEST_RESET));
}

#[test]
fn peek_rejects_a_non_message_arm_like_prost() {
    // an in-range field with a varint wire type fails prost's oneof decode,
    // so peek reports the frame opaque rather than as that kind
    let mut frame = Vec::new();
    encode_key(peek::CHILD_EVENT_COMPLETE, WireType::Varint, &mut frame);
    encode_varint(1, &mut frame);
    assert!(pb::ChildEvent::decode(frame.as_slice()).is_err());
    assert_eq!(peek::child_event_kind(&frame), None);
    assert!(pb::ParentRequest::decode(frame.as_slice()).is_err());
    assert_eq!(peek::parent_request_kind(&frame), None);
}

#[test]
fn peek_skips_unknown_leading_fields_like_prost() {
    // a message-level field added to ParentRequest tomorrow (tag 21+, i.e.
    // beyond `trace_parent`) must not stop classification: prost skips
    // unknown fields, so peek does too
    let mut frame = Vec::new();
    encode_key(21, WireType::Varint, &mut frame);
    encode_varint(7, &mut frame);
    frame.extend(request_frame(RequestKind::Dump(pb::Dump {})));
    let decoded = pb::ParentRequest::decode(frame.as_slice()).expect("unknown field is skipped");
    assert!(matches!(decoded.kind, Some(RequestKind::Dump(_))));
    assert_eq!(peek::parent_request_kind(&frame), Some(peek::PARENT_REQUEST_DUMP));
}

/// A message-level field preceding the oneof must not hide the arm.
///
/// Prost orders fields by tag, so its own `ParentRequest` always leads with
/// the arm and `trace_parent` (tag 20) trails it — but the wire format allows
/// either order, and a sender that is not prost may lead with `trace_parent`.
/// Reading only the first key would call such a frame opaque, so a relay
/// would never intercept it, while the endpoint decoded it normally.
#[test]
fn peek_finds_an_arm_behind_a_leading_message_level_field() {
    for (kind, tag) in [
        (RequestKind::Dump(pb::Dump {}), peek::PARENT_REQUEST_DUMP),
        (RequestKind::Shutdown(pb::Shutdown {}), peek::PARENT_REQUEST_SHUTDOWN),
    ] {
        // hand-built: `trace_parent` first, then the arm
        let mut frame = Vec::new();
        encode_key(20, WireType::LengthDelimited, &mut frame);
        encode_varint(2, &mut frame);
        frame.extend_from_slice(b"tp");
        frame.extend(
            encode_to_capped_vec(&pb::ParentRequest {
                kind: Some(kind),
                trace_parent: None,
            })
            .unwrap(),
        );

        let decoded = pb::ParentRequest::decode(frame.as_slice()).expect("leading field is legal");
        assert_eq!(decoded.trace_parent.as_deref(), Some("tp"));
        assert_eq!(peek::parent_request_kind(&frame), Some(tag));
    }
}

#[test]
fn peek_reports_an_unknown_arm_as_opaque_not_misclassified() {
    // an arm added to the schema but not to `peek`: a ParentRequest tag past
    // the reserved range must read as `None` ("forward verbatim, never
    // intercept") rather than as some known kind
    let mut frame = Vec::new();
    encode_key(21, WireType::LengthDelimited, &mut frame);
    encode_varint(0, &mut frame);
    assert_eq!(peek::parent_request_kind(&frame), None);
}

#[test]
fn peek_rejects_empty_and_malformed_frames() {
    assert_eq!(peek::parent_request_kind(&[]), None);
    assert_eq!(peek::child_event_kind(&[]), None);
    // 0xFF runs off the end of the buffer as a varint key
    assert_eq!(peek::parent_request_kind(&[0xFF]), None);
    assert_eq!(peek::child_event_kind(&[0xFF]), None);
    // a frame with only non-oneof fields never finds an arm
    let no_kind = encode_to_capped_vec(&pb::ChildEvent {
        kind: None,
        total_execution_micros: 9,
        max_duration_micros: None,
        restored_script_name: None,
    })
    .unwrap();
    assert_eq!(peek::child_event_kind(&no_kind), None);
}
