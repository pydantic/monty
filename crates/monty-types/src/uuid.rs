//! The 16-byte UUID carried on the wire for class and instance identity.
//!
//! `MontyUuid` only stores, formats and parses ids — it never generates them.
//! Hosts generate uuid4 values (Python via the `uuid` crate, JS via `crypto`);
//! the worker generates ids for sandbox-defined classes/instances from raw entropy
//! via [`MontyUuid::from_random_bytes`]. Keeping generation out of this crate
//! leaves `monty-types` free of RNG dependencies on every target.

use std::fmt;

/// A 16-byte UUID identifying a host or sandbox class/instance across the
/// sandbox boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct MontyUuid([u8; 16]);

impl MontyUuid {
    /// Wraps raw bytes as-is; used when the bytes are already a valid uuid.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Builds a deterministic id from an integer — for tests and fixtures
    /// that must be reproducible; never use for real identity.
    #[must_use]
    pub const fn from_u128(v: u128) -> Self {
        Self(v.to_be_bytes())
    }

    /// Stamps the uuid4 version and RFC 4122 variant bits over caller-supplied
    /// random bytes, so any entropy source yields a well-formed uuid4.
    #[must_use]
    pub const fn from_random_bytes(mut bytes: [u8; 16]) -> Self {
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
        Self(bytes)
    }

    /// Returns the raw bytes (big-endian field order, per RFC 4122).
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses exactly 16 bytes; `None` for any other length. This is the
    /// wire-decode entry point, so it must never panic.
    #[must_use]
    pub fn try_from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; 16]>::try_from(bytes).ok().map(Self)
    }

    /// Parses the canonical hyphenated form (`8-4-4-4-12` hex digits), any
    /// case; `None` on any deviation. Used by the JS surfaces, which carry
    /// uuids as strings.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let raw = s.as_bytes();
        if raw.len() != 36 || raw[8] != b'-' || raw[13] != b'-' || raw[18] != b'-' || raw[23] != b'-' {
            return None;
        }
        let mut bytes = [0u8; 16];
        let mut out = 0;
        let mut i = 0;
        while i < 36 {
            if matches!(i, 8 | 13 | 18 | 23) {
                i += 1;
                continue;
            }
            let hi = hex_value(raw[i])?;
            let lo = hex_value(raw[i + 1])?;
            bytes[out] = (hi << 4) | lo;
            out += 1;
            i += 2;
        }
        Some(Self(bytes))
    }
}

/// Renders the canonical hyphenated lowercase form, e.g.
/// `"0d1f3c9a-5b7e-4c21-9f8a-2e6b4d0c7a13"`.
impl fmt::Display for MontyUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Decodes one ASCII hex digit; `None` for anything else.
const fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
