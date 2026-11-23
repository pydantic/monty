use std::io::{self, Read};

use monty_proto::{
    FrameError, FrameReader, OpeningRequest, TracedRequest, Traceparent, WireFunctionCall, decode_frame,
    encode_to_capped_vec, pb,
    pb::{client_request::Kind as RequestKind, worker_event::Kind as EventKind},
    write_frame,
};
use prost::Message;

/// A reader that returns at most one byte per `read` call, exercising the
/// partial-read loop in the frame reader.
struct OneByteReader<R: Read>(R);

impl<R: Read> Read for OneByteReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(1);
        self.0.read(&mut buf[..len])
    }
}

fn feed() -> pb::ClientRequest {
    pb::ClientRequest {
        kind: Some(RequestKind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
            async_functions: Vec::new().into(),
            stream_async_calls: false,
        })),
        traceparent: None,
    }
}

/// An `OpeningRequest` encodes exactly as a `Configure` request whose `first`
/// is set, for each request that can be first.
#[test]
fn an_opening_request_encodes_as_configure_with_first() {
    let load = pb::ClientRequest {
        kind: Some(RequestKind::Load(pb::Load {
            state: b"dump".to_vec().into(),
        })),
        traceparent: None,
    };
    for request in [feed(), load] {
        let request = pb::ClientRequest {
            traceparent: Some("00-trace".to_owned()),
            ..request
        };
        let configure = pb::Configure {
            script_name: "main.py".to_owned(),
            ..Default::default()
        };
        let opening = OpeningRequest::new(&configure, &request).expect("a first request");
        let first = match request.kind.clone() {
            Some(RequestKind::Feed(feed)) => pb::configure::First::Feed(feed),
            Some(RequestKind::Load(load)) => pb::configure::First::Load(load),
            other => panic!("not a first request: {other:?}"),
        };
        let expected = pb::ClientRequest {
            kind: Some(RequestKind::Configure(pb::Configure {
                first: Some(first),
                ..configure.clone()
            })),
            traceparent: request.traceparent.clone(),
        };
        assert_eq!(
            encode_to_capped_vec(&opening).unwrap(),
            encode_to_capped_vec(&expected).unwrap()
        );
    }
    let dump = pb::ClientRequest {
        kind: Some(RequestKind::Dump(pb::Dump {})),
        traceparent: None,
    };
    assert!(OpeningRequest::new(&pb::Configure::default(), &dump).is_none());
}

const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// A replacement `traceparent` is the only one sent, on a plain request and
/// on one carried by its session's `Configure`.
#[test]
fn a_replacement_traceparent_is_sent_in_place_of_the_requests() {
    let replacement = Traceparent::parse(TRACEPARENT).unwrap();
    for own in [None, Some("00-caller".to_owned())] {
        let request = pb::ClientRequest {
            traceparent: own,
            ..feed()
        };
        let expected = pb::ClientRequest {
            traceparent: Some(TRACEPARENT.to_owned()),
            ..feed()
        };
        assert_eq!(
            encode_to_capped_vec(&TracedRequest::new(&request, replacement)).unwrap(),
            encode_to_capped_vec(&expected).unwrap()
        );
        assert_eq!(
            expected.encoded_len(),
            feed().encoded_len() + Traceparent::REQUEST_FIELD_LEN
        );
        let configure = pb::Configure::default();
        let opening = OpeningRequest::new(&configure, &request)
            .unwrap()
            .with_traceparent(replacement);
        let sent: pb::ClientRequest = decode_frame(&encode_to_capped_vec(&opening).unwrap()).unwrap();
        assert_eq!(sent.traceparent.as_deref(), Some(TRACEPARENT));
    }
}

/// Trace context from a peer is advisory: a malformed value reads as absent
/// on every field that carries one, and never fails the decode.
#[test]
fn a_malformed_traceparent_decodes_as_absent() {
    let malformed = [
        "",
        "00-trace",
        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
        "00-4bf92f3577b34da6a3ce929d0e0e473é-00f067aa0ba902b7-1",
    ];
    for value in malformed.into_iter().map(str::to_owned).map(Some).chain([None]) {
        let event = pb::WorkerEvent {
            kind: Some(EventKind::Ok(pb::Ok {})),
            traceparent: value.clone(),
            ..Default::default()
        };
        let event: pb::WorkerEvent = decode_frame(&encode_to_capped_vec(&event).unwrap()).unwrap();
        assert_eq!(event.parsed_traceparent(), None, "{value:?}");
        let request = pb::ClientRequest {
            traceparent: value.clone(),
            ..feed()
        };
        assert_eq!(request.parsed_traceparent(), None, "{value:?}");
        // field 9 of `FunctionCall`, as a streamed call carries it
        let mut call = vec![0x4a, u8::try_from(value.as_ref().map_or(0, String::len)).unwrap()];
        call.extend(value.iter().flat_map(|value| value.bytes()));
        let call: WireFunctionCall = decode_frame(&call).unwrap();
        assert_eq!(call.traceparent, None, "{value:?}");
    }
}

/// A well-formed `traceparent` round-trips on an event and on a streamed call.
#[test]
fn a_traceparent_round_trips() {
    let traceparent = Traceparent::parse(TRACEPARENT).unwrap();
    assert_eq!(
        (traceparent.trace_id(), traceparent.span_id(), traceparent.flags()),
        (0x4bf9_2f35_77b3_4da6_a3ce_929d_0e0e_4736, 0x00f0_67aa_0ba9_02b7, 1)
    );
    assert_eq!(
        Traceparent::new(traceparent.trace_id(), traceparent.span_id(), 1),
        Some(traceparent)
    );
    assert_eq!(Traceparent::new(0, 1, 1), None);
    assert_eq!(Traceparent::new(1, 0, 1), None);
    assert_eq!(
        Traceparent::new(u128::MAX, u64::MAX, 0xff).unwrap().as_str(),
        "00-ffffffffffffffffffffffffffffffff-ffffffffffffffff-ff"
    );
    assert_eq!(
        Traceparent::new(1, 2, 0).unwrap().as_str(),
        "00-00000000000000000000000000000001-0000000000000002-00"
    );
    let call = WireFunctionCall {
        function_name: "fetch".to_owned(),
        traceparent: Some(traceparent),
        ..Default::default()
    };
    let event = pb::WorkerEvent {
        kind: Some(EventKind::StreamedCalls(pb::StreamedCalls {
            calls: vec![call.clone()].into(),
        })),
        traceparent: Some(TRACEPARENT.to_owned()),
        ..Default::default()
    };
    let back: pb::WorkerEvent = decode_frame(&encode_to_capped_vec(&event).unwrap()).unwrap();
    assert_eq!(back.parsed_traceparent(), Some(traceparent));
    let Some(EventKind::StreamedCalls(streamed)) = back.kind else {
        panic!("expected streamed calls");
    };
    assert_eq!(streamed.calls.into_inner(), vec![call]);
}

#[test]
fn frames_round_trip() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    write_frame(
        &mut buf,
        &pb::WorkerEvent {
            kind: Some(EventKind::Ok(pb::Ok {})),
            ..Default::default()
        },
    )
    .unwrap();

    let mut reader = FrameReader::new(buf.as_slice());
    let req: pb::ClientRequest = reader.read().unwrap().expect("first frame");
    assert_eq!(req, feed());
    let event: pb::WorkerEvent = reader.read().unwrap().expect("second frame");
    assert!(matches!(event.kind, Some(EventKind::Ok(_))));
    // clean EOF at the frame boundary
    assert!(reader.read::<pb::WorkerEvent>().unwrap().is_none());
}

#[test]
fn frames_round_trip_through_chunked_reads() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    let mut reader = FrameReader::new(OneByteReader(buf.as_slice()));
    let req: pb::ClientRequest = reader.read().unwrap().expect("frame");
    assert_eq!(req, feed());
}

#[test]
fn oversized_frame_is_rejected_without_allocating() {
    // length prefix claims 4GiB-1; the reader must refuse before allocating
    let bytes = [0xFF, 0xFF, 0xFF, 0xFF];
    let mut reader = FrameReader::with_max_frame_len(bytes.as_slice(), 1024);
    let err = reader.read::<pb::ClientRequest>().expect_err("oversized frame");
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
    let err = reader.read::<pb::ClientRequest>().expect_err("truncated prefix");
    assert!(matches!(err, FrameError::Truncated));
}

#[test]
fn truncated_body_is_an_error() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    buf.truncate(buf.len() - 1); // drop the last body byte
    let mut reader = FrameReader::new(buf.as_slice());
    let err = reader.read::<pb::ClientRequest>().expect_err("truncated body");
    assert!(matches!(err, FrameError::Truncated));
}

#[test]
fn garbage_body_is_a_decode_error() {
    let body = [0xFFu8; 8]; // not a valid Request message
    let mut buf = Vec::new();
    buf.extend_from_slice(&8u32.to_le_bytes());
    buf.extend_from_slice(&body);
    let mut reader = FrameReader::new(buf.as_slice());
    let err = reader.read::<pb::ClientRequest>().expect_err("garbage body");
    assert!(matches!(err, FrameError::Decode(_)));
}
