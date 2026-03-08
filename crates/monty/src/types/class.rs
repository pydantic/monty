//! Minimal user-defined class object support.
//!
//! This type implements the first slice of Python `class` support in Monty:
//! a class statement can bind a real class object that exposes class metadata
//! like `__name__` and renders with a Python class-style repr.
//!
//! It is intentionally narrow. Richer class semantics like executing class
//! bodies, descriptors, inheritance, and instance creation are future work.

use std::{fmt::Write, mem::size_of};

use ahash::AHashSet;

use super::{AttrCallResult, PyTrait, Type};
use crate::{
    exception_private::RunResult,
    heap::{Heap, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::ResourceTracker,
    value::{EitherStr, Value},
};

/// A minimal user-defined class object created by `class Foo: pass`.
///
/// The object currently stores only the class name. It behaves like a Python
/// class object for repr and `__name__`, but does not yet support general class
/// bodies, inheritance, or instantiation.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Class {
    name: StringId,
}

impl Class {
    /// Creates a new skeletal class object with the given interned name.
    pub fn new(name: StringId) -> Self {
        Self { name }
    }

    /// Returns the interned class name.
    pub fn name(&self) -> StringId {
        self.name
    }
}

impl PyTrait for Class {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Type
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<bool, crate::ResourceError> {
        Ok(false)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        write!(f, "<class '{}'>", interns.get_str(self.name))
    }

    fn py_estimate_size(&self) -> usize {
        size_of::<Self>()
    }

    fn py_getattr(
        &self,
        attr: &EitherStr,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        let is_dunder_name = attr
            .static_string()
            .map_or_else(|| false, |ss| ss == StaticStrings::DunderName);
        if is_dunder_name {
            return Ok(Some(AttrCallResult::Value(Value::InternString(self.name))));
        }
        Ok(None)
    }
}
