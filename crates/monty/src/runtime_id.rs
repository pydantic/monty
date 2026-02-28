//! Host-facing runtime value identifiers.
//!
//! These identifiers are generic runtime instrumentation metadata used by host
//! integrations. They are intentionally separate from guest-Python state and
//! are serialized with snapshots so identity continuity can be observed across
//! suspend/resume and dump/load boundaries.

/// Stable host-facing identifier for a runtime value.
///
/// The identifier is opaque to host callers: it is only guaranteed to be
/// stable for a value within a single execution lineage (including snapshots).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RuntimeValueId(usize);

impl RuntimeValueId {
    /// Creates a new runtime value identifier from a raw internal value.
    #[must_use]
    pub const fn new(raw: usize) -> Self {
        Self(raw)
    }

    /// Returns the raw identifier value.
    #[must_use]
    pub const fn raw(self) -> usize {
        self.0
    }
}
