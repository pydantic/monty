//! Encoding a session's opening `Configure` together with its first request.

use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType, encode_key, encode_varint, encoded_len_varint, key_len},
};

use crate::{budgeted_prost::encoding, decode_budget, pb};

/// `monty.v1.ClientRequest.configure`'s field number.
const CONFIGURE_TAG: u32 = 1;
/// `monty.v1.Configure.feed`'s field number.
const FEED_TAG: u32 = 15;
/// `monty.v1.Configure.load`'s field number.
const LOAD_TAG: u32 = 16;
/// `monty.v1.ClientRequest.trace_parent`'s field number.
const TRACE_PARENT_TAG: u32 = 20;

/// A `Configure` request carrying a session's first `Feed` / `Load` as `first`,
/// encoded from borrowed messages so that neither is cloned.
///
/// Encode-only: decoding into it fails. `configure.first` must be unset, or the
/// frame would carry two first requests, which protobuf merges.
#[derive(Debug)]
pub struct OpeningRequest<'a> {
    configure: &'a pb::Configure,
    first: First<'a>,
    trace_parent: Option<&'a str>,
}

/// The borrowed first request.
#[derive(Debug)]
enum First<'a> {
    Feed(&'a pb::Feed),
    Load(&'a pb::Load),
}

impl<'a> OpeningRequest<'a> {
    /// Pairs `configure` with `request`, keeping the request's `trace_parent`;
    /// `None` unless `request` is a `Feed` or `Load`, the only first requests.
    #[must_use]
    pub fn new(configure: &'a pb::Configure, request: &'a pb::ClientRequest) -> Option<Self> {
        let first = match &request.kind {
            Some(pb::client_request::Kind::Feed(feed)) => First::Feed(feed),
            Some(pb::client_request::Kind::Load(load)) => First::Load(load),
            _ => return None,
        };
        Some(Self {
            configure,
            first,
            trace_parent: request.trace_parent.as_deref(),
        })
    }

    /// The encoded length of the `Configure` with `first` set.
    fn configure_len(&self) -> usize {
        let first = match self.first {
            First::Feed(feed) => encoding::message::encoded_len(FEED_TAG, feed),
            First::Load(load) => encoding::message::encoded_len(LOAD_TAG, load),
        };
        self.configure.encoded_len() + first
    }
}

impl Message for OpeningRequest<'_> {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        debug_assert!(
            self.configure.first.is_none(),
            "the Configure already carries a request"
        );
        // a message's fields may be written in any order, so appending `first`
        // encodes the `Configure` as if it were set
        encode_key(CONFIGURE_TAG, WireType::LengthDelimited, buf);
        encode_varint(self.configure_len() as u64, buf);
        self.configure.encode_raw(buf);
        match self.first {
            First::Feed(feed) => encoding::message::encode(FEED_TAG, feed, buf),
            First::Load(load) => encoding::message::encode(LOAD_TAG, load, buf),
        }
        if let Some(trace_parent) = self.trace_parent {
            encode_key(TRACE_PARENT_TAG, WireType::LengthDelimited, buf);
            encode_varint(trace_parent.len() as u64, buf);
            buf.put_slice(trace_parent.as_bytes());
        }
    }

    fn encoded_len(&self) -> usize {
        let configure_len = self.configure_len();
        key_len(CONFIGURE_TAG)
            + encoded_len_varint(configure_len as u64)
            + configure_len
            + self.trace_parent.map_or(0, |trace_parent| {
                key_len(TRACE_PARENT_TAG) + encoded_len_varint(trace_parent.len() as u64) + trace_parent.len()
            })
    }

    fn merge_field(
        &mut self,
        _tag: u32,
        _wire_type: WireType,
        _buf: &mut impl Buf,
        _ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        Err(decode_budget::error("OpeningRequest is encode-only"))
    }

    fn clear(&mut self) {}
}
