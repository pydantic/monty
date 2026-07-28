//! Versioned framing for serialized interpreter state.

use std::mem::size_of;

use serde::{Serialize, de::DeserializeOwned};

/// Prefix distinguishing Monty dumps from unframed postcard data.
const MAGIC: &[u8; 6] = b"MONTY\0";

/// Version of the postcard schema shared by all interpreter dump kinds.
pub(crate) const VERSION: u16 = 1;

/// Number of bytes before the postcard payload.
const HEADER_LEN: usize = MAGIC.len() + size_of::<u16>() + size_of::<u8>();

/// Identifies the public API which produced a dump.
#[repr(u8)]
#[derive(Clone, Copy)]
pub(crate) enum DumpKind {
    /// A compiled [`crate::MontyRun`].
    MontyRun = 0,
    /// Suspended [`crate::RunProgress`].
    RunProgress = 1,
    /// An idle [`crate::MontyRepl`].
    MontyRepl = 2,
    /// Suspended [`crate::ReplProgress`].
    ReplProgress = 3,
}

/// Serializes `value` with a versioned, kind-specific header.
pub(crate) fn dump(value: &impl Serialize, kind: DumpKind) -> Result<Vec<u8>, postcard::Error> {
    let payload = postcard::to_allocvec(value)?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.push(kind as u8);
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Deserializes a dump after validating its format version and kind.
pub(crate) fn load<T: DeserializeOwned>(bytes: &[u8], kind: DumpKind) -> Result<T, postcard::Error> {
    let Some(header) = bytes.get(..HEADER_LEN) else {
        return Err(postcard::Error::DeserializeBadEncoding);
    };
    let version = u16::from_le_bytes([header[MAGIC.len()], header[MAGIC.len() + 1]]);
    if &header[..MAGIC.len()] != MAGIC || version != VERSION || header[HEADER_LEN - 1] != kind as u8 {
        Err(postcard::Error::DeserializeBadEncoding)
    } else {
        let (value, remainder) = postcard::take_from_bytes(&bytes[HEADER_LEN..])?;
        if remainder.is_empty() {
            Ok(value)
        } else {
            Err(postcard::Error::DeserializeBadEncoding)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VERSION;
    use crate::{
        bytecode::opcode_fingerprint, expressions::comparison_operators_fingerprint, intern::static_strings_fingerprint,
    };

    /// If a component changes incompatibly, bump `VERSION` before updating its
    /// expected fingerprint. Compatible changes only require a fingerprint update.
    ///
    /// NB this test is not exhaustive of all possible compatibility issues, it just helps
    /// catch the obvious ones!
    #[test]
    fn serialized_components_match_dump_version() {
        assert_eq!(
            opcode_fingerprint(),
            0xb1a3_02e5_6343_c49f,
            "opcodes changed for dump version {VERSION}"
        );
        assert_eq!(
            static_strings_fingerprint(),
            0x8284_9bb4_2d6d_5fc7,
            "static strings changed for dump version {VERSION}"
        );
        assert_eq!(
            comparison_operators_fingerprint(),
            0x8ecc_d26b_160d_9c0b,
            "comparison operators changed for dump version {VERSION}"
        );
    }
}
