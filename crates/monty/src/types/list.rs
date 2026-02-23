use std::fmt::Write;

use ahash::AHashSet;
use itertools::Itertools;
use smallvec::SmallVec;

use super::{AttrCallResult, MontyIter, PyTrait};
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard, HeapId},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    sorting::{apply_permutation, sort_indices},
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
                let items = MontyIter::new(v, heap, interns)?.collect(heap, interns)?;
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

        let items = get_slice_items(&self.items, start, stop, step, heap)?;
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
        defer_drop!(key, heap);
        defer_drop_mut!(value, heap);

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt.
        // Note: The LongInt-to-i64 conversion is defensive code. In normal execution,
        // heap-allocated LongInt values always exceed i64 range because into_value()
        // demotes i64-fitting values to Value::Int. However, this could be reached via
        // deserialization of crafted snapshot data.
        let index = match key {
            Value::Int(i) => *i,
            Value::Bool(b) => i64::from(*b),
            Value::Ref(heap_id) => {
                if let HeapData::LongInt(li) = heap.get(*heap_id) {
                    if let Some(i) = li.to_i64() {
                        i
                    } else {
                        return Err(ExcType::index_error_int_too_large());
                    }
                } else {
                    let key_type = key.py_type(heap);
                    return Err(ExcType::type_error_list_assignment_indices(key_type));
                }
            }
            _ => {
                let key_type = key.py_type(heap);
                return Err(ExcType::type_error_list_assignment_indices(key_type));
            }
        };

        // Normalize negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.items.len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_assignment_index_error());
        }

        let idx = usize::try_from(normalized_index).expect("index validated non-negative");

        // Update contains_refs if storing a Ref (must check before swap,
        // since after swap `value` holds the old item)
        if matches!(*value, Value::Ref(_)) {
            self.contains_refs = true;
            heap.mark_potential_cycle();
        }

        // Replace value (old one dropped by defer_drop_mut guard)
        std::mem::swap(&mut self.items[idx], value);

        Ok(())
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        if self.items.len() != other.items.len() {
            return Ok(false);
        }
        let token = heap.incr_recursion_depth()?;
        defer_drop!(token, heap);

        for (i1, i2) in self.items.iter().zip(&other.items) {
            heap.check_time()?;
            if !i1.py_eq(i2, heap, interns)? {
                return Ok(false);
            }
        }
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
        interns: &Interns,
    ) -> std::fmt::Result {
        repr_sequence_fmt('[', ']', &self.items, f, heap, heap_ids, interns)
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
        let args_guard = HeapGuard::new(args, heap);
        let Some(method) = attr.static_string() else {
            return Err(ExcType::attribute_error(Type::List, attr.as_str(interns)));
        };

        let (args, heap) = args_guard.into_parts();
        call_list_method(self, method, args, heap, interns)
    }

    /// Intercepts `sort` to call `do_list_sort` (which needs `PrintWriter` for key functions),
    /// and delegates all other methods to `py_call_attr`.
    fn py_call_attr_raw(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<AttrCallResult> {
        if attr.static_string() == Some(StaticStrings::Sort) {
            do_list_sort(self, args, vm)?;
            return Ok(AttrCallResult::Value(Value::None));
        }
        self.py_call_attr(vm.heap, attr, args, vm.interns)
            .map(AttrCallResult::Value)
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
        // Note: list.sort is handled by py_call_attr_raw which intercepts it
        _ => {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::List, method.into()))
        }
    }
}

/// Implements Python's `list.insert(index, item)` method.
fn list_insert(list: &mut List, args: ArgValues, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let (index_obj, item) = args.get_two_args("insert", heap)?;
    defer_drop!(index_obj, heap);
    let mut item_guard = HeapGuard::new(item, heap);
    let heap = item_guard.heap();
    // Python's insert() handles negative indices by adding len
    // If still negative after adding len, clamps to 0
    // If >= len, appends to end
    let index_i64 = index_obj.as_int(heap)?;
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
    let (item, heap) = item_guard.into_parts();
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
    defer_drop!(value, heap);

    // Find the first matching element
    let mut found_idx = None;
    for (i, item) in list.items.iter().enumerate() {
        heap.check_time()?;
        if value.py_eq(item, heap, interns)? {
            found_idx = Some(i);
            break;
        }
    }

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
    let items: SmallVec<[_; 2]> = MontyIter::new(iterable, heap, interns)?.collect(heap, interns)?;

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
    for (i, item) in list.items[start..end].iter().enumerate() {
        heap.check_time()?;
        if value.py_eq(item, heap, interns)? {
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
    defer_drop!(value, heap);

    let mut count: usize = 0;
    for item in &list.items {
        heap.check_time()?;
        if value.py_eq(item, heap, interns)? {
            count += 1;
        }
    }

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
fn do_list_sort(list: &mut List, args: ArgValues, vm: &mut VM<impl ResourceTracker>) -> Result<(), RunError> {
    // Parse keyword-only arguments: key and reverse
    let (key_arg, reverse_arg) = args.extract_two_kwargs_only("list.sort", "key", "reverse", vm.heap, vm.interns)?;

    // Convert reverse to bool (default false)
    let reverse = if let Some(v) = reverse_arg {
        let result = v.py_bool(vm.heap, vm.interns);
        v.drop_with_heap(vm.heap);
        result
    } else {
        false
    };

    // Handle key function (None means no key function)
    let key_fn = match key_arg {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with_heap(vm.heap);
            None
        }
        other => other,
    };
    defer_drop!(key_fn, vm);

    // Step 1: Borrow from the list for in-place sorting
    let items = list.as_vec_mut();

    // 2. Compute key values if a key function was provided, otherwise we'll sort by the items themselves
    let mut keys_guard;
    let (compare_values, vm) = if let Some(f) = key_fn {
        let keys: Vec<Value> = Vec::with_capacity(items.len());
        // Use a HeapGuard to ensure that if key function evaluation fails partway through,
        // we clean up any keys that were successfully computed
        keys_guard = HeapGuard::new(keys, vm);
        let (keys, vm) = keys_guard.as_parts_mut();
        items
            .iter()
            .map(|item| {
                let item = item.clone_with_heap(vm.heap);
                vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))
            })
            .process_results(|keys_iter| keys.extend(keys_iter))?;
        keys_guard.as_parts()
    } else {
        (&*items, vm)
    };

    // 3. Sort indices by comparing key values (or items themselves if no key)
    let len = compare_values.len();
    let mut indices: Vec<usize> = (0..len).collect();

    sort_indices(&mut indices, compare_values, reverse, vm.heap, vm.interns)?;

    // 4. Rearrange items in-place according to the sorted permutation
    apply_permutation(items, &mut indices);
    Ok(())
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
/// * `interns` - The interned strings table for looking up string/bytes literals
pub(crate) fn repr_sequence_fmt(
    start: char,
    end: char,
    items: &[Value],
    f: &mut impl Write,
    heap: &Heap<impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
    interns: &Interns,
) -> std::fmt::Result {
    // Check depth limit before recursing
    let Some(token) = heap.incr_recursion_depth_for_repr() else {
        return f.write_str("...");
    };
    crate::defer_drop_immutable_heap!(token, heap);

    f.write_char(start)?;
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        first.py_repr_fmt(f, heap, heap_ids, interns)?;
        for item in iter {
            if heap.check_time().is_err() {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
            item.py_repr_fmt(f, heap, heap_ids, interns)?;
        }
    }
    f.write_char(end)?;

    Ok(())
}

/// Helper to extract items from a slice for list/tuple slicing.
///
/// Handles both positive and negative step values. For negative step,
/// iterates backward from start down to (but not including) stop.
///
/// Returns a new Vec of cloned values with proper refcount increments.
/// Checks the time limit on each iteration to enforce timeouts during slicing.
///
/// Note: step must be non-zero (callers should validate this via `slice.indices()`).
pub(crate) fn get_slice_items(
    items: &[Value],
    start: usize,
    stop: usize,
    step: i64,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<Vec<Value>> {
    let mut result = Vec::new();

    // try_from succeeds for non-negative step; step==0 rejected upstream by slice.indices()
    if let Ok(step_usize) = usize::try_from(step) {
        // Positive step: iterate forward
        let mut i = start;
        while i < stop && i < items.len() {
            heap.check_time()?;
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
            heap.check_time()?;
            result.push(items[i_usize].clone_with_heap(heap));
            i -= step_abs_i64;
        }
    }

    Ok(result)
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
