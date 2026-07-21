use std::io::{self, Read};

use monty_proto::{FrameError, FrameReader, pb, write_frame};

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
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![],
            skip_type_check: false,
        })),
    }
}

#[test]
fn frames_round_trip() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &feed()).unwrap();
    write_frame(
        &mut buf,
        &pb::ChildEvent {
            kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
            ..Default::default()
        },
    )
    .unwrap();

    let mut reader = FrameReader::new(buf.as_slice());
    let req: pb::ParentRequest = reader.read().unwrap().expect("first frame");
    assert_eq!(req, feed());
    let event: pb::ChildEvent = reader.read().unwrap().expect("second frame");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Ok(_))));
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
