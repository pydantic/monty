use std::{cmp::Ordering, fmt::Write, mem};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use super::{CmpOrder, PyTrait, iter::collect_owned_iterable};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput, HeapReader},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    sorting::parse_and_sort,
    types::{
        LazyHeapSet, Type,
        slice::{normalize_sequence_index, slice_collect_iterator},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// Python list type, wrapping a Vec of Values.
///
/// This type provides Python list semantics including dynamic growth,
/// reference counting for heap values, and standard list methods.
///
/// # Implemented Methods
/// - `append(item)` - Add item to end
/// - `insert(index, item)` - Insert item at index
/// - `pop([index])` - Remove and return item (default: last)
/// - `remove(value)` - Remove first occurrence of value
/// - `clear()` - Remove all items
/// - `copy()` - Shallow copy
/// - `extend(iterable)` - Append items from iterable
/// - `index(value[, start[, end]])` - Find first index of value
/// - `count(value)` - Count occurrences
/// - `reverse()` - Reverse in place
/// - `sort([key][, reverse])` - Sort in place
///
/// Note: `sort(key=...)` supports builtin key functions (len, abs, etc.)
/// but not user-defined functions. This is handled at VM level for access
/// to function calling machinery.
///
/// All list methods from Python's builtins are implemented.
///
/// # Reference Counting
/// When values are added to the list (via append, insert, etc.), their
/// reference counts are incremented if they are heap-allocated (Ref variants).
/// This ensures values remain valid while referenced by the list.
///
/// # GC Optimization
/// The `contains_refs` flag tracks whether the list contains any `Value::Ref` items.
/// This allows `collect_child_ids` and `py_dec_ref_ids` to skip iteration when the
/// list contains only primitive values (ints, bools, None, etc.), significantly
/// improving GC performance for lists of primitives.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct List {
    items: Vec<Value>,
    /// True if any item in the list is a `Value::Ref`. Used to skip iteration
    /// in `collect_child_ids` and `py_dec_ref_ids` when no refs are present.
    contains_refs: bool,
}

impl List {
    /// Creates a new list from a vector of values.
    ///
    /// Automatically computes the `contains_refs` flag by checking if any value
    /// is a `Value::Ref`.
    ///
    /// Note: This does NOT increment reference counts - the caller must
    /// ensure refcounts are properly managed.
    #[must_use]
    pub fn new(vec: Vec<Value>) -> Self {
        let contains_refs = vec.iter().any(|v| matches!(v, Value::Ref(_)));
        Self {
            items: vec,
            contains_refs,
        }
    }

    /// Returns a reference to the underlying vector.
    #[must_use]
    pub fn as_slice(&self) -> &[Value] {
        &self.items
    }

    /// Returns a mutable reference to the underlying vector.
    ///
    /// # Safety Considerations
    /// Be careful when mutating the vector directly - you must manually
    /// manage reference counts for any heap values you add or remove.
    /// The `contains_refs` flag is NOT automatically updated by direct
    /// vector mutations. Prefer using `append()` or `insert()` instead.
    pub fn as_vec_mut(&mut self) -> &mut Vec<Value> {
        &mut self.items
    }

    /// Returns the number of elements in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns whether the list contains any heap references.
    ///
    /// When false, `collect_child_ids` and `py_dec_ref_ids` can skip iteration.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// Marks that the list contains heap references.
    ///
    /// This should be called when directly mutating the list's items vector
    /// (via `as_vec_mut()`) with values that include `Value::Ref` variants.
    #[inline]
    pub fn set_contains_refs(&mut self) {
        self.contains_refs = true;
    }
}

impl<'h> HeapRead<'h, List> {
    /// Appends an element to the end of the list.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    pub fn append(&mut self, vm: &mut VM<'h, impl ResourceTracker>, item: Value) -> RunResult<()> {
        // Check memory limit before growing the internal Vec
        vm.heap.track_growth(VALUE_SIZE)?;
        // Track whether the list now contains heap refs so child-walk fast paths
        // can short-circuit; cycle-collector seeding is handled by `dec_ref`,
        // not at mutation time.
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        // Ownership transfer - refcount was already handled by caller
        self.get_mut(vm.heap).items.push(item);
        Ok(())
    }

    /// Inserts an element at the specified index.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented.
    ///
    /// # Arguments
    /// * `index` - The position to insert at (0-based). If index >= len(),
    ///   the item is appended to the end (matching Python semantics).
    pub fn insert(&mut self, vm: &mut VM<'h, impl ResourceTracker>, index: usize, item: Value) -> RunResult<()> {
        // Check memory limit before growing the internal Vec
        vm.heap.track_growth(VALUE_SIZE)?;
        // Track whether the list now contains heap refs so child-walk fast paths
        // can short-circuit; cycle-collector seeding is handled by `dec_ref`.
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        // Ownership transfer - refcount was already handled by caller
        // Python's insert() appends if index is out of bounds
        let this = self.get_mut(vm.heap);
        if index >= this.items.len() {
            this.items.push(item);
        } else {
            this.items.insert(index, item);
        }
        Ok(())
    }
}

impl List {
    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list", vm.heap)?;
        match value {
            None => {
                let heap_id = vm.heap.allocate(HeapData::List(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let items = collect_owned_iterable(v, vm)?;
                let heap_id = vm.heap.allocate(HeapData::List(Self::new(items)))?;
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl<'h> HeapRead<'h, List> {
    /// Handles slice-based indexing for lists.
    ///
    /// Returns a new list containing the selected elements.
    fn getitem_slice(&self, slice: &super::Slice, vm: &VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let items = slice_collect_iterator(vm, slice, self.get(vm.heap).items.iter(), |item| {
            item.clone_with_heap(vm.heap)
        })?;
        let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
        Ok(Value::Ref(heap_id))
    }

    /// Clones the item at the given index with proper refcount management.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h, impl ResourceTracker>) -> Value {
        self.get(vm.heap).items[index].clone_with_heap(vm.heap)
    }

    /// Lexicographic comparison for lists — the ordering behind `<`/`<=`/`>`/`>=`.
    ///
    /// Element-by-element left-to-right; the first non-equal pair decides. If all
    /// compared elements are equal, the shorter list is less (`[1] < [1, 2]`).
    /// The first differing pair's [`CmpOrder`] propagates: a `NaN` element makes
    /// the list [`CmpOrder::Unordered`] (`[nan] < [1]` is `False`), a
    /// type-mismatched element makes it [`CmpOrder::Incomparable`] (`[1] < ['a']`
    /// raises `TypeError`). Mirrors [`Tuple::py_cmp`](super::Tuple) — the
    /// `ListIter` holds the recursion token that bounds nested-container depth.
    pub(crate) fn py_cmp(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CmpOrder> {
        let a_len = self.get(vm.heap).items.len();
        let b_len = other.get(vm.heap).items.len();
        let min_len = a_len.min(b_len);
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, av)) = iter.next_with_index(vm)? {
            if i >= min_len {
                break;
            }
            let bv = other.clone_item(i, vm);
            defer_drop!(bv, vm);
            match av.py_cmp(bv, vm)? {
                CmpOrder::Ordered(Ordering::Equal) => {}
                CmpOrder::Ordered(ord) => return Ok(CmpOrder::Ordered(ord)),
                // A `NaN` element is never `==`-equal, so it is the first
                // differing pair and the list is unordered (yields `False`).
                CmpOrder::Unordered => return Ok(CmpOrder::Unordered),
                // CPython checks `__eq__` first and only orders non-equal pairs, so
                // equal-but-unorderable elements (e.g. `None == None`) don't block.
                CmpOrder::Incomparable => {
                    if !av.py_eq(bv, vm)? {
                        return Ok(CmpOrder::Incomparable);
                    }
                }
            }
        }
        Ok(CmpOrder::Ordered(a_len.cmp(&b_len)))
    }

    /// Clones all items from this list with proper refcount management.
    fn clone_all_items(&self, vm: &mut VM<'h, impl ResourceTracker>) -> Vec<Value> {
        let len = self.get(vm.heap).items.len();
        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            result.push(self.clone_item(i, vm));
        }
        result
    }

    /// Returns a stack-borrowed lending iterator over the list's items,
    /// holding a recursion-depth token for its entire lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`ListIter`]) because that's the obvious entry point for "iterate
    /// this container" — the lending shape is documented on the returned
    /// type, and exposing `next(vm)` makes it self-evident the result is
    /// not a [`Iterator`](core::iter::Iterator).
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter<R: ResourceTracker>(&self, vm: &mut VM<'h, R>) -> RunResult<ListIter<'_, 'h>> {
        ListIter::new(self, vm)
    }
}

/// Stack-borrowed lending iterator over a [`List`]'s items.
///
/// Borrows a [`HeapRead`] for its lifetime, so the heap entry is pinned by the
/// reader count for the duration of iteration — no extra refcount on the
/// container is needed.
///
/// **Lending shape.** [`next`](Self::next) returns `Option<&Value>` rather
/// than `Option<Value>`. The iterator itself owns the most-recently-yielded
/// item in its `current` slot (using [`Value::Undefined`] as the empty
/// sentinel) and drops the previous item at the start of each `next` call.
/// The held item is also dropped when the iterator itself is released via
/// [`DropWithContext`]. This means call sites do **not** need a per-item
/// `defer_drop!`; the iter manages every item it hands out.
///
/// **Recursion guard.** Acquires a [`RecursionToken`] at construction and
/// releases it via [`DropWithContext`]. The iterator MUST be wrapped in
/// [`defer_drop_mut!`] so the token (and any in-flight item) is released on
/// every exit path (success, early `return`, error via `?`). The token is
/// intentionally non-optional — every iteration of a Python container can
/// transitively trigger `py_eq` / `py_hash` / `py_repr` / `py_cmp` /
/// dict-or-set membership, all of which recurse on cyclic structures.
///
/// **Mutation safety.** The list length is re-read on every call to `next`,
/// so items appended during iteration may be visited and shrinking past the
/// current index halts iteration cleanly rather than panicking on an
/// out-of-bounds index.
pub(crate) struct ListIter<'a, 'h> {
    list: &'a HeapRead<'h, List>,
    index: usize,
    token: RecursionToken,
    /// Most-recently-yielded item. `Value::Undefined` when nothing is held —
    /// drops on that variant are no-ops, so `next` can unconditionally drop
    /// the previous slot before fetching the new one.
    current: Value,
}

impl<'a, 'h> ListIter<'a, 'h> {
    fn new<R: ResourceTracker>(list: &'a HeapRead<'h, List>, vm: &mut VM<'h, R>) -> RunResult<Self> {
        let token = vm.recursion_token()?;
        Ok(Self {
            list,
            index: 0,
            token,
            current: Value::Undefined,
        })
    }

    /// Advances the iterator and returns a borrow of the next item, or
    /// `Ok(None)` when the list is exhausted (or has shrunk below the
    /// current index).
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
        if self.index >= self.list.get(vm.heap).len() {
            return Ok(None);
        }
        self.current = self.list.get(vm.heap).items[self.index].clone_with_heap(vm.heap);
        self.index += 1;
        Ok(Some(&self.current))
    }

    /// Like [`next`](Self::next), but also returns the 0-based position of
    /// the yielded item — useful for `zip`-style sibling-container access,
    /// returning the matching position from search methods, or applying
    /// per-position range checks.
    ///
    /// The position is the index of the item being returned (not the next
    /// one to read), so the first yielded item has position 0.
    pub(crate) fn next_with_index<'i, R: ResourceTracker>(
        &'i mut self,
        vm: &mut VM<'h, R>,
    ) -> RunResult<Option<(usize, &'i Value)>> {
        // Capture before `next` increments `self.index`.
        let position = self.index;
        Ok(self.next(vm)?.map(|item| (position, item)))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for ListIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, List> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::List
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).items.len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            return self.getitem_slice(slice, vm);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::List)?;

        // Convert to usize, handling negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_index_error());
        }

        // Return clone of the item with proper refcount increment
        // Safety: normalized_index is validated to be in [0, len) above
        let idx = usize::try_from(normalized_index).expect("list index validated non-negative");
        Ok(self.get(vm.heap).items[idx].clone_with_heap(vm))
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop_mut!(value, vm);

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt.
        // Note: The LongInt-to-i64 conversion is defensive code. In normal execution,
        // heap-allocated LongInt values always exceed i64 range because into_value()
        // demotes i64-fitting values to Value::Int. However, this could be reached via
        // deserialization of crafted snapshot data.
        let index = match *key {
            Value::Int(i) => i,
            Value::Bool(b) => i64::from(b),
            Value::Ref(heap_id) => {
                if let HeapData::LongInt(li) = vm.heap.get(heap_id) {
                    if let Some(i) = li.to_i64() {
                        i
                    } else {
                        return Err(ExcType::index_error_int_too_large());
                    }
                } else {
                    let key_type = key.py_type_name(vm);
                    return Err(ExcType::type_error_list_assignment_indices(&key_type));
                }
            }
            _ => {
                let key_type = key.py_type_name(vm);
                return Err(ExcType::type_error_list_assignment_indices(&key_type));
            }
        };

        // Normalize negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_assignment_index_error());
        }

        let idx = usize::try_from(normalized_index).expect("index validated non-negative");

        // Update contains_refs if storing a Ref (must check before swap,
        // since after swap `value` holds the old item)
        if matches!(*value, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }

        // Replace value (old one dropped by defer_drop_mut guard)
        mem::swap(&mut self.get_mut(vm.heap).items[idx], value);

        Ok(())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::List(other)) = other.read_heap(vm) else {
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

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        let len = self.get(vm.heap).len();
        repr_sequence_fmt('[', ']', len, |heap, i| &self.get(heap).as_slice()[i], f, vm, heap_ids)
    }

    fn py_add(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let mut items = self.clone_all_items(vm);
        items.extend(other.clone_all_items(vm));
        let id = vm.heap.allocate(HeapData::List(List::new(items)))?;
        Ok(Some(Value::Ref(id)))
    }

    fn py_iadd(
        &mut self,
        other: &Value,
        vm: &mut VM<'h, impl ResourceTracker>,
        self_id: Option<HeapId>,
    ) -> Result<bool, ResourceError> {
        let Value::Ref(other_id) = other else {
            return Ok(false);
        };

        if Some(*other_id) == self_id {
            // Self-extend: clone our own items with proper refcounting
            let items = self.clone_all_items(vm);
            // Check memory limit before extending
            vm.heap.track_growth(items.len() * VALUE_SIZE)?;
            self.get_mut(vm.heap).items.extend(items);
        } else {
            // Pre-check memory limit before extending from the other list.
            // Read source list via HeapRead, clone items into a temporary Vec
            let source = vm.heap.read(*other_id);
            let HeapReadOutput::List(source_list) = source else {
                return Ok(false);
            };
            let source_len = source_list.get(vm.heap).len();
            vm.heap.track_growth(source_len * VALUE_SIZE)?;
            let source_items = source_list.clone_all_items(vm);
            // Check if new items contain refs
            let has_new_refs = source_items.iter().any(|v| matches!(v, Value::Ref(_)));
            self.get_mut(vm.heap).items.extend(source_items);
            if has_new_refs {
                self.get_mut(vm.heap).contains_refs = true;
            }
        }

        Ok(true)
    }

    /// Delegates methods to `call_list_method`.
    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        if attr.static_string() == Some(StaticStrings::Sort) {
            do_list_sort(self, args, vm)?;
            return Ok(CallResult::Value(Value::None));
        }

        let Some(method) = attr.static_string() else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::List, attr.as_str(vm.interns)));
        };

        call_list_method(self, method, args, vm).map(CallResult::Value)
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let list_id = self_id.expect("heap values have an id");
        let iterator = vm.heap.allocate(HeapData::ListIterator(ListIterator {
            list: list_id,
            index: 0,
        }))?;
        vm.heap.inc_ref(list_id);
        Ok(Value::Ref(iterator))
    }
}

impl HeapItem for List {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.items.len() * VALUE_SIZE
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - major GC optimization for lists of primitives
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

/// Dispatches a method call on a list value.
///
/// This is the unified entry point for list method calls.
///
/// # Arguments
/// * `list` - The list to call the method on
/// * `method` - The method to call (e.g., `StaticStrings::Append`)
/// * `args` - The method arguments
/// * `heap` - The heap for allocation and reference counting
fn call_list_method<'h>(
    list: &mut HeapRead<'h, List>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let heap = &mut *vm.heap;
    match method {
        StaticStrings::Append => {
            let item = args.get_one_arg("list.append", heap)?;
            list.append(vm, item)?;
            Ok(Value::None)
        }
        StaticStrings::Insert => list_insert(list, args, vm),
        StaticStrings::Pop => list_pop(list, args, vm),
        StaticStrings::Remove => list_remove(list, args, vm),
        StaticStrings::Clear => {
            args.check_zero_args("list.clear", heap)?;
            list_clear(list, vm);
            Ok(Value::None)
        }
        StaticStrings::Copy => {
            args.check_zero_args("list.copy", heap)?;
            Ok(list_copy(list.get(heap), heap)?)
        }
        StaticStrings::Extend => list_extend(list, args, vm),
        StaticStrings::Index => list_index(list, args, vm),
        StaticStrings::Count => list_count(list, args, vm),
        StaticStrings::Reverse => {
            args.check_zero_args("list.reverse", heap)?;
            list.get_mut(vm.heap).items.reverse();
            Ok(Value::None)
        }
        // Note: list.sort is handled by py_call_attr which intercepts it before reaching here
        _ => {
            args.drop_with(heap);
            Err(ExcType::attribute_error(Type::List, method.into()))
        }
    }
}

/// Implements Python's `list.insert(index, item)` method.
fn list_insert<'h>(
    list: &mut HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (index_obj, item) = args.get_two_args("insert", vm.heap)?;
    defer_drop!(index_obj, vm);
    let mut item_guard = DropGuard::new(item, vm);
    let vm = item_guard.ctx();
    // Python's insert() handles negative indices by adding len
    // If still negative after adding len, clamps to 0
    // If >= len, appends to end
    let index_i64 = index_obj.as_int(vm)?;
    let len = list.get(vm.heap).items.len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let index = if index_i64 < 0 {
        // Negative index: add length, clamp to 0 if still negative
        let adjusted = index_i64 + len_i64;
        usize::try_from(adjusted).unwrap_or(0)
    } else {
        // Positive index: clamp to len if too large
        usize::try_from(index_i64).unwrap_or(len)
    };
    let (item, heap) = item_guard.into_parts();
    list.insert(heap, index, item)?;
    Ok(Value::None)
}

/// Implements Python's `list.pop([index])` method.
///
/// Removes the item at the given index (default: -1) and returns it.
/// Raises IndexError if the list is empty or the index is out of range.
fn list_pop<'h>(
    list: &mut HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let index_arg = args.get_zero_one_arg("list.pop", vm.heap)?;

    // Validate index type FIRST (if provided), matching Python's validation order.
    // Python raises TypeError for bad index type even on empty list.
    let index_i64 = if let Some(v) = index_arg {
        let result = v.as_int(vm);
        v.drop_with(vm);
        result?
    } else {
        -1
    };

    // THEN check empty list
    if list.get(vm.heap).items.is_empty() {
        return Err(ExcType::index_error_pop_empty_list());
    }

    // Normalize index
    let len = list.get(vm.heap).items.len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let normalized = if index_i64 < 0 { index_i64 + len_i64 } else { index_i64 };

    // Bounds check
    if normalized < 0 || normalized >= len_i64 {
        return Err(ExcType::index_error_pop_out_of_range());
    }

    // Remove and return the item
    let idx = usize::try_from(normalized).expect("index validated non-negative");
    Ok(list.get_mut(vm.heap).items.remove(idx))
}

/// Implements Python's `list.remove(value)` method.
///
/// Removes the first occurrence of value. Raises ValueError if not found.
fn list_remove<'h>(
    list: &mut HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_one_arg("list.remove", vm.heap)?;
    defer_drop!(value, vm);

    let mut found_idx = None;
    {
        let iter = list.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, item)) = iter.next_with_index(vm)? {
            if value.py_eq(item, vm)? {
                found_idx = Some(i);
                break;
            }
        }
    }

    match found_idx {
        Some(idx) => {
            // Remove the element and drop its refcount
            let removed = list.get_mut(vm.heap).items.remove(idx);
            removed.drop_with(vm.heap);
            Ok(Value::None)
        }
        None => Err(ExcType::value_error_remove_not_in_list()),
    }
}

/// Implements Python's `list.clear()` method.
///
/// Removes all items from the list.
fn list_clear<'h>(list: &mut HeapRead<'h, List>, vm: &mut VM<'h, impl ResourceTracker>) {
    mem::take(&mut list.get_mut(vm.heap).items).drop_with(vm);
    // Note: contains_refs stays true even if all refs removed, per conservative GC strategy
}

/// Implements Python's `list.copy()` method.
///
/// Returns a shallow copy of the list.
fn list_copy(list: &List, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    let items: Vec<Value> = list.items.iter().map(|v| v.clone_with_heap(heap)).collect();
    let heap_id = heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `list.extend(iterable)` method.
///
/// Extends the list by appending all items from the iterable.
fn list_extend<'h>(
    list: &mut HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let iterable = args.get_one_arg("list.extend", vm.heap)?;
    let items: SmallVec<[_; 2]> = collect_owned_iterable(iterable, vm)?;

    // Batch memory check for all items at once, then extend
    vm.heap.track_growth(items.len() * VALUE_SIZE)?;
    let has_refs = items.iter().any(|v| matches!(v, Value::Ref(_)));
    if has_refs {
        list.get_mut(vm.heap).set_contains_refs();
    }
    list.get_mut(vm.heap).as_vec_mut().extend(items);

    Ok(Value::None)
}

/// Implements Python's `list.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn list_index<'h>(
    list: &HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let pos_args = args.into_pos_only("list.index", vm.heap)?;
    defer_drop!(pos_args, vm);

    let len = list.get(vm.heap).items.len();
    let (value, start, end) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("list.index", 1, 0)),
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
        other => return Err(ExcType::type_error_at_most("list.index", 3, other.len())),
    };

    // Search for the value in the specified range
    let iter = list.iter(vm)?;
    defer_drop_mut!(iter, vm);
    while let Some((idx, item)) = iter.next_with_index(vm)? {
        if idx >= end {
            // No further matches possible inside [start, end).
            break;
        }
        if idx >= start && value.py_eq(item, vm)? {
            let i64_idx = i64::try_from(idx).expect("index exceeds i64::MAX");
            return Ok(Value::Int(i64_idx));
        }
    }
    Err(ExcType::value_error_not_in_list())
}

/// Implements Python's `list.count(value)` method.
///
/// Returns the number of occurrences of value in the list.
fn list_count<'h>(
    list: &HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_one_arg("list.count", vm.heap)?;
    defer_drop!(value, vm);

    let mut count: usize = 0;
    let iter = list.iter(vm)?;
    defer_drop_mut!(iter, vm);
    while let Some(item) = iter.next(vm)? {
        if value.py_eq(item, vm)? {
            count += 1;
        }
    }

    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}

/// Performs an in-place sort on a list with optional key function and reverse flag.
///
/// To safely support user-supplied `key` callbacks (and rich-comparison `__lt__`
/// methods) that may reentrantly mutate the same list, we follow CPython's
/// strategy: the list's `items` vector is **detached** for the duration of the
/// sort so the list looks empty to any reentrant code. All sort work is then
/// performed on the detached buffer, which is always swapped back into the
/// list afterwards. If the user mutated the live (empty) list during the
/// sort, we additionally raise `ValueError: list modified during sort`,
/// matching CPython exactly.
fn do_list_sort<'h>(
    list: &mut HeapRead<'h, List>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> Result<(), RunError> {
    // Detach the list's items so reentrant access via the list's heap id sees
    // an empty list. The detached buffer is always swapped back into the list
    // when we're done. Done *before* parsing args so the reentrancy guard is
    // in place if parsing somehow allocates or drops user values that run
    // arbitrary code.
    let items = mem::take(&mut list.get_mut(vm.heap).items);
    defer_drop_mut!(items, vm);

    let sort_result = parse_and_sort(items, args, vm);

    // Swap our (sorted) buffer back into the list. Whatever the user placed
    // on the live empty list during the sort ends up in `items`; if
    // it's not empty, the user mutated the list. The `contains_refs` flag
    // survives `mem::take`, so it still describes the buffer being swapped
    // back.
    mem::swap(list.get_mut(vm.heap).as_vec_mut(), items);

    // Surface any sort error first; otherwise the modification error (if any).
    sort_result?;
    if items.is_empty() {
        Ok(())
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "list modified during sort").into())
    }
}

/// Writes a formatted sequence of values to a formatter.
///
/// This helper function is used to implement `__repr__` for sequence types like
/// lists and tuples. It writes items as comma-separated repr interns.
///
/// # Arguments
/// * `start` - The opening character (e.g., '[' for lists, '(' for tuples)
/// * `end` - The closing character (e.g., ']' for lists, ')' for tuples)
/// * `len` - The number of items to format
/// * `get_item` - Returns the i-th value via brief immutable heap access
/// * `f` - The formatter to write to
/// * `vm` - The VM for resolving value references and looking up interned strings
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
pub(crate) fn repr_sequence_fmt<'h, T: ResourceTracker>(
    start: char,
    end: char,
    len: usize,
    get_item: impl for<'r> Fn(&'r HeapReader<'h, T>, usize) -> &'r Value,
    f: &mut impl Write,
    vm: &mut VM<'h, T>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    // Check depth limit before recursing
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;

    f.write_char(start)?;
    for i in 0..len {
        if i > 0 {
            if vm.heap.check_time().is_err() {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        let item = get_item(vm.heap, i).clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    f.write_char(end)?;

    Ok(())
}

/// Iterates over a list while observing changes to its current contents.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ListIterator {
    /// Owned reference to the list under iteration.
    list: HeapId,
    /// Index of the next item to yield.
    index: usize,
}

impl ListIterator {
    /// Returns the list kept alive by this iterator.
    pub(crate) fn list_id(&self) -> HeapId {
        self.list
    }

    /// Returns the number of items remaining in the list's current contents.
    pub(crate) fn size_hint(&self, heap: &Heap<impl ResourceTracker>) -> usize {
        let HeapData::List(list) = heap.get(self.list) else {
            unreachable!("list iterator must reference a list")
        };
        list.len().saturating_sub(self.index)
    }
}

impl HeapItem for ListIterator {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.list);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, ListIterator> {
    fn py_type(&self, _: &VM<'h, impl ResourceTracker>) -> Type {
        Type::ListIterator
    }

    fn py_len(&self, _: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let self_id = self_id.expect("heap values have an id");
        vm.heap.inc_ref(self_id);
        Ok(Value::Ref(self_id))
    }

    fn py_next(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        let (list_id, index) = {
            let iterator = self.get(vm.heap);
            (iterator.list, iterator.index)
        };
        let item = match vm.heap.get(list_id) {
            HeapData::List(list) => list.items.get(index).map(|item| item.clone_with_heap(vm.heap)),
            _ => unreachable!("list iterator must reference a list"),
        };
        if item.is_some() {
            self.get_mut(vm.heap).index += 1;
        }
        Ok(item)
    }
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;

    use super::*;
    use crate::{
        PrintWriter,
        heap::{Heap, HeapReader},
        intern::{InternerBuilder, Interns},
        resource::NoLimitTracker,
        run::AssertMessageAnnotations,
        types::LongInt,
    };

    /// Creates a minimal Interns for testing.
    fn create_test_interns() -> Interns {
        let interner = InternerBuilder::new("");
        Interns::new(interner, vec![])
    }

    /// Creates a heap with a list and a LongInt index, bypassing into_value() demotion.
    ///
    /// This allows testing the defensive code path where a LongInt contains an i64-fitting value.
    fn create_heap_with_list_and_longint(
        list_items: Vec<Value>,
        index_value: BigInt,
    ) -> (Heap<NoLimitTracker>, HeapId, HeapId) {
        let heap = Heap::new(16, NoLimitTracker);
        let list = List::new(list_items);
        let list_id = heap.allocate(HeapData::List(list)).unwrap();
        let long_int = LongInt::new(index_value);
        let index_id = heap.allocate(HeapData::LongInt(long_int)).unwrap();
        (heap, list_id, index_id)
    }

    /// Tests py_setitem with a LongInt index that fits in i64.
    ///
    /// This is a defensive code path - normally unreachable because LongInt::into_value()
    /// demotes i64-fitting values to Value::Int. However, it could be reached via
    /// deserialization of crafted snapshot data.
    #[test]
    fn py_setitem_longint_fits_in_i64() {
        let (mut heap, list_id, index_id) =
            create_heap_with_list_and_longint(vec![Value::Int(10), Value::Int(20), Value::Int(30)], BigInt::from(1));
        let mut interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let mut vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the list was updated by checking it matches expected Int value
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.as_slice()[1], Value::Int(99)));

        // Clean up
        Value::Ref(list_id).drop_with(&mut heap);
    }

    /// Tests py_setitem with a negative LongInt index that fits in i64.
    #[test]
    fn py_setitem_longint_negative_fits_in_i64() {
        let (mut heap, list_id, index_id) = create_heap_with_list_and_longint(
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
            BigInt::from(-1), // Last element
        );
        let mut interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let mut vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the last element was updated
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.as_slice()[2], Value::Int(99)));

        Value::Ref(list_id).drop_with(&mut heap);
    }

    /// Tests py_setitem with i64::MAX as a LongInt index.
    #[test]
    fn py_setitem_longint_at_i64_max() {
        let (mut heap, list_id, index_id) =
            create_heap_with_list_and_longint(vec![Value::Int(10)], BigInt::from(i64::MAX));
        let mut interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let mut vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_err());

        Value::Ref(list_id).drop_with(&mut heap);
    }
}
