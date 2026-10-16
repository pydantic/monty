//! Reference representations shared by runtime graphs and pointer-free wire graphs.

use std::{fmt::Debug, hash::Hash, ptr::NonNull};

use serde::Serialize;

use super::{HeapEntry, HeapIndex};
use crate::value::NoHeapRef;

/// A copyable handle, not an owned Python reference; ownership belongs to its containing value.
pub(crate) trait Reference: Copy + Debug + Eq + Hash {
    /// Only runtime references require explicit contextual refcount cleanup.
    #[cfg(feature = "memory-model-checks")]
    const CHECK_DROP: bool;
}

impl Reference for HeapIndex {
    #[cfg(feature = "memory-model-checks")]
    const CHECK_DROP: bool = false;
}

impl Reference for NoHeapRef {
    #[cfg(feature = "memory-model-checks")]
    const CHECK_DROP: bool = false;
}

/// Sealed arena-branded access, unavailable to integer wire references and constants.
pub(crate) trait RuntimeRef: Reference + Serialize + sealed::Sealed {
    /// Numeric identity remains stable while slots are vacant or reused.
    fn index(self) -> usize;

    /// Returns the permanent, interior-mutable entry protected by this reference's brand.
    fn pointer(self) -> NonNull<HeapEntry<Self>>;

    /// Mints a handle only inside the arena whose lifetime and identity the type represents.
    ///
    /// # Safety
    /// `entry` must remain initialized and allocated whenever this handle can be used.
    /// Only the sealed owner may retain an erased brand; its active brands must each
    /// identify one arena that cannot be replaced independently of its roots.
    unsafe fn from_pointer(entry: NonNull<HeapEntry<Self>>) -> Self;
}

/// Restricts runtime references to the heap's audited pointer representation.
pub(super) mod sealed {
    pub trait Sealed {}
}
