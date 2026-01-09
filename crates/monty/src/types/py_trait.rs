/// Trait for heap-allocated Python values that need common operations.
///
/// This trait abstracts over container types (List, Tuple, Str, Bytes) stored
/// in the heap, providing a unified interface for operations like length,
/// equality, reference counting support, and attribute dispatch.
///
/// The trait is designed to work with `enum_dispatch` for efficient virtual
/// dispatch on `HeapData` without boxing overhead.
use std::borrow::Cow;
use std::{cmp::Ordering, fmt::Write};

use ahash::AHashSet;

use super::Type;
use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::{Heap, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    value::{Attr, Value},
};

/// Common operations for heap-allocated Python values.
///
/// Implementers should provide Python-compatible semantics for all operations.
/// Most methods take a `&Heap` reference to allow nested lookups for containers
/// holding `Value::Ref` values.
///
/// This trait is used with `enum_dispatch` on `HeapData` to enable efficient
/// virtual dispatch without boxing overhead.
///
/// Many methods are generic over `T: ResourceTracker` to work with any heap
/// configuration. This allows the same trait to work with both unlimited and
/// resource-limited execution contexts.
pub trait PyTrait {
    /// Returns the Python type name for this value (e.g., "list", "str").
    ///
    /// Used for error messages and the `type()` builtin.
    /// Takes heap reference for cases where nested Value lookups are needed.
    fn py_type(&self, heap: Option<&Heap<impl ResourceTracker>>) -> Type;

    /// Returns the number of elements in this container.
    ///
    /// For interns, returns the number of Unicode codepoints (characters), matching Python.
    /// Returns `None` if the type doesn't support `len()`.
    ///
    /// The `interns` parameter provides access to interned string content for InternString/InternBytes.
    fn py_len(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<usize>;

    /// Python equality comparison (`==`).
    ///
    /// For containers, this performs element-wise comparison using the heap
    /// to resolve nested references. Takes `&mut Heap` to allow lazy hash
    /// computation for dict key lookups.
    ///
    /// The `interns` parameter provides access to interned string content.
    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool;

    /// Python comparison (`<`, `>`, etc.).
    ///
    /// For containers, this performs element-wise comparison using the heap
    /// to resolve nested references. Takes `&mut Heap` to allow lazy hash
    /// computation for dict key lookups.
    ///
    /// The `interns` parameter provides access to interned string content.
    fn py_cmp(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> Option<Ordering> {
        None
    }

    /// Pushes any contained `HeapId`s onto the stack for reference counting.
    ///
    /// This is called during `dec_ref` to find nested heap references that
    /// need their refcounts decremented when this value is freed.
    ///
    /// When the `ref-count-panic` feature is enabled, this method also marks all
    /// contained `Value`s as `Dereferenced` to prevent Drop panics. This
    /// co-locates the cleanup logic with the reference collection logic.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>);

    /// Returns the truthiness of the value following Python semantics.
    ///
    /// Container types should typically report `false` when empty.
    ///
    /// The `interns` parameter provides access to interned string content.
    fn py_bool(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        self.py_len(heap, interns) != Some(0)
    }

    /// Writes the Python `repr()` string for this value to a formatter.
    ///
    /// This method enables cycle detection for self-referential structures by tracking
    /// visited heap IDs. When a cycle is detected (ID already in `heap_ids`), implementations
    /// should write an ellipsis (e.g., `[...]` for lists, `{...}` for dicts).
    ///
    /// # Arguments
    /// * `f` - The formatter to write to
    /// * `heap` - The heap for resolving value references
    /// * `heap_ids` - Set of heap IDs currently being repr'd (for cycle detection)
    /// * `interns` - The interned strings table for looking up string/bytes literals
    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result;

    /// Returns the Python `repr()` string for this value.
    ///
    /// Convenience wrapper around `py_repr_fmt` that returns an owned string.
    fn py_repr(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Cow<'static, str> {
        let mut s = String::new();
        let mut heap_ids = AHashSet::new();
        // Unwrap is safe: writing to String never fails
        self.py_repr_fmt(&mut s, heap, &mut heap_ids, interns).unwrap();
        Cow::Owned(s)
    }

    /// Returns the Python `str()` string for this value.
    fn py_str(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Cow<'static, str> {
        self.py_repr(heap, interns)
    }

    /// Python addition (`__add__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(ResourceError)` if allocation fails.
    ///
    /// The `interns` parameter provides access to interned string content for InternString/InternBytes.
    fn py_add(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        Ok(None)
    }

    /// Python subtraction (`__sub__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(ResourceError)` if allocation fails.
    fn py_sub(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        Ok(None)
    }

    /// Python modulus (`__mod__`).
    fn py_mod(&self, _other: &Self) -> Option<Value> {
        None
    }

    /// Optimized helper for `(a % b) == c` comparisons.
    fn py_mod_eq(&self, _other: &Self, _right_value: i64) -> Option<bool> {
        None
    }

    /// Python in-place addition (`__iadd__`).
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the operation was successful, `Ok(false)` if not supported,
    /// or `Err(ResourceError)` if allocation fails.
    ///
    /// The `interns` parameter provides access to interned string content for InternString/InternBytes.
    fn py_iadd(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        _self_id: Option<HeapId>,
        _interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        // Drop other if it's a Ref (ensure proper refcounting for unsupported types)
        other.drop_with_heap(heap);
        Ok(false)
    }

    /// Python multiplication (`__mul__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types.
    /// For numeric types: Int * Int, Float * Float, Int * Float, etc.
    /// For sequences: str * int, list * int for repetition.
    fn py_mult(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python true division (`__truediv__`).
    ///
    /// Always returns float for numeric types. Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for division by zero.
    fn py_div(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python floor division (`__floordiv__`).
    ///
    /// Returns int for int//int, float for float operations.
    /// Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for division by zero.
    fn py_floordiv(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python power (`__pow__`).
    ///
    /// Int ** positive_int returns int, int ** negative_int returns float.
    /// Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for 0 ** negative.
    fn py_pow(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Calls an attribute method on this value (e.g., `list.append()`).
    ///
    /// Returns an error if the attribute doesn't exist or the arguments are invalid.
    /// Generic over ResourceTracker to work with any heap configuration.
    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        _args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        Err(ExcType::attribute_error(self.py_type(Some(heap)), attr.as_str(interns)))
    }

    /// Estimates the memory size in bytes of this value.
    ///
    /// Used by resource tracking to enforce memory limits. Returns the approximate
    /// heap footprint including struct overhead and variable-length data (e.g., string
    /// contents, list elements).
    ///
    /// Note: For containers holding `Value::Ref` entries, this counts the size of
    /// the reference slots, not the referenced objects. Nested objects are sized
    /// separately when they are allocated.
    fn py_estimate_size(&self) -> usize;

    /// Python subscript get operation (`__getitem__`), e.g., `d[key]`.
    ///
    /// Returns the value associated with the key, or an error if the key doesn't exist
    /// or the type doesn't support subscripting.
    ///
    /// The `&mut Heap` parameter is needed for proper reference counting when cloning
    /// the returned value. The `interns` parameter provides access to interned string content.
    ///
    /// Default implementation returns TypeError.
    fn py_getitem(&self, _key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        Err(ExcType::type_error_not_sub(self.py_type(Some(heap))))
    }

    /// Python subscript set operation (`__setitem__`), e.g., `d[key] = value`.
    ///
    /// Sets the value associated with the key, or returns an error if the key is invalid
    /// or the type doesn't support subscript assignment.
    ///
    /// The `interns` parameter provides access to interned string content.
    ///
    /// Default implementation returns TypeError.
    fn py_setitem(
        &mut self,
        _key: Value,
        _value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<()> {
        Err(ExcType::TypeError).map_err(|e| {
            crate::exception_private::exc_fmt!(e; "'{}' object does not support item assignment", self.py_type(Some(heap)))
                .into()
        })
    }
}
