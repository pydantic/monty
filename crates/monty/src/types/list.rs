use std::{cmp::Ordering, fmt::Write};

use ahash::AHashSet;
use smallvec::SmallVec;

use super::{MontyIter, PyTrait};
use crate::{
    args::ArgValues,
    builtins::Builtins,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    io::PrintWriter,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::Type,
    value::{EitherStr, Value},
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

    /// Appends an element to the end of the list.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// Returns `Value::None`, matching Python's behavior where `list.append()` returns None.
    pub fn append(&mut self, heap: &mut Heap<impl ResourceTracker>, item: Value) {
        // Track if we're adding a reference and mark potential cycle
        if matches!(item, Value::Ref(_)) {
            self.contains_refs = true;
            heap.mark_potential_cycle();
        }
        // Ownership transfer - refcount was already handled by caller
        self.items.push(item);
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
    ///
    /// Returns `Value::None`, matching Python's behavior where `list.insert()` returns None.
    pub fn insert(&mut self, heap: &mut Heap<impl ResourceTracker>, index: usize, item: Value) {
        // Track if we're adding a reference and mark potential cycle
        if matches!(item, Value::Ref(_)) {
            self.contains_refs = true;
            heap.mark_potential_cycle();
        }
        // Ownership transfer - refcount was already handled by caller
        // Python's insert() appends if index is out of bounds
        if index >= self.items.len() {
            self.items.push(item);
        } else {
            self.items.insert(index, item);
        }
    }

    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list", heap)?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::List(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let mut iter = MontyIter::new(v, heap, interns)?;
                let items = iter.collect(heap, interns)?;
                iter.drop_with_heap(heap);
                let heap_id = heap.allocate(HeapData::List(Self::new(items)))?;
                Ok(Value::Ref(heap_id))
            }
        }
    }

    /// Handles slice-based indexing for lists.
    ///
    /// Returns a new list containing the selected elements.
    fn getitem_slice(&self, slice: &crate::types::Slice, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let (start, stop, step) = slice
            .indices(self.items.len())
            .map_err(|()| ExcType::value_error_slice_step_zero())?;

        let items = get_slice_items(&self.items, start, stop, step, heap);
        let heap_id = heap.allocate(HeapData::List(Self::new(items)))?;
        Ok(Value::Ref(heap_id))
    }
}

impl From<List> for Vec<Value> {
    fn from(list: List) -> Self {
        list.items
    }
}

impl PyTrait for List {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::List
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.items.len() * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.items.len())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = heap.get(*id)
        {
            // Clone the slice to release the borrow on heap before calling getitem_slice
            let slice = slice.clone();
            return self.getitem_slice(&slice, heap);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(heap, Type::List)?;

        // Convert to usize, handling negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.items.len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_index_error());
        }

        // Return clone of the item with proper refcount increment
        // Safety: normalized_index is validated to be in [0, len) above
        let idx = usize::try_from(normalized_index).expect("list index validated non-negative");
        Ok(self.items[idx].clone_with_heap(heap))
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<()> {
        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt.
        // Note: The LongInt-to-i64 conversion is defensive code. In normal execution,
        // heap-allocated LongInt values always exceed i64 range because into_value()
        // demotes i64-fitting values to Value::Int. However, this could be reached via
        // deserialization of crafted snapshot data.
        let index = match &key {
            Value::Int(i) => *i,
            Value::Bool(b) => i64::from(*b),
            Value::Ref(heap_id) => {
                if let HeapData::LongInt(li) = heap.get(*heap_id) {
                    if let Some(i) = li.to_i64() {
                        i
                    } else {
                        key.drop_with_heap(heap);
                        value.drop_with_heap(heap);
                        return Err(ExcType::index_error_int_too_large());
                    }
                } else {
                    let key_type = key.py_type(heap);
                    key.drop_with_heap(heap);
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error_list_assignment_indices(key_type));
                }
            }
            _ => {
                let key_type = key.py_type(heap);
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(ExcType::type_error_list_assignment_indices(key_type));
            }
        };
        // Drop the key after extracting the index (Int and Bool are not ref-counted)
        key.drop_with_heap(heap);

        // Normalize negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.items.len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            value.drop_with_heap(heap);
            return Err(ExcType::list_assignment_index_error());
        }

        // Replace value, drop old one
        let idx = usize::try_from(normalized_index).expect("index validated non-negative");
        let old_value = std::mem::replace(&mut self.items[idx], value);
        old_value.drop_with_heap(heap);

        // Update contains_refs if adding a Ref
        if matches!(self.items[idx], Value::Ref(_)) {
            self.contains_refs = true;
            heap.mark_potential_cycle();
        }

        Ok(())
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        if self.items.len() != other.items.len() {
            return Ok(false);
        }
        guard.increase_err()?;

        for (i1, i2) in self.items.iter().zip(&other.items) {
            if !i1.py_eq(i2, heap, guard, interns)? {
                guard.decrease();
                return Ok(false);
            }
        }
        guard.decrease();
        Ok(true)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - major GC optimization for lists of primitives
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                obj.dec_ref_forget();
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> std::fmt::Result {
        repr_sequence_fmt('[', ']', &self.items, f, heap, heap_ids, guard, interns)
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // Clone both lists' contents with proper refcounting
        let mut result: Vec<Value> = self.items.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        let other_cloned: Vec<Value> = other.items.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        result.extend(other_cloned);
        let id = heap.allocate(HeapData::List(Self::new(result)))?;
        Ok(Some(Value::Ref(id)))
    }

    fn py_iadd(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        self_id: Option<HeapId>,
        _interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        // Extract the value ID first, keeping `other` around to drop later
        let Value::Ref(other_id) = &other else { return Ok(false) };

        if Some(*other_id) == self_id {
            // Self-extend: clone our own items with proper refcounting
            let items = self
                .items
                .iter()
                .map(|obj| obj.clone_with_heap(heap))
                .collect::<Vec<_>>();
            // If we're self-extending and have refs, mark potential cycle
            if self.contains_refs {
                heap.mark_potential_cycle();
            }
            self.items.extend(items);
        } else {
            // Get items from other list using iadd_extend_from_heap helper
            // This handles the borrow checker limitations with lifetime propagation
            let prev_len = self.items.len();
            if !heap.iadd_extend_list(*other_id, &mut self.items) {
                return Ok(false);
            }
            // Check if we added any refs and mark potential cycle
            if self.contains_refs {
                // Already had refs, but adding more may create cycles
                heap.mark_potential_cycle();
            } else {
                for item in &self.items[prev_len..] {
                    if matches!(item, Value::Ref(_)) {
                        self.contains_refs = true;
                        heap.mark_potential_cycle();
                        break;
                    }
                }
            }
        }

        // Drop the other value - we've extracted its contents and are done with the temporary reference
        other.drop_with_heap(heap);
        Ok(true)
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(heap);
            return Err(ExcType::attribute_error(Type::List, attr.as_str(interns)));
        };

        call_list_method(self, method, args, heap, interns)
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
/// * `interns` - The interns table for resolving interned strings
fn call_list_method(
    list: &mut List,
    method: StaticStrings,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    match method {
        StaticStrings::Append => {
            let item = args.get_one_arg("list.append", heap)?;
            list.append(heap, item);
            Ok(Value::None)
        }
        StaticStrings::Insert => list_insert(list, args, heap),
        StaticStrings::Pop => list_pop(list, args, heap),
        StaticStrings::Remove => list_remove(list, args, heap, interns),
        StaticStrings::Clear => {
            args.check_zero_args("list.clear", heap)?;
            list_clear(list, heap);
            Ok(Value::None)
        }
        StaticStrings::Copy => {
            args.check_zero_args("list.copy", heap)?;
            Ok(list_copy(list, heap)?)
        }
        StaticStrings::Extend => list_extend(list, args, heap, interns),
        StaticStrings::Index => list_index(list, args, heap, interns),
        StaticStrings::Count => list_count(list, args, heap, interns),
        StaticStrings::Reverse => {
            args.check_zero_args("list.reverse", heap)?;
            list.items.reverse();
            Ok(Value::None)
        }
        // Note: list.sort is handled at VM level in call.rs to support key functions
        _ => {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::List, method.into()))
        }
    }
}

/// Implements Python's `list.insert(index, item)` method.
fn list_insert(list: &mut List, args: ArgValues, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let (index_obj, item) = args.get_two_args("insert", heap)?;
    // Python's insert() handles negative indices by adding len
    // If still negative after adding len, clamps to 0
    // If >= len, appends to end
    let index_result = index_obj.as_int(heap);
    // Drop index_obj before propagating error - it could be a Ref (e.g., dict)
    index_obj.drop_with_heap(heap);
    let index_i64 = match index_result {
        Ok(i) => i,
        Err(e) => {
            item.drop_with_heap(heap);
            return Err(e);
        }
    };
    let len = list.items.len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let index = if index_i64 < 0 {
        // Negative index: add length, clamp to 0 if still negative
        let adjusted = index_i64 + len_i64;
        usize::try_from(adjusted).unwrap_or(0)
    } else {
        // Positive index: clamp to len if too large
        usize::try_from(index_i64).unwrap_or(len)
    };
    list.insert(heap, index, item);
    Ok(Value::None)
}

/// Implements Python's `list.pop([index])` method.
///
/// Removes the item at the given index (default: -1) and returns it.
/// Raises IndexError if the list is empty or the index is out of range.
fn list_pop(list: &mut List, args: ArgValues, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let index_arg = args.get_zero_one_arg("list.pop", heap)?;

    // Validate index type FIRST (if provided), matching Python's validation order.
    // Python raises TypeError for bad index type even on empty list.
    let index_i64 = if let Some(v) = index_arg {
        let result = v.as_int(heap);
        v.drop_with_heap(heap);
        result?
    } else {
        -1
    };

    // THEN check empty list
    if list.items.is_empty() {
        return Err(ExcType::index_error_pop_empty_list());
    }

    // Normalize index
    let len = list.items.len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let normalized = if index_i64 < 0 { index_i64 + len_i64 } else { index_i64 };

    // Bounds check
    if normalized < 0 || normalized >= len_i64 {
        return Err(ExcType::index_error_pop_out_of_range());
    }

    // Remove and return the item
    let idx = usize::try_from(normalized).expect("index validated non-negative");
    Ok(list.items.remove(idx))
}

/// Implements Python's `list.remove(value)` method.
///
/// Removes the first occurrence of value. Raises ValueError if not found.
fn list_remove(
    list: &mut List,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let value = args.get_one_arg("list.remove", heap)?;

    // Find the first matching element
    let mut found_idx = None;
    let mut guard = DepthGuard::default();
    for (i, item) in list.items.iter().enumerate() {
        if value.py_eq(item, heap, &mut guard, interns)? {
            found_idx = Some(i);
            break;
        }
    }

    value.drop_with_heap(heap);

    match found_idx {
        Some(idx) => {
            // Remove the element and drop its refcount
            let removed = list.items.remove(idx);
            removed.drop_with_heap(heap);
            Ok(Value::None)
        }
        None => Err(ExcType::value_error_remove_not_in_list()),
    }
}

/// Implements Python's `list.clear()` method.
///
/// Removes all items from the list.
fn list_clear(list: &mut List, heap: &mut Heap<impl ResourceTracker>) {
    for item in list.items.drain(..) {
        item.drop_with_heap(heap);
    }
    // Note: contains_refs stays true even if all refs removed, per conservative GC strategy
}

/// Implements Python's `list.copy()` method.
///
/// Returns a shallow copy of the list.
fn list_copy(list: &List, heap: &mut Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    let items: Vec<Value> = list.items.iter().map(|v| v.clone_with_heap(heap)).collect();
    let heap_id = heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `list.extend(iterable)` method.
///
/// Extends the list by appending all items from the iterable.
fn list_extend(
    list: &mut List,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let iterable = args.get_one_arg("list.extend", heap)?;

    // Create iterator for the iterable
    let mut iter = MontyIter::new(iterable, heap, interns)?;

    // Collect all items from the iterator
    let items: SmallVec<[_; 2]> = iter.collect(heap, interns)?;
    iter.drop_with_heap(heap);

    // Add each item to the list
    for item in items {
        list.append(heap, item);
    }

    Ok(Value::None)
}

/// Implements Python's `list.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn list_index(
    list: &List,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let pos_args = args.into_pos_only("list.index", heap)?;
    defer_drop!(pos_args, heap);

    let len = list.items.len();
    let (value, start, end) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("list.index", 1, 0)),
        [value] => (value, 0, len),
        [value, start_arg] => {
            let start = normalize_list_index(start_arg.as_int(heap)?, len);
            (value, start, len)
        }
        [value, start_arg, end_arg] => {
            let start = normalize_list_index(start_arg.as_int(heap)?, len);
            let end = normalize_list_index(end_arg.as_int(heap)?, len).max(start);
            (value, start, end)
        }
        other => return Err(ExcType::type_error_at_most("list.index", 3, other.len())),
    };

    // Search for the value in the specified range
    let mut guard = DepthGuard::default();
    for (i, item) in list.items[start..end].iter().enumerate() {
        if value.py_eq(item, heap, &mut guard, interns)? {
            let idx = i64::try_from(start + i).expect("index exceeds i64::MAX");
            return Ok(Value::Int(idx));
        }
    }

    Err(ExcType::value_error_not_in_list())
}

/// Implements Python's `list.count(value)` method.
///
/// Returns the number of occurrences of value in the list.
fn list_count(
    list: &List,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let value = args.get_one_arg("list.count", heap)?;

    // Use a local DepthGuard for py_eq calls.
    // We use unwrap_or(false) for recursion errors since filter() can't propagate Results.
    let mut guard = DepthGuard::default();
    let count = list
        .items
        .iter()
        .filter(|item| value.py_eq(item, heap, &mut guard, interns).unwrap_or(false))
        .count();

    value.drop_with_heap(heap);
    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}

/// Normalizes a Python-style list index to a valid index in range [0, len].
fn normalize_list_index(index: i64, len: usize) -> usize {
    if index < 0 {
        let abs_index = usize::try_from(-index).unwrap_or(usize::MAX);
        len.saturating_sub(abs_index)
    } else {
        usize::try_from(index).unwrap_or(len).min(len)
    }
}

/// Performs an in-place sort on a list with optional key function and reverse flag.
///
/// This is called from the VM's `call_attr` when `list.sort()` is invoked.
/// The function lives here (rather than in VM) to keep list-related logic together,
/// with the VM only passing through its resources.
///
/// Uses a staged approach to avoid borrow checker issues:
/// 1. Parse and validate arguments
/// 2. Extract items from the list (temporarily empties it)
/// 3. Compute key values if a key function is provided
/// 4. Sort indices based on items or key values
/// 5. Rearrange items in sorted order and put back into the list
///
/// # Arguments
/// * `list_id` - The heap ID of the list to sort
/// * `args` - The method arguments (keyword-only: `key` and `reverse`)
/// * `heap` - The heap for memory management
/// * `interns` - Interned strings for comparisons
/// * `print_writer` - Output writer (needed for builtin function calls)
pub(crate) fn do_list_sort(
    list_id: HeapId,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
    print_writer: &mut impl PrintWriter,
) -> Result<(), RunError> {
    // Parse keyword-only arguments: key and reverse
    let (key_arg, reverse_arg) = args.extract_two_kwargs_only("list.sort", "key", "reverse", heap, interns)?;

    // Convert reverse to bool (default false)
    let reverse = if let Some(v) = reverse_arg {
        let result = v.py_bool(heap, interns);
        v.drop_with_heap(heap);
        result
    } else {
        false
    };

    // Handle key function (None means no key function)
    let key_fn = match key_arg {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with_heap(heap);
            None
        }
        other => other,
    };

    // Step 1: Extract items from the list (temporarily empties it)
    let mut items: Vec<Value> = {
        let HeapData::List(list) = heap.get_mut(list_id) else {
            if let Some(k) = key_fn {
                k.drop_with_heap(heap);
            }
            return Err(RunError::internal("expected list in do_list_sort"));
        };
        list.as_vec_mut().drain(..).collect()
    };

    // Step 2: Compute key values if key function provided
    let key_values: Option<Vec<Value>> = if let Some(ref key) = key_fn {
        let mut keys: Vec<Value> = Vec::with_capacity(items.len());
        for item in &items {
            let elem = item.clone_with_heap(heap);
            match call_key_function(key, elem, heap, interns, print_writer) {
                Ok(key_value) => keys.push(key_value),
                Err(e) => {
                    // Clean up and restore items to list on error
                    for k in keys {
                        k.drop_with_heap(heap);
                    }
                    if let Some(k) = key_fn {
                        k.drop_with_heap(heap);
                    }
                    // Restore items to the list
                    if let HeapData::List(list) = heap.get_mut(list_id) {
                        for item in items {
                            list.as_vec_mut().push(item);
                        }
                    }
                    return Err(e);
                }
            }
        }
        Some(keys)
    } else {
        None
    };

    // Drop the key function - we're done with it
    if let Some(k) = key_fn {
        k.drop_with_heap(heap);
    }

    // Step 3: Sort indices based on items or key values
    let len = items.len();
    let mut indices: Vec<usize> = (0..len).collect();
    let mut sort_error: Option<RunError> = None;
    // Create a guard for py_cmp calls. We use a RefCell to allow mutable borrows inside the closure.
    let guard = std::cell::RefCell::new(DepthGuard::default());

    if let Some(ref keys) = key_values {
        indices.sort_by(|&a, &b| {
            if sort_error.is_some() {
                return Ordering::Equal;
            }
            match keys[a].py_cmp(&keys[b], heap, &mut guard.borrow_mut(), interns) {
                Ok(Some(ord)) => {
                    if reverse {
                        ord.reverse()
                    } else {
                        ord
                    }
                }
                Ok(None) => {
                    sort_error = Some(ExcType::type_error(format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        keys[a].py_type(heap),
                        keys[b].py_type(heap)
                    )));
                    Ordering::Equal
                }
                Err(e) => {
                    sort_error = Some(e.into());
                    Ordering::Equal
                }
            }
        });
    } else {
        indices.sort_by(|&a, &b| {
            if sort_error.is_some() {
                return Ordering::Equal;
            }
            match items[a].py_cmp(&items[b], heap, &mut guard.borrow_mut(), interns) {
                Ok(Some(ord)) => {
                    if reverse {
                        ord.reverse()
                    } else {
                        ord
                    }
                }
                Ok(None) => {
                    sort_error = Some(ExcType::type_error(format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        items[a].py_type(heap),
                        items[b].py_type(heap)
                    )));
                    Ordering::Equal
                }
                Err(e) => {
                    sort_error = Some(e.into());
                    Ordering::Equal
                }
            }
        });
    }

    // Clean up key values
    if let Some(keys) = key_values {
        for k in keys {
            k.drop_with_heap(heap);
        }
    }

    // Check for sort error
    if let Some(err) = sort_error {
        // Restore items to list before returning error
        if let HeapData::List(list) = heap.get_mut(list_id) {
            for item in items {
                list.as_vec_mut().push(item);
            }
        }
        return Err(err);
    }

    // Step 4: Rearrange items in sorted order using index permutation
    let mut sorted_items: Vec<Value> = Vec::with_capacity(len);
    for &i in &indices {
        // Move the value out, replacing with Undefined as placeholder
        sorted_items.push(std::mem::replace(&mut items[i], Value::Undefined));
    }

    // Put sorted items back into the list
    let HeapData::List(list) = heap.get_mut(list_id) else {
        return Err(RunError::internal("expected list in do_list_sort"));
    };

    for item in sorted_items {
        list.as_vec_mut().push(item);
    }

    // items now contains Undefined values - no cleanup needed
    Ok(())
}

/// Calls a key function on a single element for sorting.
///
/// Currently supports builtin functions directly. User-defined functions return
/// an error since they would require VM frame management for proper execution.
fn call_key_function(
    key_fn: &Value,
    elem: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
    print_writer: &mut impl PrintWriter,
) -> Result<Value, RunError> {
    match key_fn {
        Value::Builtin(Builtins::Function(builtin)) => {
            let args = ArgValues::One(elem);
            builtin.call(heap, args, interns, print_writer)
        }
        Value::Builtin(Builtins::Type(t)) => {
            // Type constructors (int, str, float, etc.) are callable key functions
            let args = ArgValues::One(elem);
            t.call(heap, args, interns)
        }
        Value::DefFunction(_) | Value::ExtFunction(_) | Value::Ref(_) => {
            // User-defined or external functions require VM frame management
            elem.drop_with_heap(heap);
            Err(ExcType::type_error(
                "list.sort() key argument must be a builtin function (user-defined functions not yet supported)",
            ))
        }
        _ => {
            elem.drop_with_heap(heap);
            Err(ExcType::type_error("list.sort() key must be callable or None"))
        }
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
/// * `items` - The slice of values to format
/// * `f` - The formatter to write to
/// * `heap` - The heap for resolving value references
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
/// * `guard` - Recursion depth tracker to prevent stack overflow on deeply nested structures
/// * `interns` - The interned strings table for looking up string/bytes literals
#[expect(clippy::too_many_arguments)]
pub(crate) fn repr_sequence_fmt(
    start: char,
    end: char,
    items: &[Value],
    f: &mut impl Write,
    heap: &Heap<impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
    guard: &mut DepthGuard,
    interns: &Interns,
) -> std::fmt::Result {
    // Check depth limit before recursing
    if !guard.increase() {
        return f.write_str("...");
    }

    f.write_char(start)?;
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        first.py_repr_fmt(f, heap, heap_ids, guard, interns)?;
        for item in iter {
            f.write_str(", ")?;
            item.py_repr_fmt(f, heap, heap_ids, guard, interns)?;
        }
    }
    f.write_char(end)?;

    guard.decrease();
    Ok(())
}

/// Helper to extract items from a slice for list/tuple slicing.
///
/// Handles both positive and negative step values. For negative step,
/// iterates backward from start down to (but not including) stop.
///
/// Returns a new Vec of cloned values with proper refcount increments.
///
/// Note: step must be non-zero (callers should validate this via `slice.indices()`).
pub(crate) fn get_slice_items(
    items: &[Value],
    start: usize,
    stop: usize,
    step: i64,
    heap: &mut Heap<impl ResourceTracker>,
) -> Vec<Value> {
    let mut result = Vec::new();

    // try_from succeeds for non-negative step; step==0 rejected upstream by slice.indices()
    if let Ok(step_usize) = usize::try_from(step) {
        // Positive step: iterate forward
        let mut i = start;
        while i < stop && i < items.len() {
            result.push(items[i].clone_with_heap(heap));
            i += step_usize;
        }
    } else {
        // Negative step: iterate backward
        // start is the highest index, stop is the sentinel
        // stop > items.len() means "go to the beginning"
        let step_abs = usize::try_from(-step).expect("step is negative so -step is positive");
        let step_abs_i64 = i64::try_from(step_abs).expect("step magnitude fits in i64");
        let mut i = i64::try_from(start).expect("start index fits in i64");
        let stop_i64 = if stop > items.len() {
            -1
        } else {
            i64::try_from(stop).expect("stop bounded by items.len() fits in i64")
        };

        while let Ok(i_usize) = usize::try_from(i) {
            if i_usize >= items.len() || i <= stop_i64 {
                break;
            }
            result.push(items[i_usize].clone_with_heap(heap));
            i -= step_abs_i64;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;

    use super::*;
    use crate::{intern::InternerBuilder, resource::NoLimitTracker, types::LongInt};

    /// Creates a minimal Interns for testing.
    fn create_test_interns() -> crate::intern::Interns {
        let interner = InternerBuilder::new("");
        crate::intern::Interns::new(interner, vec![], vec![])
    }

    /// Creates a heap with a list and a LongInt index, bypassing into_value() demotion.
    ///
    /// This allows testing the defensive code path where a LongInt contains an i64-fitting value.
    fn create_heap_with_list_and_longint(
        list_items: Vec<Value>,
        index_value: BigInt,
    ) -> (Heap<NoLimitTracker>, HeapId, HeapId) {
        let mut heap = Heap::new(16, NoLimitTracker);
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
        let interns = create_test_interns();

        // Use heap.with_entry_mut to avoid double mutable borrow
        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = heap.with_entry_mut(list_id, |heap, data| data.py_setitem(key, new_value, heap, &interns));

        assert!(result.is_ok());

        // Verify the list was updated by checking it matches expected Int value
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.as_slice()[1], Value::Int(99)));

        // Clean up
        Value::Ref(list_id).drop_with_heap(&mut heap);
    }

    /// Tests py_setitem with a negative LongInt index that fits in i64.
    #[test]
    fn py_setitem_longint_negative_fits_in_i64() {
        let (mut heap, list_id, index_id) = create_heap_with_list_and_longint(
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
            BigInt::from(-1), // Last element
        );
        let interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = heap.with_entry_mut(list_id, |heap, data| data.py_setitem(key, new_value, heap, &interns));

        assert!(result.is_ok());

        // Verify the last element was updated
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.as_slice()[2], Value::Int(99)));

        Value::Ref(list_id).drop_with_heap(&mut heap);
    }

    /// Tests py_setitem with i64::MAX as a LongInt index.
    #[test]
    fn py_setitem_longint_at_i64_max() {
        let (mut heap, list_id, index_id) =
            create_heap_with_list_and_longint(vec![Value::Int(10)], BigInt::from(i64::MAX));
        let interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        // This should fail with IndexError because i64::MAX is out of bounds for a 1-element list
        let result = heap.with_entry_mut(list_id, |heap, data| data.py_setitem(key, new_value, heap, &interns));

        assert!(result.is_err());

        Value::Ref(list_id).drop_with_heap(&mut heap);
    }
}
