//! Encoding requests from borrowed parts: a session's opening `Configure` together
//! with its first request, and a request sent under another `traceparent`.

use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType, encode_key, encode_varint, encoded_len_varint, key_len},
};

use crate::{Traceparent, budgeted_prost::encoding, decode_budget, pb};

/// `monty.v1.ClientRequest.configure`'s field number.
const CONFIGURE_TAG: u32 = 1;
/// `monty.v1.Configure.feed`'s field number.
const FEED_TAG: u32 = 15;
/// `monty.v1.Configure.load`'s field number.
const LOAD_TAG: u32 = 16;
/// `monty.v1.ClientRequest.traceparent`'s field number.
const TRACEPARENT_TAG: u32 = 20;

/// A `Configure` request carrying a session's first `Feed` / `Load` as `first`,
/// encoded from borrowed messages so that neither is cloned.
///
/// Encode-only: decoding into it fails. `configure.first` must be unset, or the
/// frame would carry two first requests, which protobuf merges.
#[derive(Debug)]
pub struct OpeningRequest<'a> {
    configure: &'a pb::Configure,
    first: First<'a>,
    traceparent: Option<&'a str>,
    /// Sent in place of `traceparent` when set.
    replacement: Option<Traceparent>,
}

/// The borrowed first request.
#[derive(Debug)]
enum First<'a> {
    Feed(&'a pb::Feed),
    Load(&'a pb::Load),
}

impl<'a> OpeningRequest<'a> {
    /// Pairs `configure` with `request`, keeping the request's `traceparent`;
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
            traceparent: request.traceparent.as_deref(),
            replacement: None,
        })
    }

    /// Sends `traceparent` in place of the request's own.
    #[must_use]
    pub fn with_traceparent(mut self, traceparent: Traceparent) -> Self {
        self.replacement = Some(traceparent);
        self
    }

    fn traceparent(&self) -> Option<&str> {
        self.replacement.as_ref().map(Traceparent::as_str).or(self.traceparent)
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
        encode_traceparent(self.traceparent(), buf);
    }

    fn encoded_len(&self) -> usize {
        let configure_len = self.configure_len();
        key_len(CONFIGURE_TAG)
            + encoded_len_varint(configure_len as u64)
            + configure_len
            + traceparent_len(self.traceparent())
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

/// A request encoded with `traceparent` in place of its own, without cloning it.
///
/// Encode-only: decoding into it fails.
#[derive(Debug)]
pub struct TracedRequest<'a> {
    request: &'a pb::ClientRequest,
    traceparent: Traceparent,
}

impl<'a> TracedRequest<'a> {
    #[must_use]
    pub fn new(request: &'a pb::ClientRequest, traceparent: Traceparent) -> Self {
        Self { request, traceparent }
    }
}

impl Message for TracedRequest<'_> {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        if let Some(kind) = &self.request.kind {
            kind.encode(buf);
        }
        encode_traceparent(Some(self.traceparent.as_str()), buf);
    }

    fn encoded_len(&self) -> usize {
        self.request
            .kind
            .as_ref()
            .map_or(0, pb::client_request::Kind::encoded_len)
            + traceparent_len(Some(self.traceparent.as_str()))
    }

    fn merge_field(
        &mut self,
        _tag: u32,
        _wire_type: WireType,
        _buf: &mut impl Buf,
        _ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        Err(decode_budget::error("TracedRequest is encode-only"))
    }

    fn clear(&mut self) {}
}

/// Writes `ClientRequest.traceparent`, when there is one.
fn encode_traceparent(traceparent: Option<&str>, buf: &mut impl BufMut) {
    if let Some(traceparent) = traceparent {
        encode_key(TRACEPARENT_TAG, WireType::LengthDelimited, buf);
        encode_varint(traceparent.len() as u64, buf);
        buf.put_slice(traceparent.as_bytes());
    }
}

fn traceparent_len(traceparent: Option<&str>) -> usize {
    traceparent.map_or(0, |traceparent| {
        key_len(TRACEPARENT_TAG) + encoded_len_varint(traceparent.len() as u64) + traceparent.len()
    })
}
