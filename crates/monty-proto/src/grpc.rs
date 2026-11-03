//! The gRPC transport's shared vocabulary: the session RPC's path, the
//! opaque-bytes codec both ends frame messages with, and the status a server
//! ends a stream with when it drops a session by policy.
//!
//! The RPC carries the same `ParentRequest` / `ChildEvent` messages as the
//! stdio transport, one per gRPC message. Both ends keep the payload opaque at
//! the tonic layer — [`BytesCodec`] moves `Bytes` — and decode it themselves
//! through [`decode_frame_with_budget`](crate::decode_frame_with_budget), so
//! the allocation budget every other transport applies holds here too and a
//! forwarding server never has to decode what it relays. There is no generated
//! service: a client calls [`SESSION_PATH`] through `tonic::client::Grpc`.

use bytes::{Buf, BufMut, Bytes};
use strum::{Display, EnumString, IntoStaticStr};
use tonic::{
    Code, Status,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::MetadataValue,
};

/// Path of the one RPC: `MontySession.Session`, a bidirectional stream per
/// session.
pub const SESSION_PATH: &str = "/monty.v1.MontySession/Session";

/// Trailer metadata key naming why a server ended a stream, a [`Cause`].
pub const CAUSE_METADATA_KEY: &str = "monty-cause";

/// Why a server ended a session's stream, or refused to open one. Rides on the
/// [`CAUSE_METADATA_KEY`] trailer beside the canonical gRPC code so a client
/// can act on the reason rather than parse the message. Its string form,
/// the variant in `snake_case`, is the trailer value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Cause {
    /// No request arrived within the server's idle timeout.
    IdleTimeout,
    /// The session outlived the server's lifetime limit.
    SessionTimeout,
    /// One turn exceeded the server's wall-clock cap.
    TurnTimeout,
    /// The worker exceeded its memory limit; the server ended the session.
    OutOfMemory,
    /// The server is draining and the idle session had nothing to hand back.
    ServerShutdown,
    /// The server's session capacity is full.
    Capacity,
    /// The caller already holds its maximum number of sessions.
    CallerCap,
    /// The caller presented no acceptable credentials.
    Unauthenticated,
    /// No worker could be reached to serve the session.
    WorkerUnavailable,
    /// The client broke the protocol (a malformed frame, a second request mid-turn).
    ProtocolViolation,
}

impl Cause {
    /// The canonical gRPC code paired with this cause.
    #[must_use]
    pub fn code(self) -> Code {
        match self {
            Self::IdleTimeout | Self::SessionTimeout | Self::TurnTimeout => Code::DeadlineExceeded,
            Self::OutOfMemory | Self::Capacity | Self::CallerCap => Code::ResourceExhausted,
            Self::ServerShutdown | Self::WorkerUnavailable => Code::Unavailable,
            Self::Unauthenticated => Code::Unauthenticated,
            Self::ProtocolViolation => Code::InvalidArgument,
        }
    }

    /// The trailer value: the variant in `snake_case`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
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
