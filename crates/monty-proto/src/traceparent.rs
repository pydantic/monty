//! The W3C `traceparent` value the protocol's advisory trace-context fields carry.

use std::fmt;

use crate::pb;

/// Length of a version `00` `traceparent`: `00-<32 hex>-<16 hex>-<2 hex>`.
const LEN: usize = 55;

/// A validated W3C `traceparent` (version `00`): a span's trace ID, span ID and flags.
///
/// Peers are untrusted and the context is advisory, so [`Traceparent::parse`]
/// returns `None` for anything malformed rather than an error.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Traceparent([u8; LEN]);

impl Traceparent {
    /// Builds the value for a span; `None` when either ID is zero, which W3C forbids.
    #[must_use]
    pub fn new(trace_id: u128, span_id: u64, flags: u8) -> Option<Self> {
        Self::parse(&format!("00-{trace_id:032x}-{span_id:016x}-{flags:02x}"))
    }

    /// Parses a header value: lower-case hex, version `00`, non-zero IDs.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let bytes: [u8; LEN] = value.as_bytes().try_into().ok()?;
        let well_formed = bytes.iter().enumerate().all(|(index, byte)| match index {
            2 | 35 | 52 => *byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        });
        let parsed = Self(bytes);
        (well_formed && bytes.starts_with(b"00-") && parsed.trace_id() != 0 && parsed.span_id() != 0).then_some(parsed)
    }

    /// The header value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // validated ASCII, so this never falls back
        str::from_utf8(&self.0).unwrap_or_default()
    }

    #[must_use]
    pub fn trace_id(&self) -> u128 {
        u128::from_str_radix(&self.as_str()[3..35], 16).unwrap_or_default()
    }

    #[must_use]
    pub fn span_id(&self) -> u64 {
        u64::from_str_radix(&self.as_str()[36..52], 16).unwrap_or_default()
    }

    /// The trace flags; bit 0 is `sampled`.
    #[must_use]
    pub fn flags(&self) -> u8 {
        u8::from_str_radix(&self.as_str()[53..], 16).unwrap_or_default()
    }
}

impl fmt::Display for Traceparent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Traceparent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Traceparent({})", self.as_str())
    }
}

impl pb::ClientRequest {
    /// The caller's span for this turn; `None` when unset or malformed.
    #[must_use]
    pub fn parsed_traceparent(&self) -> Option<Traceparent> {
        self.traceparent.as_deref().and_then(Traceparent::parse)
    }
}
impl pb::WorkerEvent {
    /// The sender's span for this suspension; `None` when unset or malformed.
    #[must_use]
    pub fn parsed_traceparent(&self) -> Option<Traceparent> {
        self.traceparent.as_deref().and_then(Traceparent::parse)
    }
}
