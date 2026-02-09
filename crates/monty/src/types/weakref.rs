//! Minimal weak reference support.
//!
//! This module provides a `WeakRef` type that mirrors `weakref.ref` objects.
//! Weak references do not keep targets alive and return `None` when the target
//! has been cleared.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    heap::{Heap, HeapId},
    intern::{Interns, StringId},
    resource::ResourceTracker,
    types::{AttrCallResult, PyTrait, Type},
};

/// A weak reference to a heap-allocated object.
///
/// Weak references do not participate in reference counting for the target.
/// When the target is freed, the weak reference is cleared and returns `None`
/// when called.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct WeakRef {
    /// The referenced heap ID, or `None` if the target has been cleared.
    target: Option<HeapId>,
}

impl WeakRef {
    /// Creates a new weak reference to the given heap ID.
    #[must_use]
    pub fn new(target: HeapId) -> Self {
        Self { target: Some(target) }
    }

    /// Returns the target heap ID if still alive.
    #[must_use]
    pub fn target(&self) -> Option<HeapId> {
        self.target
    }

    /// Clears the weak reference (target has been freed).
    pub fn clear(&mut self) {
        self.target = None;
    }
}

impl PyTrait for WeakRef {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::WeakRef
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        false
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Weak references do not own the target.
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _interns: &Interns,
    ) -> std::fmt::Result {
        f.write_str("<weakref>")
    }

    fn py_getattr(
        &self,
        _attr_id: StringId,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> crate::exception_private::RunResult<Option<AttrCallResult>> {
        Ok(None)
    }
}
