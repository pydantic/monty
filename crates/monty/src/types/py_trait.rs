use std::{cmp::Ordering, fmt::Write};

use ahash::AHashSet;
/// Trait for heap-allocated Python values that need common operations.
///
/// This trait abstracts over container types (List, Tuple, Str, Bytes) stored
/// in the heap, providing a unified interface for operations like length,
/// equality, reference counting support, and attribute dispatch.
///
/// The lifetime `'h` ties methods to the heap lifetime so that
/// `HeapObjectRead<'h, T>` types can implement the trait with access to the `VM<'h, …>`.
///
/// The trait is designed to work with `enum_dispatch` for efficient virtual
/// dispatch on `HeapData` without boxing overhead.
use monty_types::OsFunctionCall;

use super::{Type, allocate_string};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    expressions::CmpOperator,
    hash::HashValue,
    heap::{DropWithContext, HeapId},
    intern::StringId,
    value::{EitherStr, Value},
};

/// Return type for attribute method calls on heap-allocated types.
///
/// Similar to `CallResult` but without the `FramePushed` variant, since attribute
/// methods never push new frames directly. Used by `py_call_attr` implementations
/// to signal the VM about what action to take after the call completes.
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
    OsCall(OsFunctionCall),

    /// The method needs to call an external function. VM should yield `FrameExit::ExternalCall`.
    ///
    /// Used when attribute methods delegate to registered external functions.
    /// Currently unused - will be used when types need to call external functions from attribute methods.
    #[expect(dead_code)]
    ExternalCall(StringId, ArgValues),
}

impl From<AttrCallResult> for CallResult {
    fn from(result: AttrCallResult) -> Self {
        match result {
            AttrCallResult::Value(v) => Self::Value(v),
            AttrCallResult::OsCall(call) => Self::OsCall(call),
            AttrCallResult::ExternalCall(ext_id, args) => Self::External(EitherStr::Interned(ext_id), args),
        }
    }
}

/// One of Python's six rich-comparison operations.
///
/// Identity and containment operators use separate protocols and deliberately
/// cannot reach the rich-comparison vtable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichCmpOp {
    /// Less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Le,
    /// Equal (`==`).
    Eq,
    /// Not equal (`!=`).
    Ne,
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Ge,
}

impl RichCmpOp {
    /// Converts a source comparison operator when it is a rich comparison.
    pub(crate) fn from_cmp_operator(op: CmpOperator) -> Option<Self> {
        match op {
            CmpOperator::Lt => Some(Self::Lt),
            CmpOperator::LtE => Some(Self::Le),
            CmpOperator::Eq => Some(Self::Eq),
            CmpOperator::NotEq => Some(Self::Ne),
            CmpOperator::Gt => Some(Self::Gt),
            CmpOperator::GtE => Some(Self::Ge),
            CmpOperator::Is | CmpOperator::IsNot | CmpOperator::In | CmpOperator::NotIn => None,
        }
    }

    /// Returns the operation tried on the right operand after `NotImplemented`.
    pub(crate) fn reflected(self) -> Self {
        match self {
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Eq => Self::Eq,
            Self::Ne => Self::Ne,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
        }
    }

    /// Whether this operation belongs to the equality pair.
    pub(crate) fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }

    /// Returns the special method implementing this operation on an instance.
    pub(crate) fn dunder(self) -> &'static str {
        match self {
            Self::Lt => "__lt__",
            Self::Le => "__le__",
            Self::Eq => "__eq__",
            Self::Ne => "__ne__",
            Self::Gt => "__gt__",
            Self::Ge => "__ge__",
        }
    }

    /// Evaluates this operation against an existing total ordering.
    pub(crate) fn holds(self, ordering: Ordering) -> bool {
        match self {
            Self::Lt => ordering.is_lt(),
            Self::Le => ordering.is_le(),
            Self::Eq => ordering.is_eq(),
            Self::Ne => !ordering.is_eq(),
            Self::Gt => ordering.is_gt(),
            Self::Ge => ordering.is_ge(),
        }
    }

    /// Converts one-sided boolean equality into a rich-comparison result.
    pub(crate) fn equality_result(self, equal: Option<bool>) -> Value {
        debug_assert!(self.is_equality());
        match equal {
            Some(equal) => Value::Bool(if self == Self::Ne { !equal } else { equal }),
            None => Value::NotImplemented,
        }
    }

    /// Returns the spelling used in ordering `TypeError` messages.
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A native implementation of one rich-comparison slot.
pub(crate) type RichCmpFn<'h, T> = fn(&T, &Value, RichCmpOp, &mut VM<'h>) -> RunResult<Value>;

/// Static rich-comparison slots exposed by a Rust value implementation.
///
/// A missing pointer means the type does not define that operation. Multiple
/// slots may share one function when their type checks and comparison work are
/// common; the operation is passed to the function for that purpose.
pub(crate) struct RichCmpVtable<'h, T> {
    /// Native `<` implementation, if defined.
    lt: Option<RichCmpFn<'h, T>>,
    /// Native `<=` implementation, if defined.
    le: Option<RichCmpFn<'h, T>>,
    /// Native `==` implementation, if defined.
    eq: Option<RichCmpFn<'h, T>>,
    /// Native `!=` implementation, if defined.
    ne: Option<RichCmpFn<'h, T>>,
    /// Native `>` implementation, if defined.
    gt: Option<RichCmpFn<'h, T>>,
    /// Native `>=` implementation, if defined.
    ge: Option<RichCmpFn<'h, T>>,
}

impl<'h, T> RichCmpVtable<'h, T> {
    /// A type with no native rich-comparison methods.
    pub(crate) const EMPTY: Self = Self::new(None, None, None, None, None, None);

    /// Creates an arbitrary combination of rich-comparison slots.
    pub(crate) const fn new(
        lt: Option<RichCmpFn<'h, T>>,
        le: Option<RichCmpFn<'h, T>>,
        eq: Option<RichCmpFn<'h, T>>,
        ne: Option<RichCmpFn<'h, T>>,
        gt: Option<RichCmpFn<'h, T>>,
        ge: Option<RichCmpFn<'h, T>>,
    ) -> Self {
        Self { lt, le, eq, ne, gt, ge }
    }

    /// Exposes only equality, deriving `!=` through the same implementation.
    pub(crate) const fn equality(compare: RichCmpFn<'h, T>) -> Self {
        Self::new(None, None, Some(compare), Some(compare), None, None)
    }

    /// Exposes all six operations through one shared implementation.
    pub(crate) const fn all(compare: RichCmpFn<'h, T>) -> Self {
        Self::new(
            Some(compare),
            Some(compare),
            Some(compare),
            Some(compare),
            Some(compare),
            Some(compare),
        )
    }

    /// Resolves the native function for one operation.
    fn get(&self, op: RichCmpOp) -> Option<RichCmpFn<'h, T>> {
        match op {
            RichCmpOp::Lt => self.lt,
            RichCmpOp::Le => self.le,
            RichCmpOp::Eq => self.eq,
            RichCmpOp::Ne => self.ne,
            RichCmpOp::Gt => self.gt,
            RichCmpOp::Ge => self.ge,
        }
    }
}

/// Invokes one statically resolved rich-comparison slot.
pub(crate) fn invoke_rich_cmp_slot<'h>(
    receiver: &impl PyTrait<'h>,
    other: &Value,
    op: RichCmpOp,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    if let Some(compare) = <_ as PyTrait<'h>>::RICH_COMPARE.get(op) {
        compare(receiver, other, op, vm)
    } else {
        Ok(Value::NotImplemented)
    }
}

/// Common operations for heap-allocated Python values.
///
/// Implementers should provide Python-compatible semantics for all operations.
/// Most methods take a `&VM` or `&mut VM` reference to access the heap and interned
/// strings for nested lookups in containers holding `Value::Ref` values.
///
/// This trait is used with `enum_dispatch` on `HeapData` to enable efficient
/// virtual dispatch without boxing overhead.
///
/// Methods take the concrete [`VM`]/`Heap` types, which own the resource
/// tracker enforcing time/memory/recursion limits.
///
/// The lifetime `'h` connects [`crate::heap::HeapObjectRead`] implementations
/// to the VM's heap reference; immediate [`Value`] implementations use it only
/// through the VM argument.
pub(crate) trait PyTrait<'h>: Sized {
    /// Native rich-comparison methods supplied by this type.
    const RICH_COMPARE: RichCmpVtable<'h, Self> = RichCmpVtable::EMPTY;

    /// Returns the Python type name for this value (e.g., "list", "str").
    ///
    /// Used for error messages and the `type()` builtin.
    /// Takes heap reference for cases where nested Value lookups are needed.
    fn py_type(&self, vm: &VM<'h>) -> Type;

    /// Returns the number of elements in this container.
    ///
    /// For interns, returns the number of Unicode codepoints (characters), matching Python.
    /// Returns `None` if the type doesn't support `len()`.
    fn py_len(&self, vm: &VM<'h>) -> Option<usize>;

    /// Computes the hash for this Python value, used for dict and set keys.
    ///
    /// Returns `Ok(Some(hash))` for hashable types, `Ok(None)` for unhashable
    /// types (such as `list` and `dict`), or `Err(ResourceError::Recursion)` if
    /// the recursion limit is exceeded while hashing nested containers.`
    ///
    /// Container implementations should track recursion depth via
    /// `vm.recursion_guard()` (or `vm.incr_recursion()` when iterating) and
    /// recurse through `Value::py_hash` for nested values.
    ///
    /// The default implementation returns `Ok(None)` (unhashable).
    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(None)
    }

    /// One-sided implementation of Python membership (`__contains__`).
    ///
    /// `Ok(None)` means the type has no containment logic of its own, so
    /// [`Value::py_contains`] falls back to iteration and then `TypeError`.
    fn py_contains_impl(&self, _item: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Returns the truthiness of the value following Python semantics.
    ///
    /// Container types should typically report `false` when empty. Truth
    /// testing may raise, notably for Python 3.14's `NotImplemented` singleton.
    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.py_len(vm) != Some(0))
    }

    /// Writes the Python `repr()` string for this value to a formatter.
    ///
    /// This method enables cycle detection for self-referential structures by tracking
    /// visited heap IDs. When a cycle is detected (ID already in `heap_ids`), implementations
    /// should write an ellipsis (e.g., `[...]` for lists, `{...}` for dicts).
    ///
    /// Recursion depth is tracked via `vm.recursion_guard()`.
    ///
    /// # Arguments
    /// * `f` - The formatter to write to
    /// * `vm` - The VM for resolving value references and looking up interned strings
    /// * `heap_ids` - Set of heap IDs currently being repr'd (for cycle detection)
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let type_name = self.py_type(vm).name(vm.heap, vm.interns);
        write!(f, "<{type_name} object>")?;
        Ok(())
    }

    /// Returns the Python `repr()` string for this value as a heap `str` `Value`.
    ///
    /// Convenience wrapper around `py_repr_fmt` that allocates the result.
    ///
    /// TODO: the intermediate `String` here is *not* tracked, so recursive
    /// `repr()` of nested containers can amplify into a multi-gigabyte
    /// host-side buffer before `allocate_string` consults the tracker.
    /// `StringBuilder` is the canonical fix: now that `py_repr` returns a
    /// `Value`, the builder can be `finish`ed here (outside the recursion),
    /// but `py_repr_fmt` still borrows `&mut vm` while writing, so plugging it
    /// in first needs `py_repr_fmt` to no longer need `&mut vm` while the
    /// builder is alive. Today's per-type protections (`INT_MAX_STR_DIGITS`,
    /// `check_repeat_size`, etc.) blunt the worst amplifications but don't
    /// fully cover container `repr()`.
    fn py_repr(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let mut s = String::new();
        let mut heap_ids = LazyHeapSet::default();
        self.py_repr_fmt(&mut s, vm, &mut heap_ids)?;
        Ok(allocate_string(s, vm.heap))
    }

    /// Returns the Python `str()` string for this value.
    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        self.py_repr(vm)
    }

    /// Python unary minus (`__neg__`).
    ///
    /// `Ok(None)` — the default — means the type has no negation, so the VM
    /// raises `TypeError`.
    fn py_neg_impl(&self, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python unary plus (`__pos__`), with [`py_neg_impl`](Self::py_neg_impl)'s contract.
    ///
    /// Rarely a no-op: `+True` is `1`, and `+Counter(a=-1)` strips the
    /// non-positive counts, so implementations return a value rather than `self`.
    fn py_pos_impl(&self, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python addition (`__add__`).
    ///
    fn py_add_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python addition (`__radd__`).
    fn py_radd_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python subtraction (`__sub__`).
    fn py_sub_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python subtraction (`__rsub__`).
    fn py_rsub_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python multiplication (`__mul__`).
    fn py_mul_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python multiplication (`__rmul__`).
    fn py_rmul_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python matrix multiplication (`__matmul__`).
    fn py_matmul_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python matrix multiplication (`__rmatmul__`).
    fn py_rmatmul_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python true division (`__truediv__`).
    fn py_truediv_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python true division (`__rtruediv__`).
    fn py_rtruediv_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python floor division (`__floordiv__`).
    fn py_floordiv_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python floor division (`__rfloordiv__`).
    fn py_rfloordiv_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python modulus (`__mod__`).
    fn py_mod_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python modulus (`__rmod__`).
    fn py_rmod_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python power (`__pow__`).
    fn py_pow_impl(&self, _other: &Value, _modulus: Option<&Value>, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python power (`__rpow__`).
    fn py_rpow_impl(&self, _other: &Value, _modulus: Option<&Value>, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python bitwise AND (`__and__`).
    fn py_and_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python bitwise AND (`__rand__`).
    fn py_rand_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python bitwise OR (`__or__`).
    fn py_or_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python bitwise OR (`__ror__`).
    fn py_ror_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python bitwise XOR (`__xor__`).
    fn py_xor_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python bitwise XOR (`__rxor__`).
    fn py_rxor_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python left shift (`__lshift__`).
    fn py_lshift_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python left shift (`__rlshift__`).
    fn py_rlshift_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// One-sided implementation of Python right shift (`__rshift__`).
    fn py_rshift_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Reflected implementation of Python right shift (`__rrshift__`).
    fn py_rrshift_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(None)
    }

    /// Python in-place addition (`__iadd__`).
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the operation was successful, `Ok(false)` if not supported
    /// (the VM then falls back to `py_add`), or `Err` if the operation raised — including
    /// types whose `+=` is `extend` (e.g. `deque`), which raise `TypeError` from the
    /// iterator protocol rather than a `ResourceError`.
    fn py_iadd_impl(&mut self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(false)
    }

    /// Python in-place subtraction (`__isub__`), with [`py_iadd_impl`](Self::py_iadd_impl)'s
    /// contract: `Ok(true)` mutated `self` in place, `Ok(false)` falls back to binary `-`.
    fn py_isub_impl(&mut self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(false)
    }

    /// Python in-place bitwise AND (`__iand__`), with [`py_iadd_impl`](Self::py_iadd_impl)'s contract.
    fn py_iand_impl(&mut self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(false)
    }

    /// Python in-place bitwise OR (`__ior__`), with [`py_iadd_impl`](Self::py_iadd_impl)'s contract.
    fn py_ior_impl(&mut self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(false)
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
    /// # Returns
    ///
    /// - `Ok(CallResult::Value(v))` - Method completed synchronously with value `v`
    /// - `Ok(CallResult::OsCall(func, args))` - Method needs OS operation; VM yields to host
    /// - `Ok(CallResult::External(name, args))` - Method needs external function call
    /// - `Ok(CallResult::MethodCall(attr, args))` - Dataclass method call; VM yields to host
    /// - `Err(e)` - Method call failed with error
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        // `py_call_attr` takes ownership of the argument bundle. Implementations that
        // do not recognize the attribute still need to release those values before
        // reporting `AttributeError`, otherwise method calls on unsupported types leak
        // references on the error path (caught by `memory-model-checks`).

        args.drop_with(vm);
        Err(ExcType::attribute_error(
            self.py_type(vm).name(vm.heap, vm.interns),
            attr.as_str(vm.interns),
        ))
    }

    /// Whether this type implements the context-manager protocol.
    ///
    /// The `BeforeWith` opcode calls this *before* invoking [`py_enter`] so it
    /// can raise CPython's specific `TypeError` ("object does not support the
    /// context manager protocol") on types that aren't context managers. We
    /// cannot rely on translating the [`py_enter`] default's `AttributeError`,
    /// because a real context manager whose `__enter__` itself raises
    /// `AttributeError` would be misidentified — the distinction has to come
    /// from a declarative check, not from sniffing exception messages.
    ///
    /// Default is `false`; types implementing the protocol override this
    /// alongside [`py_enter`] / [`py_exit`].
    ///
    /// Takes `&VM` (not just the heap) because user-defined instances resolve
    /// the check against their class namespace, which needs both heap and
    /// interns access. Mirroring CPython, the check is for `__exit__` — the
    /// dunder CPython's own protocol error names first — while a missing
    /// `__enter__` is reported by [`py_enter`] itself.
    ///
    /// [`py_enter`]: PyTrait::py_enter
    /// [`py_exit`]: PyTrait::py_exit
    fn py_is_context_manager(&self, _vm: &VM<'h>) -> bool {
        false
    }

    /// Context-manager entry hook (`__enter__`).
    ///
    /// Invoked by the `BeforeWith` opcode after [`py_is_context_manager`]
    /// returns `true`. Returns the value bound to the `as` target (or discarded
    /// if there is none). Typically a context manager returns itself, but it
    /// may return any value.
    ///
    /// Returns `CallResult` so implementations can yield to the host (OS call,
    /// external function, etc.) before producing the entered value.
    ///
    /// The default implementation raises `AttributeError`, matching CPython's
    /// behavior for direct `obj.__enter__()` calls on objects that don't
    /// implement the protocol. The `with` statement never reaches this default
    /// because [`py_is_context_manager`] gates the invocation.
    ///
    /// [`py_is_context_manager`]: PyTrait::py_is_context_manager
    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Err(ExcType::attribute_error(
            self.py_type(vm).name(vm.heap, vm.interns),
            "__enter__",
        ))
    }

    /// Context-manager exit hook (`__exit__`).
    ///
    /// Invoked when execution leaves a `with` block. `exc` is `None` on a normal
    /// exit and `Some(exc_id)` when an exception is propagating; on the exception
    /// path the heap value at `exc_id` is the exception object itself, and a
    /// truthy return value suppresses the exception.
    ///
    /// Monty does not have traceback objects, so the `__exit__(typ, val, tb)`
    /// triple's traceback slot is effectively `None`. This is documented in
    /// `limitations/with.md`.
    ///
    /// Returns `CallResult` so implementations can yield to the host (e.g. file
    /// close issues an `OsCall`).
    ///
    /// The default implementation raises `AttributeError`. In practice the
    /// `with` statement gates this on [`py_is_context_manager`], so this path
    /// is reached only by direct invocation via `obj.__exit__(...)`.
    ///
    /// [`py_is_context_manager`]: PyTrait::py_is_context_manager
    fn py_exit(&mut self, vm: &mut VM<'h>, _exc: Option<HeapId>) -> RunResult<CallResult> {
        Err(ExcType::attribute_error(
            self.py_type(vm).name(vm.heap, vm.interns),
            "__exit__",
        ))
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
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        Err(ExcType::type_error_not_sub(&self.py_type(vm).name(vm.heap, vm.interns)))
    }

    /// Python subscript set operation (`__setitem__`), e.g., `d[key] = value`.
    ///
    /// Sets the value associated with the key, or returns an error if the key is invalid
    /// or the type doesn't support subscript assignment.
    ///
    /// Default implementation returns TypeError.
    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        key.drop_with(vm);
        value.drop_with(vm);
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!(
                "'{}' object does not support item assignment",
                self.py_type(vm).name(vm.heap, vm.interns)
            ),
        )
        .into())
    }

    /// Python attribute assignment, consuming `value` on both success and error.
    ///
    /// The default rejects assignment. Mutable attribute-bearing types override
    /// this method and are responsible for releasing any replaced value.
    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        value.drop_with(vm);
        let type_name = self.py_type(vm).name(vm.heap, vm.interns);
        Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)))
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
    fn py_getattr(&self, _attr: &EitherStr, _vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok(None)
    }

    /// Reports whether [`py_iter`](PyTrait::py_iter) would succeed for this
    /// object, without building the iterator.
    ///
    /// Callers that iterate should just call `py_iter`; this exists for the
    /// sites that must report their *own* error for a non-iterable — the `*`
    /// literal unpack (`Value after * must be an iterable`), set unpack
    /// (`'x' object is not iterable`) and assignment unpacking (`cannot unpack
    /// non-iterable x object`) all word it differently, so they have to ask
    /// before delegating rather than map a failure afterwards.
    ///
    /// Mirrors [`py_is_context_manager`](PyTrait::py_is_context_manager): a type
    /// that can be iterated MUST return `true` here, or it will be reported as
    /// non-iterable by those sites while `list()` and `for` still accept it.
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        false
    }

    /// Whether `next()` can drive this object, the counterpart to
    /// [`py_is_iterable`](PyTrait::py_is_iterable)'s "can `iter()` be called".
    ///
    /// Mirrors CPython's `PyIter_Check`, used by `PyObject_GetIter` to reject an
    /// `__iter__` returning a non-iterator — the one caller here, in
    /// `Instance::py_iter`. A type overriding `py_next` MUST return `true`, or
    /// a user `__iter__` returning it is wrongly rejected.
    fn py_is_iterator(&self, _vm: &VM<'h>) -> bool {
        false
    }

    /// Returns a Python iterator for this object (`__iter__`).
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Err(ExcType::type_error_not_iterable(
            &self.py_type(vm).name(vm.heap, vm.interns),
        ))
    }

    /// Advances this object using Python's iterator protocol (`__next__`).
    ///
    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Err(ExcType::type_error_not_iterator(
            &self.py_type(vm).name(vm.heap, vm.interns),
        ))
    }
}

/// Converts an attribute name into an owned dict key, preserving interned names.
pub(crate) fn attribute_name_value(name: &EitherStr, vm: &VM<'_>) -> Value {
    match name {
        EitherStr::Interned(string_id) => Value::InternString(*string_id),
        EitherStr::Heap(s) => allocate_string(s.as_str(), vm.heap),
    }
}

/// Lazy wrapper around [`AHashSet`] that only allocates the set when needed.
#[derive(Default, Debug, Clone)]
pub(crate) struct LazyHeapSet(Option<AHashSet<HeapId>>);

impl LazyHeapSet {
    pub fn insert(&mut self, heap_id: HeapId) {
        if let Some(s) = self.0.as_mut() {
            s.insert(heap_id);
        } else {
            let mut s = AHashSet::default();
            s.insert(heap_id);
            self.0 = Some(s);
        }
    }

    #[expect(clippy::trivially_copy_pass_by_ref, reason = "Match AHashSet method")]
    pub fn contains(&self, heap_id: &HeapId) -> bool {
        self.0.as_ref().is_some_and(|s| s.contains(heap_id))
    }

    #[expect(clippy::trivially_copy_pass_by_ref, reason = "Match AHashSet method")]
    pub fn remove(&mut self, heap_id: &HeapId) {
        if let Some(s) = self.0.as_mut() {
            s.remove(heap_id);
        }
    }
}
