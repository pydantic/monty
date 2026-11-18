use std::io::{self, Read};

use monty_proto::{
    FrameError, FrameReader, OpeningRequest, encode_to_capped_vec, pb,
    pb::{client_request::Kind as RequestKind, worker_event::Kind as EventKind},
    write_frame,
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
        trace_parent: None,
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
        trace_parent: None,
    };
    for request in [feed(), load] {
        let request = pb::ClientRequest {
            trace_parent: Some("00-trace".to_owned()),
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
            trace_parent: request.trace_parent.clone(),
        };
        assert_eq!(
            encode_to_capped_vec(&opening).unwrap(),
            encode_to_capped_vec(&expected).unwrap()
        );
    }
    let dump = pb::ClientRequest {
        kind: Some(RequestKind::Dump(pb::Dump {})),
        trace_parent: None,
    };
    assert!(OpeningRequest::new(&pb::Configure::default(), &dump).is_none());
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
