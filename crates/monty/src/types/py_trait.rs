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
    bytecode::VM,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapId},
    intern::{ExtFunctionId, Interns},
    os::OsFunction,
    resource::{DepthGuard, ResourceTracker},
    value::{EitherStr, Value},
};

/// Result of calling an attribute method via `py_call_attr_raw`.
///
/// This enum enables attribute methods to signal different outcomes to the VM:
/// - `Value`: The call completed synchronously with a return value
/// - `OsCall`: The method needs an OS operation; VM should yield to host
/// - `ExternalCall`: The method needs to call an external function
///
/// This unifies the pattern where `call_function` returns `CallResult` to indicate
/// different outcomes. Types that only support synchronous attribute calls can
/// use the default `py_call_attr_raw` implementation which wraps `py_call_attr`.
///
/// # Future Extensibility
///
/// When needed for features like `list.sort(key=func)`, we can add:
/// ```ignore
/// CallFunction(Value, ArgValues)  // Call a callable, result becomes attr result
/// ```
#[derive(Debug)]
pub enum AttrCallResult {
    /// Call completed synchronously with a value to return.
    Value(Value),

    /// The method needs an OS operation. VM should yield `FrameExit::OsCall` to host.
    ///
    /// The host executes the OS operation and resumes the VM with the result.
    /// Used by `Path` filesystem methods like `exists()`, `read_text()`, etc.
    OsCall(OsFunction, ArgValues),

    /// The method needs to call an external function. VM should yield `FrameExit::ExternalCall`.
    ///
    /// Used when attribute methods delegate to registered external functions.
    /// Currently unused - will be used when types need to call external functions from attribute methods.
    #[expect(dead_code)]
    ExternalCall(ExtFunctionId, ArgValues),

    /// Dataclass method call — VM should yield `FrameExit::MethodCall` to host.
    ///
    /// Carries the method name (e.g. `"distance"`) and args with self prepended.
    /// This is detected by `call_dataclass_attr_raw` when a public attribute name is not
    /// found in the dataclass's attrs dict.
    MethodCall(EitherStr, ArgValues),
    /// The method returned a value that should be implicitly awaited.
    ///
    /// Used by `asyncio.run()` to execute a coroutine without an explicit `await`.
    /// The VM will push the value onto the stack and execute `exec_get_awaitable`.
    AwaitValue(Value),
}

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
    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type;

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
    /// The `guard` parameter tracks recursion depth to prevent stack overflow
    /// on deeply nested structures.
    ///
    /// Returns `Ok(true)` if equal, `Ok(false)` if not equal, or
    /// `Err(ResourceError::Recursion)` if maximum depth is exceeded.
    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Result<bool, ResourceError>;

    /// Python comparison (`<`, `>`, etc.).
    ///
    /// For containers, this performs element-wise comparison using the heap
    /// to resolve nested references. Takes `&mut Heap` to allow lazy hash
    /// computation for dict key lookups.
    ///
    /// The `interns` parameter provides access to interned string content.
    /// The `guard` parameter tracks recursion depth to prevent stack overflow
    /// on deeply nested structures.
    ///
    /// Returns `Ok(Some(Ordering))` for comparable values, `Ok(None)` if not comparable,
    /// or `Err(ResourceError::Recursion)` if maximum depth is exceeded.
    fn py_cmp(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<Ordering>, ResourceError> {
        Ok(None)
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
    /// * `guard` - Recursion depth tracker to prevent stack overflow on deeply nested structures
    /// * `interns` - The interned strings table for looking up string/bytes literals
    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> std::fmt::Result;

    /// Returns the Python `repr()` string for this value.
    ///
    /// Convenience wrapper around `py_repr_fmt` that returns an owned string.
    /// Creates a new `DepthGuard` internally to track recursion depth.
    fn py_repr(
        &self,
        heap: &Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Cow<'static, str> {
        let mut s = String::new();
        let mut heap_ids = AHashSet::new();
        // Unwrap is safe: writing to String never fails
        self.py_repr_fmt(&mut s, heap, &mut heap_ids, guard, interns).unwrap();
        Cow::Owned(s)
    }

    /// Returns the Python `str()` string for this value.
    ///
    /// The `guard` parameter tracks recursion depth to prevent stack overflow
    /// on deeply nested structures.
    fn py_str(
        &self,
        heap: &Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Cow<'static, str> {
        self.py_repr(heap, guard, interns)
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
    ) -> Result<Option<Value>, ResourceError> {
        Ok(None)
    }

    /// Python subtraction (`__sub__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(ResourceError)` if allocation fails.
    fn py_sub(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        Ok(None)
    }

    /// Python modulus (`__mod__`).
    ///
    /// Returns `Ok(None)` if the operation is not supported for these types,
    /// `Ok(Some(value))` on success, or `Err(RunError)` if an error occurs.
    fn py_mod(&self, _other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
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
    ///
    /// The `interns` parameter provides access to interned string content for InternString/InternBytes.
    fn py_iadd(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        _self_id: Option<HeapId>,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
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
    fn py_div(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<Value>> {
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
        attr: &EitherStr,
        _args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        Err(ExcType::attribute_error(self.py_type(heap), attr.as_str(interns)))
    }

    /// Calls an attribute method, returning an `AttrCallResult` that may signal OS, external,
    /// or method calls.
    ///
    /// This method enables types to signal that they need operations the VM cannot perform
    /// directly (OS operations, external function calls, dataclass method calls). The VM
    /// converts the result to the appropriate `FrameExit` variant.
    ///
    /// The default implementation wraps `py_call_attr` in `AttrCallResult::Value`. Types that
    /// need to perform OS/external operations, intercept specific methods (e.g. `list.sort`),
    /// or detect method calls (e.g. dataclass methods) should override this method.
    ///
    /// # Arguments
    /// * `self_id` - The heap ID of this value, needed by types that must reference themselves
    ///   (e.g. dataclass method calls prepend `self` to args)
    ///
    /// # Returns
    ///
    /// - `Ok(AttrCallResult::Value(v))` - Method completed synchronously with value `v`
    /// - `Ok(AttrCallResult::OsCall(func, args))` - Method needs OS operation; VM yields to host
    /// - `Ok(AttrCallResult::ExternalCall(id, args))` - Method needs external function call
    /// - `Ok(AttrCallResult::MethodCall(attr, args))` - Dataclass method call; VM yields to host
    /// - `Err(e)` - Method call failed with error
    fn py_call_attr_raw(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<AttrCallResult> {
        let value = self.py_call_attr(vm.heap, attr, args, vm.interns)?;
        Ok(AttrCallResult::Value(value))
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
        Err(ExcType::type_error_not_sub(self.py_type(heap)))
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
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("'{}' object does not support item assignment", self.py_type(heap)),
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
    /// Takes `&mut Heap` to allow:
    /// - Cloning stored values with proper reference counting
    /// - Allocating computed values that need heap storage
    ///
    /// Default implementation returns `Ok(None)`, indicating the type doesn't support
    /// attribute access and a generic `AttributeError` should be raised by the caller.
    fn py_getattr(
        &self,
        _attr: &EitherStr,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        Ok(None)
    }
}
