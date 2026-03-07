//! Opaque embedder-owned metadata persisted alongside suspendable snapshots.
//!
//! Hosts can attach these bytes to iterative execution progress and recover
//! them after serializing and deserializing snapshots. Monty treats the payload
//! as opaque data and never interprets its contents.

/// Opaque bytes persisted with a suspendable snapshot.
///
/// This newtype keeps snapshot metadata grouped behind a dedicated type so the
/// snapshot API can grow without exposing raw storage details everywhere. The
/// payload remains a simple byte vector today, but callers can use this type to
/// make intent explicit when attaching metadata to snapshots.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SnapshotExtension(Vec<u8>);

impl SnapshotExtension {
    /// Creates a new snapshot extension from raw bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the payload as a byte slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns the owned byte payload.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl AsRef<[u8]> for SnapshotExtension {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<Vec<u8>> for SnapshotExtension {
    fn from(value: Vec<u8>) -> Self {
        Self::new(value)
    }
}

/// Clones an optional snapshot extension reference into owned storage.
#[must_use]
pub(crate) fn clone_snapshot_extension(extension: Option<&SnapshotExtension>) -> Option<SnapshotExtension> {
    extension.cloned()
}
