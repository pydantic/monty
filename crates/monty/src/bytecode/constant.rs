//! Heap-independent constants remain shareable across executions and snapshots.

use serde::{Deserialize, Serialize};

use crate::{
    heap::Reference,
    value::{NoHeapRef, Value},
};

/// An immediate value or integer intern ID, never an execution heap reference.
/// The transparent encoding preserves the existing constant-pool dump representation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Constant(Value<NoHeapRef>);

impl Constant {
    /// Admits compiler output only when it can outlive every execution heap.
    pub fn new(value: &Value<NoHeapRef>) -> Self {
        Self(value.copy_without_refs().expect("heap reference in compiled constants"))
    }

    /// Materializes an immediate runtime value without interner lookups or heap references.
    pub fn value<H: Reference>(&self) -> Value<H> {
        self.0.copy_without_refs().expect("invalid compiled constant")
    }
}

impl Clone for Constant {
    fn clone(&self) -> Self {
        Self(self.0.copy_without_refs().expect("invalid compiled constant"))
    }
}

const _: () = assert!(size_of::<Constant>() == 16);
