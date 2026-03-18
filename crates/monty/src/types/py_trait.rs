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
    ResourceError,
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapId},
    resource::ResourceTracker,
    value::{EitherStr, Value},
};

/// Common operations for heap-allocated Python values.
///
/// Implementers should provide Python-compatible semantics for all operations.
/// Most methods take a `&VM` or `&mut VM` reference to access the heap and interned
/// strings for nested lookups in containers holding `Value::Ref` values.
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
    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type;

    /// Returns the number of elements in this container.
    ///
    /// For interns, returns the number of Unicode codepoints (characters), matching Python.
    /// Returns `None` if the type doesn't support `len()`.
    fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize>;

    /// Python equality comparison (`==`).
    ///
    /// For containers, this performs element-wise comparison using the heap
    /// to resolve nested references. Takes `&mut VM` to allow lazy hash
    /// computation for dict key lookups and access to interned string content.
    ///
    /// Recursion depth is tracked via `heap.incr_recursion_depth()`.
    ///
    /// Returns `Ok(true)` if equal, `Ok(false)` if not equal, or
    /// `Err(ResourceError::Recursion)` if maximum depth is exceeded.
    fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError>;

    /// Python comparison (`<`, `>`, etc.).
    ///
    /// For containers, this performs element-wise comparison using the heap
    /// to resolve nested references. Takes `&mut VM` to allow lazy hash
    /// computation for dict key lookups and access to interned string content.
    ///
    /// Recursion depth is tracked via `heap.incr_recursion_depth()`.
    ///
    /// Returns `Ok(Some(Ordering))` for comparable values, `Ok(None)` if not comparable,
    /// or `Err(ResourceError::Recursion)` if maximum depth is exceeded.
    fn py_cmp(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Ordering>, ResourceError> {
        Ok(None)
    }

    /// Returns the truthiness of the value following Python semantics.
    ///
    /// Container types should typically report `false` when empty.
    fn py_bool(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        self.py_len(vm) != Some(0)
    }

    /// Writes the Python `repr()` string for this value to a formatter.
    ///
    /// This method enables cycle detection for self-referential structures by tracking
    /// visited heap IDs. When a cycle is detected (ID already in `heap_ids`), implementations
    /// should write an ellipsis (e.g., `[...]` for lists, `{...}` for dicts).
    ///
    /// Recursion depth is tracked via `heap.incr_recursion_depth_for_repr()`.
    ///
    /// # Arguments
    /// * `f` - The formatter to write to
    /// * `vm` - The VM for resolving value references and looking up interned strings
    /// * `heap_ids` - Set of heap IDs currently being repr'd (for cycle detection)
    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result;

    /// Returns the Python `repr()` string for this value.
    ///
    /// Convenience wrapper around `py_repr_fmt` that returns an owned string.
    fn py_repr(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Cow<'static, str> {
        let mut s = String::new();
        let mut heap_ids = AHashSet::new();
        // Unwrap is safe: writing to String never fails
        self.py_repr_fmt(&mut s, vm, &mut heap_ids).unwrap();
        Cow::Owned(s)
    }

    /// Returns the Python `str()` string for this value.
    ///
    /// Recursion depth is tracked via the heap's recursion depth counter.
    fn py_str(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Cow<'static, str> {
        self.py_repr(vm)
    }

    /// Python addition (`__add__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(ResourceError)` if allocation fails.
    fn py_add(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, ResourceError> {
        Ok(None)
    }

    /// Python subtraction (`__sub__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(ResourceError)` if allocation fails.
    fn py_sub(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, ResourceError> {
        Ok(None)
    }

    /// Python modulus (`__mod__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(RunError)` if an error occurs.
    fn py_mod(&self, _other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
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
    fn py_iadd(
        &mut self,
        _other: &Value,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
        _self_id: Option<HeapId>,
    ) -> Result<bool, ResourceError> {
        Ok(false)
    }

    /// Python multiplication (`__mul__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types.
    /// For numeric types: Int * Int, Float * Float, Int * Float, etc.
    /// For sequences: str * int, list * int for repetition.
    fn py_mult(&self, _other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python true division (`__truediv__`).
    ///
    /// Always returns float for numeric types. Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for division by zero.
    fn py_div(&self, _other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python floor division (`__floordiv__`).
    ///
    /// Returns int for int//int, float for float operations.
    /// Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for division by zero.
    fn py_floordiv(&self, _other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python power (`__pow__`).
    ///
    /// Int ** positive_int returns int, int ** negative_int returns float.
    /// Returns `Ok(None)` if not supported.
    /// Returns `Err(ZeroDivisionError)` for 0 ** negative.
    fn py_pow(&self, _other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Calls an attribute method on this value (e.g., `list.append()`), returning a
    /// `CallResult` that may signal OS, external, or method calls.
    ///
    /// This method enables types to signal that they need operations the VM cannot perform
    /// directly (OS operations, external function calls, dataclass method calls). The VM
    /// converts the result to the appropriate `FrameExit` variant.
    ///
    /// Types that only support synchronous attribute calls should wrap their return value
    /// with `CallResult::Value`. Types that need to perform OS/external operations,
    /// intercept specific methods (e.g. `list.sort`), or detect method calls (e.g. dataclass
    /// methods) should return the appropriate `CallResult` variant.
    ///
    /// # Arguments
    /// * `self_id` - The heap ID of this value, needed by types that must reference themselves
    ///   (e.g. dataclass method calls prepend `self` to args)
    ///
    /// # Returns
    ///
    /// - `Ok(CallResult::Value(v))` - Method completed synchronously with value `v`
    /// - `Ok(CallResult::OsCall(func, args))` - Method needs OS operation; VM yields to host
    /// - `Ok(CallResult::External(name, args))` - Method needs external function call
    /// - `Ok(CallResult::MethodCall(attr, args))` - Dataclass method call; VM yields to host
    /// - `Err(e)` - Method call failed with error
    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        // `py_call_attr` takes ownership of the argument bundle. Implementations that
        // do not recognize the attribute still need to release those values before
        // reporting `AttributeError`, otherwise method calls on unsupported types leak
        // references on the error path (caught by `ref-count-panic`).
        args.drop_with_heap(vm);
        Err(ExcType::attribute_error(self.py_type(vm.heap), attr.as_str(vm.interns)))
    }

    /// Python subscript get operation (`__getitem__`), e.g., `d[key]`.
    ///
    /// Returns the value associated with the key, or an error if the key doesn't exist
    /// or the type doesn't support subscripting.
    ///
    /// Takes `&mut VM` for proper reference counting when cloning the returned value
    /// and access to interned string content.
    ///
    /// Default implementation returns TypeError.
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
        Err(ExcType::type_error_not_sub(self.py_type(vm.heap)))
    }

    /// Python subscript set operation (`__setitem__`), e.g., `d[key] = value`.
    ///
    /// Sets the value associated with the key, or returns an error if the key is invalid
    /// or the type doesn't support subscript assignment.
    ///
    /// Default implementation returns TypeError.
    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        key.drop_with_heap(vm.heap);
        value.drop_with_heap(vm.heap);
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("'{}' object does not support item assignment", self.py_type(vm.heap)),
        )
        .into())
    }

    /// Python attribute get operation (`__getattr__`), e.g., `obj.attr`.
    ///
    /// Returns the value associated with the attribute (owned), or `Ok(None)` if the type
    /// doesn't support attribute access at all. Types that support attributes should return
    /// `Err(AttributeError)` when an attribute is not found, not `Ok(None)`.
    ///
    /// The returned `Value` is always owned:
    /// - For stored values (Dataclass, Module, NamedTuple fields): clone with `clone_with_heap`
    /// - For computed values (Exception.args, Slice.start, Path.name): return newly created value
    ///
    /// Takes `&mut VM` to allow:
    /// - Cloning stored values with proper reference counting
    /// - Allocating computed values that need heap storage
    ///
    /// Default implementation returns `Ok(None)`, indicating the type doesn't support
    /// attribute access and a generic `AttributeError` should be raised by the caller.
    fn py_getattr(
        &self,
        _attr: &EitherStr,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Option<CallResult>> {
        Ok(None)
    }
}
