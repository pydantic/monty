//! SHA-1 and the SHA-2 family through the RustCrypto `sha1` / `sha2` crates,
//! whose compression uses the CPU's SHA instructions where it has them.
//!
//! The hasher is persisted through the crate's `SerializableState`, so the
//! dump holds the crate's own byte layout: a crate upgrade that changes it is a
//! `DUMP_VERSION` bump (see `dump_format.rs`).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::digest::{
    Digest,
    common::hazmat::{SerializableState, SerializedState},
};

use super::DigestBytes;

/// SHA-1, as `hashlib.sha1`.
pub(crate) type Sha1 = CrateHash<sha1::Sha1>;
/// SHA-224, as `hashlib.sha224`.
pub(crate) type Sha224 = CrateHash<sha2::Sha224>;
/// SHA-256, as `hashlib.sha256`.
pub(crate) type Sha256 = CrateHash<sha2::Sha256>;
/// SHA-384, as `hashlib.sha384`.
pub(crate) type Sha384 = CrateHash<sha2::Sha384>;
/// SHA-512, as `hashlib.sha512`.
pub(crate) type Sha512 = CrateHash<sha2::Sha512>;

/// A RustCrypto hasher with the streaming interface the other cores share.
///
/// The crate types are fixed-size stack structs, so cloning one to take a
/// digest without consuming the stream costs nothing on the heap.
#[derive(Clone)]
pub(crate) struct CrateHash<D>(D);

impl<D: Digest + Clone> CrateHash<D> {
    pub(crate) fn new() -> Self {
        Self(D::new())
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        Digest::update(&mut self.0, data);
    }

    /// The digest of everything absorbed so far; the state stays usable.
    pub(crate) fn digest(&self) -> DigestBytes {
        DigestBytes::from_slice(&self.0.clone().finalize())
    }
}

impl<D> fmt::Debug for CrateHash<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CrateHash(..)")
    }
}

impl<D: SerializableState> Serialize for CrateHash<D> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0.serialize())
    }
}

impl<'de, D: SerializableState> Deserialize<'de> for CrateHash<D> {
    fn deserialize<De: Deserializer<'de>>(deserializer: De) -> Result<Self, De::Error> {
        let bytes = serde_bytes::ByteBuf::deserialize(deserializer)?;
        let state = SerializedState::<D>::try_from(bytes.as_slice()).map_err(de::Error::custom)?;
        D::deserialize(&state).map(Self).map_err(de::Error::custom)
    }
}
