/// Python tuple type using `SmallVec` for inline storage of small tuples.
///
/// This type provides Python tuple semantics. Tuples are immutable sequences
/// that can contain any Python object. Like lists, tuples properly handle
/// reference counting for heap-allocated values.
///
/// # Optimization
/// Uses `SmallVec<[Value; 2]>` to store up to 2 elements inline without heap
/// allocation. This benefits common cases like 2-tuples from `enumerate()`,
/// `dict.items()`, and function return values.
///
/// # Implemented Methods
/// - `index(value[, start[, end]])` - Find first index of value
/// - `count(value)` - Count occurrences
///
/// All tuple methods from Python's builtins are implemented.
use std::{
    cell::Cell,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use smallvec::SmallVec;

use super::{CmpOrder, PyTrait, iter::collect_owned_iterable};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    hash::HashValue,
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{
        LazyHeapSet, Type,
        list::repr_sequence_fmt,
        slice::{normalize_sequence_index, slice_collect_iterator},
    },
    value::{EitherStr, Value},
};

/// Inline capacity for small tuples. Tuples with 2 or fewer elements avoid
/// heap allocation for the items storage.
const TUPLE_INLINE_CAPACITY: usize = 3;

/// Storage type for tuple items. Uses SmallVec to inline small tuples.
pub(crate) type TupleVec = SmallVec<[Value; TUPLE_INLINE_CAPACITY]>;

/// Python tuple value stored on the heap.
///
/// Uses `SmallVec<[Value; 3]>` internally to avoid separate heap allocation
/// for tuples with 3 or fewer elements. This is a significant optimization
/// since small tuples are very common (enumerate, dict items, returns, etc.).
///
/// # Reference Counting
/// When a tuple is freed, all contained heap references have their refcounts
/// decremented via `push_stack_ids`.
///
/// # GC Optimization
/// The `contains_refs` flag tracks whether the tuple contains any `Value::Ref` items.
/// This allows `collect_child_ids` and `py_dec_ref_ids` to skip iteration when the
/// tuple contains only primitive values (ints, bools, None, etc.).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Tuple {
    items: TupleVec,
    /// True if any item in the tuple is a `Value::Ref`. Set at creation time
    /// since tuples are immutable.
    contains_refs: bool,
    /// Lazily-computed Python hash. Tuples are immutable so this is
    /// computed on first `py_hash` and reused thereafter. Skipped on
    /// serde — recomputable from `items` and we don't want to lock the
    /// snapshot format to the current hash function.
    #[serde(skip)]
    cached_hash: Cell<Option<HashValue>>,
}

impl Tuple {
    /// Creates a new tuple from a vector of values.
    ///
    /// Automatically computes the `contains_refs` flag by checking if any value
    /// is a `Value::Ref`. Since tuples are immutable, this flag never changes.
    ///
    /// For tuples with 3 or fewer elements, the items are stored inline in the
    /// SmallVec without additional heap allocation.
    ///
    /// Note: This does NOT increment reference counts - the caller must
    /// ensure refcounts are properly managed.
    #[must_use]
    fn new(items: TupleVec) -> Self {
        let contains_refs = items.iter().any(|v| matches!(v, Value::Ref(_)));
        Self {
            items,
            contains_refs,
            cached_hash: Cell::new(None),
        }
    }

    /// Returns a reference to the underlying SmallVec.
    #[must_use]
    pub fn as_slice(&self) -> &[Value] {
        &self.items
    }

    /// Returns whether the tuple contains any heap references.
    ///
    /// When false, `collect_child_ids` and `py_dec_ref_ids` can skip iteration.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// Creates a tuple from the `tuple()` constructor call.
    ///
    /// - `tuple()` with no args returns an empty tuple (singleton)
    /// - `tuple(iterable)` creates a tuple from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("tuple", vm.heap)?;
        match value {
            None => {
                // Use empty tuple singleton
                Ok(vm.heap.get_empty_tuple())
            }
            Some(v) => {
                let items = collect_owned_iterable(v, vm)?;
                Ok(allocate_tuple(items, vm.heap)?)
            }
        }
    }
}

impl From<Tuple> for Vec<Value> {
    fn from(tuple: Tuple) -> Self {
        tuple.items.into_vec()
    }
}

impl From<Tuple> for TupleVec {
    fn from(tuple: Tuple) -> Self {
        tuple.items
    }
}

/// Allocates a tuple, using the empty tuple singleton when appropriate.
///
/// This is the preferred way to allocate tuples as it provides:
/// - Empty tuple interning: `() is ()` returns `True`
/// - SmallVec optimization for small tuples (≤3 elements)
///
/// # Example Usage
/// ```ignore
/// // Empty tuple - returns singleton
/// let empty = allocate_tuple(Vec::new(), heap)?;
///
/// // Small tuple - stored inline in SmallVec
/// let pair = allocate_tuple(vec![Value::Int(1), Value::Int(2)], heap)?;
/// ```
pub fn allocate_tuple(
    items: SmallVec<[Value; TUPLE_INLINE_CAPACITY]>,
    heap: &Heap<impl ResourceTracker>,
) -> Result<Value, ResourceError> {
    if items.is_empty() {
        Ok(heap.get_empty_tuple())
    } else {
        // Allocate a new tuple (SmallVec will inline if ≤3 elements)
        let heap_id = heap.allocate(HeapData::Tuple(Tuple::new(items)))?;
        Ok(Value::Ref(heap_id))
    }
}

impl<'h> HeapRead<'h, Tuple> {
    /// Clones the item at the given index with proper refcount management.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h, impl ResourceTracker>) -> Value {
        self.get(vm.heap).items[index].clone_with_heap(vm)
    }

    /// Clones all items from this tuple with proper refcount management.
    fn clone_all_items(&self, vm: &mut VM<'h, impl ResourceTracker>) -> TupleVec {
        let len = self.get(vm.heap).items.len();
        let mut result = TupleVec::with_capacity(len);
        for i in 0..len {
            result.push(self.clone_item(i, vm));
        }
        result
    }

    /// Returns a stack-borrowed lending iterator over the tuple's items,
    /// holding a recursion-depth token for its entire lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`TupleIter`]) because that's the obvious entry point for "iterate
    /// this container".
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter<R: ResourceTracker>(&self, vm: &mut VM<'h, R>) -> RunResult<TupleIter<'_, 'h>> {
        TupleIter::new(self, vm)
    }
}

/// Stack-borrowed lending iterator over a [`Tuple`]'s items.
///
/// Borrows a [`HeapRead`] for its lifetime, so the heap entry is pinned by
/// the reader count for the duration of iteration — no extra refcount on
/// the container is needed.
///
/// **Lending shape.** [`next`](Self::next) returns `Option<&Value>`. The
/// iterator itself owns the most-recently-yielded item in its `current`
/// slot (using [`Value::Undefined`] as the empty sentinel) and drops the
/// previous item at the start of each `next` call, so call sites do **not**
/// need a per-item `defer_drop!`. The held item is also dropped when the
/// iterator is released via [`DropWithContext`].
///
/// **Recursion guard.** Acquires a [`RecursionToken`] at construction and
/// releases it via [`DropWithContext`]. The iterator MUST be wrapped in
/// [`defer_drop_mut!`] so the token (and any in-flight item) is released
/// on every exit path. Unlike list / dict / set iteration, tuples are
/// immutable so size never changes during iteration, but the token still
/// belongs here — tuple iteration almost always feeds into operations that
/// recurse (`py_eq`, `py_hash`, `py_repr`, JSON serialization), and the
/// token bounds the otherwise-unprotected native stack depth.
pub(crate) struct TupleIter<'a, 'h> {
    tuple: &'a HeapRead<'h, Tuple>,
    index: usize,
    token: RecursionToken,
    /// Most-recently-yielded item. `Value::Undefined` when nothing is held —
    /// drops on that variant are no-ops, so `next` can unconditionally drop
    /// the previous slot before fetching the new one.
    current: Value,
}

impl<'a, 'h> TupleIter<'a, 'h> {
    fn new<R: ResourceTracker>(tuple: &'a HeapRead<'h, Tuple>, vm: &mut VM<'h, R>) -> RunResult<Self> {
        let token = vm.recursion_token()?;
        Ok(Self {
            tuple,
            index: 0,
            token,
            current: Value::Undefined,
        })
    }

    /// Advances the iterator and returns a borrow of the next item, or
    /// `Ok(None)` when the tuple is exhausted.
    ///
    /// The returned reference is valid until the next call to `next` (or
    /// until the iterator itself is dropped), at which point the held item
    /// is released.
    ///
    /// Performs a [`check_time`](Heap::check_time) on every call so long
    /// Rust-side loops cannot bypass the configured timeout.
    pub(crate) fn next<'i, R: ResourceTracker>(&'i mut self, vm: &mut VM<'h, R>) -> RunResult<Option<&'i Value>> {
        // Drop the previously-yielded item (no-op when `current` is `Undefined`).
        mem::replace(&mut self.current, Value::Undefined).drop_with(vm.heap);
        vm.heap.check_time()?;
        let items = &self.tuple.get(vm.heap).items;
        if self.index >= items.len() {
            return Ok(None);
        }
        self.current = items[self.index].clone_with_heap(vm.heap);
        self.index += 1;
        Ok(Some(&self.current))
    }

    /// Like [`next`](Self::next), but also returns the 0-based position of the
    /// yielded item — useful for `zip`-style sibling-container access (e.g.
    /// element-wise comparison against another tuple) and for search methods
    /// that return the match position.
    pub(crate) fn next_with_index<'i, R: ResourceTracker>(
        &'i mut self,
        vm: &mut VM<'h, R>,
    ) -> RunResult<Option<(usize, &'i Value)>> {
        // Capture before `next` increments `self.index`.
        let position = self.index;
        Ok(self.next(vm)?.map(|item| (position, item)))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for TupleIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Tuple> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Tuple
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).items.len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(key_id) = key
            && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
        {
            let items =
                slice_collect_iterator(vm, slice_obj, self.get(vm.heap).items.iter(), |v| v.clone_with_heap(vm))?;
            return Ok(allocate_tuple(items, vm.heap)?);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::Tuple)?;
        let len = self.get(vm.heap).as_slice().len();
        let len_i64 = i64::try_from(len).expect("tuple length exceeds i64::MAX");
        let normalized = if index < 0 { index + len_i64 } else { index };

        if normalized < 0 || normalized >= len_i64 {
            return Err(ExcType::tuple_index_error());
        }

        let idx = usize::try_from(normalized).expect("tuple index validated non-negative");
        Ok(self.clone_item(idx, vm))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // A tuple equals another tuple; `tuple == namedtuple` is handled by the
        // reflected pass via `NamedTuple::py_eq_impl`.
        let Some(HeapReadOutput::Tuple(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        if self.get(vm.heap).items.len() != other.get(vm.heap).items.len() {
            return Ok(Some(false));
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, a)) = iter.next_with_index(vm)? {
            let b = other.clone_item(i, vm);
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }

    /// Hashes the tuple as the combined hash of its elements.
    ///
    /// Identical to `NamedTuple::py_hash`, so a `Tuple` and a `NamedTuple` with
    /// the same elements hash equally — required because they compare equal
    /// (matching CPython, where `NamedTuple` is a `tuple` subclass).
    ///
    /// Caches the computed hash on first call. We only cache the `Some(_)`
    /// outcome — `None` (unhashable child) is uncommon and skipping it
    /// keeps the cache slot free of a 3-state encoding.
    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        if let Some(cached) = self.get(vm.heap).cached_hash.get() {
            return Ok(Some(cached));
        }
        let mut hasher = DefaultHasher::new();
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(item) = iter.next(vm)? {
            match item.py_hash(vm)? {
                Some(h) => h.hash(&mut hasher),
                None => return Ok(None),
            }
        }
        let hash = HashValue::new(hasher.finish());
        self.get(vm.heap).cached_hash.set(Some(hash));
        Ok(Some(hash))
    }

    /// Lexicographic comparison for tuples.
    ///
    /// Compares element-by-element left-to-right. The first non-equal pair
    /// determines the result. If all compared elements are equal, the shorter
    /// tuple is considered less than the longer one — matching Python semantics:
    /// `(1, 2) < (1, 2, 3)` is `True`.
    ///
    /// The result of the first differing pair propagates directly: a `NaN`
    /// element yields [`CmpOrder::Unordered`] (`(nan,) < (1,)` is `False`, not a
    /// `TypeError`), while a type-mismatched element yields
    /// [`CmpOrder::Incomparable`] (`(1,) < ('a',)` raises) — this element-level
    /// distinction is exactly why [`CmpOrder`] exists.
    fn py_cmp(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CmpOrder> {
        let a_len = self.get(vm.heap).items.len();
        let b_len = other.get(vm.heap).items.len();
        let min_len = a_len.min(b_len);
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, av)) = iter.next_with_index(vm)? {
            if i >= min_len {
                // `self` was longer than `other`; remaining items don't
                // participate in element-wise comparison.
                break;
            }
            let bv = other.clone_item(i, vm);
            defer_drop!(bv, vm);
            match av.py_cmp(bv, vm)? {
                CmpOrder::Ordered(Ordering::Equal) => {}
                CmpOrder::Ordered(ord) => return Ok(CmpOrder::Ordered(ord)),
                // A `NaN` element: elements are never `==`-equal to a `NaN`, so
                // this is the first differing pair and the tuple is unordered.
                CmpOrder::Unordered => return Ok(CmpOrder::Unordered),
                CmpOrder::Incomparable => {
                    // The elements don't support ordering. CPython checks
                    // `__eq__` first and only calls `__lt__` for non-equal
                    // pairs, so equal-but-unorderable elements (e.g.
                    // `None == None`) are treated as equal and don't block the
                    // comparison; a genuinely differing pair makes the tuple
                    // incomparable.
                    if !av.py_eq(bv, vm)? {
                        return Ok(CmpOrder::Incomparable);
                    }
                }
            }
        }
        Ok(CmpOrder::Ordered(a_len.cmp(&b_len)))
    }

    fn py_add(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let mut items = self.clone_all_items(vm);
        items.extend(other.clone_all_items(vm));
        Ok(Some(allocate_tuple(items, vm.heap)?))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        match attr.static_string() {
            Some(StaticStrings::Index) => tuple_index(self, args, vm).map(CallResult::Value),
            Some(StaticStrings::Count) => tuple_count(self, args, vm).map(CallResult::Value),
            _ => {
                args.drop_with(vm);
                Err(ExcType::attribute_error(Type::Tuple, attr.as_str(vm.interns)))
            }
        }
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        let len = self.get(vm.heap).as_slice().len();

        if len == 1 {
            // Special case for single-element tuples: include the trailing comma
            //
            // Match `repr_sequence_fmt`'s depth handling so nested one-element
            // tuples can't bypass `max_recursion_depth` and overflow the stack.
            let Ok(mut guard) = vm.recursion_guard() else {
                return Ok(f.write_str("...")?);
            };
            let vm = &mut *guard;
            write!(f, "(")?;
            let item = self.clone_item(0, vm);
            defer_drop!(item, vm);
            item.py_repr_fmt(f, vm, heap_ids)?;
            write!(f, ",)")?;
            return Ok(());
        }

        repr_sequence_fmt('(', ')', len, |heap, i| &self.get(heap).as_slice()[i], f, vm, heap_ids)
    }
}

impl HeapItem for Tuple {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.items.len() * mem::size_of::<Value>()
    }

    /// Pushes all heap IDs contained in this tuple onto the stack.
    ///
    /// Called during garbage collection to decrement refcounts of nested values.
    /// When `memory-model-checks` is enabled, also marks all Values as Dereferenced.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - GC optimization for tuples of primitives
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                obj.dec_ref_forget();
            }
        }
    }
}

/// Implements Python's `tuple.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn tuple_index<'h>(
    tuple: &HeapRead<'h, Tuple>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let pos_args = args.into_pos_only("tuple.index", vm.heap)?;
    defer_drop!(pos_args, vm);

    let len = tuple.get(vm.heap).as_slice().len();
    let (value, start, end) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("tuple.index", 1, 0)),
        [value] => (value, 0, len),
        [value, start_arg] => {
            let start = normalize_sequence_index(start_arg.as_int(vm)?, len);
            (value, start, len)
        }
        [value, start_arg, end_arg] => {
            let start = normalize_sequence_index(start_arg.as_int(vm)?, len);
            let end = normalize_sequence_index(end_arg.as_int(vm)?, len).max(start);
            (value, start, end)
        }
        other => return Err(ExcType::type_error_at_most("tuple.index", 3, other.len())),
    };

    let iter = tuple.iter(vm)?;
    defer_drop_mut!(iter, vm);
    while let Some((idx, item)) = iter.next_with_index(vm)? {
        if idx >= end {
            // No further matches possible inside [start, end).
            break;
        }
        if idx >= start && value.py_eq(item, vm)? {
            let idx_i64 = i64::try_from(idx).expect("index exceeds i64::MAX");
            return Ok(Value::Int(idx_i64));
        }
    }

    Err(ExcType::value_error_not_in_tuple())
}

/// Implements Python's `tuple.count(value)` method.
///
/// Returns the number of occurrences of value in the tuple.
fn tuple_count<'h>(
    tuple: &HeapRead<'h, Tuple>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_one_arg("tuple.count", vm.heap)?;
    defer_drop!(value, vm);

    let mut count = 0usize;
    let iter = tuple.iter(vm)?;
    defer_drop_mut!(iter, vm);
    while let Some(item) = iter.next(vm)? {
        if value.py_eq(item, vm)? {
            count += 1;
        }
    }

    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}
