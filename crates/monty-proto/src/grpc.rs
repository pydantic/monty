//! The gRPC transport's shared vocabulary: the session RPC's path, the
//! opaque-bytes codec both ends frame messages with, and the status a server
//! ends a stream with when it drops a session by policy (a [`Cause`]'s code
//! and trailer).
//!
//! The RPC carries the same `ClientRequest` / `WorkerEvent` messages as the
//! stdio transport, one per gRPC message. Both ends keep the payload opaque at
//! the tonic layer — [`BytesCodec`] moves `Bytes` — and decode it themselves
//! through [`decode_frame_with_budget`](crate::decode_frame_with_budget), so
//! the allocation budget every other transport applies holds here too and a
//! forwarding server never has to decode what it relays. There is no generated
//! service: a client calls [`SESSION_PATH`] through `tonic::client::Grpc`.

use bytes::{Buf, BufMut, Bytes};
use tonic::{
    Code, Status,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::MetadataValue,
};

pub use crate::cause::{CAUSE_METADATA_KEY, Cause};

/// Path of the one RPC: `MontySession.Session`, a bidirectional stream per
/// session.
pub const SESSION_PATH: &str = "/monty.v1.MontySession/Session";

impl Cause {
    /// The canonical gRPC code paired with this cause.
    #[must_use]
    pub fn code(self) -> Code {
        match self {
            Self::IdleTimeout | Self::SessionTimeout | Self::TurnTimeout => Code::DeadlineExceeded,
            Self::OutOfMemory | Self::Capacity | Self::CallerCap => Code::ResourceExhausted,
            Self::ServerShutdown | Self::WorkerUnavailable => Code::Unavailable,
            Self::Unauthenticated => Code::Unauthenticated,
            Self::PermissionDenied => Code::PermissionDenied,
            Self::ProtocolViolation => Code::InvalidArgument,
        }
    }

    /// Builds the status a server ends (or refuses) a stream with: this
    /// cause's code, `message`, and the cause itself as a trailer.
    #[must_use]
    pub fn status(self, message: impl Into<String>) -> Status {
        let mut status = Status::new(self.code(), message);
        status
            .metadata_mut()
            .insert(CAUSE_METADATA_KEY, MetadataValue::from_static(self.as_str()));
        status
    }

    /// Reads the cause a server attached to `status`, if it attached one this
    /// build knows.
    #[must_use]
    pub fn of(status: &Status) -> Option<Self> {
        status
            .metadata()
            .get(CAUSE_METADATA_KEY)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    }
}

/// A tonic codec that moves each message as opaque `Bytes`, in both
/// directions. Each end encodes before sending and decodes after receiving
/// with the frame helpers, which is what keeps the decode budget and the
/// 256 MiB cap identical across transports; tonic's own message-size limits
/// are set to the same cap by the caller as a second fence.
#[derive(Debug, Clone, Copy, Default)]
pub struct BytesCodec;

impl Codec for BytesCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = Self;
    type Decoder = Self;

    fn encoder(&mut self) -> Self::Encoder {
        Self
    }

    fn decoder(&mut self) -> Self::Decoder {
        Self
    }
}

impl Encoder for BytesCodec {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for BytesCodec {
    type Item = Bytes;
    type Error = Status;

    /// tonic hands over a buffer holding exactly one whole message, so taking
    /// everything in it is the message.
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(src.copy_to_bytes(src.remaining())))
    }
}
