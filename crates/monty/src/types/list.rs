use std::fmt::Write;

use ahash::AHashSet;
use itertools::Itertools;
use smallvec::SmallVec;

use super::{MontyIter, PyTrait};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
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

    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list", vm.heap)?;
        match value {
            None => {
                let heap_id = vm.heap.allocate(HeapData::List(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let items = MontyIter::new(v, vm)?.collect(vm)?;
                let heap_id = vm.heap.allocate(HeapData::List(Self::new(items)))?;
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl<'h> HeapRead<'h, List> {
    /// Appends an element to the end of the list.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    pub fn append(&mut self, item: Value, vm: &mut VM<'h, '_, impl ResourceTracker>) {
        // Track if we're adding a reference and mark potential cycle
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
            vm.heap.mark_potential_cycle();
        }
        // Ownership transfer - refcount was already handled by caller
        self.get_mut(vm.heap).items.push(item);
    }

    /// Clones the item at the given index with proper refcount management.
    ///
    /// Uses the short-lived borrow pattern: reads the value discriminant through
    /// a shared heap borrow, releases it, then increments refcount if needed
    /// through a mutable heap borrow.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Value {
        let ref_id = match &self.get(vm.heap).items[index] {
            Value::Ref(id) => Some(*id),
            _ => None,
        };
        if let Some(id) = ref_id {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        } else {
            self.get(vm.heap).items[index].clone_immediate()
        }
    }

    /// Clones all items from this list with proper refcount management.
    ///
    /// Uses the short-lived borrow pattern per element to avoid holding
    /// a heap borrow across refcount increments.
    fn clone_all_items(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Vec<Value> {
        let len = self.get(vm.heap).items.len();
        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            result.push(self.clone_item(i, vm));
        }
        result
    }

    /// Collects sliced items using the two-phase clone pattern.
    ///
    /// Unlike `get_slice_items` which takes `&[Value]` + `&mut Heap`, this method
    /// uses short-lived borrows through HeapRead to avoid holding the heap borrow
    /// across clone_with_heap calls.
    fn getitem_slice_items(
        &self,
        start: usize,
        stop: usize,
        step: i64,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Vec<Value>> {
        let items_len = self.get(vm.heap).len();
        let mut result = Vec::new();

        if let Ok(step_usize) = usize::try_from(step) {
            let mut i = start;
            while i < stop && i < items_len {
                vm.heap.check_time()?;
                result.push(self.clone_item(i, vm));
                i += step_usize;
            }
        } else {
            let step_abs = usize::try_from(-step).expect("step is negative so -step is positive");
            let step_abs_i64 = i64::try_from(step_abs).expect("step magnitude fits in i64");
            let mut i = i64::try_from(start).expect("start index fits in i64");
            let stop_i64 = if stop > items_len {
                -1
            } else {
                i64::try_from(stop).expect("stop bounded by items.len() fits in i64")
            };
            while i > stop_i64 {
                vm.heap.check_time()?;
                let idx = usize::try_from(i).expect("i is non-negative");
                if idx < items_len {
                    result.push(self.clone_item(idx, vm));
                }
                i -= step_abs_i64;
            }
        }

        Ok(result)
    }
}

impl<'h> HeapRead<'h, List> {
    /// `list.insert(index, item)` via HeapRead.
    fn hr_insert(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let (index_obj, item) = args.get_two_args("insert", vm.heap)?;
        defer_drop!(index_obj, vm);

        let index_i64 = index_obj.as_int(vm.heap)?;
        let len = self.get(vm.heap).items.len();
        let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
        let index = if index_i64 < 0 {
            usize::try_from(index_i64 + len_i64).unwrap_or(0)
        } else {
            usize::try_from(index_i64).unwrap_or(len)
        };

        // Inline the insert logic with short-lived borrows
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
            vm.heap.mark_potential_cycle();
        }
        let items = &mut self.get_mut(vm.heap).items;
        let insert_at = index.min(items.len());
        items.insert(insert_at, item);
        Ok(Value::None)
    }

    /// `list.pop([index])` via HeapRead.
    fn hr_pop(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let index_arg = args.get_zero_one_arg("list.pop", vm.heap)?;

        let index_i64 = if let Some(v) = index_arg {
            let result = v.as_int(vm.heap);
            v.drop_with_heap(vm);
            result?
        } else {
            -1
        };

        if self.get(vm.heap).items.is_empty() {
            return Err(ExcType::index_error_pop_empty_list());
        }

        let len = self.get(vm.heap).items.len();
        let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
        let normalized = if index_i64 < 0 { index_i64 + len_i64 } else { index_i64 };

        if normalized < 0 || normalized >= len_i64 {
            return Err(ExcType::index_error_pop_out_of_range());
        }

        let idx = usize::try_from(normalized).expect("index validated non-negative");
        Ok(self.get_mut(vm.heap).items.remove(idx))
    }

    /// `list.remove(value)` via HeapRead.
    fn hr_remove(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let value = args.get_one_arg("list.remove", vm.heap)?;
        defer_drop!(value, vm);

        let len = self.get(vm.heap).items.len();
        let mut found_idx = None;
        for i in 0..len {
            vm.heap.check_time()?;
            let item = self.clone_item(i, vm);
            let is_eq = value.py_eq(&item, vm)?;
            item.drop_with_heap(vm);
            if is_eq {
                found_idx = Some(i);
                break;
            }
        }

        match found_idx {
            Some(idx) => {
                let removed = self.get_mut(vm.heap).items.remove(idx);
                removed.drop_with_heap(vm);
                Ok(Value::None)
            }
            None => Err(ExcType::value_error_remove_not_in_list()),
        }
    }

    /// `list.clear()` via HeapRead.
    fn hr_clear(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) {
        let entries: Vec<Value> = self.get_mut(vm.heap).items.drain(..).collect();
        entries.drop_with_heap(vm);
    }

    /// `list.copy()` via HeapRead — returns a shallow copy.
    fn hr_copy(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let items = self.clone_all_items(vm);
        let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
        Ok(Value::Ref(heap_id))
    }

    /// `list.extend(iterable)` via HeapRead.
    ///
    /// Because data stays in the heap, `x.extend(x)` works correctly — the
    /// source list is accessible through the heap via `MontyIter`.
    fn hr_extend(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let iterable = args.get_one_arg("list.extend", vm.heap)?;
        let items: SmallVec<[_; 2]> = MontyIter::new(iterable, vm)?.collect(vm)?;

        for item in items {
            self.append(item, vm);
        }

        Ok(Value::None)
    }

    /// `list.index(value[, start[, end]])` via HeapRead.
    fn hr_index(&self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let pos_args = args.into_pos_only("list.index", vm.heap)?;
        defer_drop!(pos_args, vm);

        let len = self.get(vm.heap).items.len();
        let (value, start, end) = match pos_args.as_slice() {
            [] => return Err(ExcType::type_error_at_least("list.index", 1, 0)),
            [value] => (value, 0, len),
            [value, start_arg] => {
                let start = normalize_list_index(start_arg.as_int(vm.heap)?, len);
                (value, start, len)
            }
            [value, start_arg, end_arg] => {
                let start = normalize_list_index(start_arg.as_int(vm.heap)?, len);
                let end = normalize_list_index(end_arg.as_int(vm.heap)?, len).max(start);
                (value, start, end)
            }
            other => return Err(ExcType::type_error_at_most("list.index", 3, other.len())),
        };

        for i in start..end {
            vm.heap.check_time()?;
            let item = self.clone_item(i, vm);
            let is_eq = value.py_eq(&item, vm)?;
            item.drop_with_heap(vm);
            if is_eq {
                let idx = i64::try_from(i).expect("index exceeds i64::MAX");
                return Ok(Value::Int(idx));
            }
        }

        Err(ExcType::value_error_not_in_list())
    }

    /// `list.count(value)` via HeapRead.
    fn hr_count(&self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let value = args.get_one_arg("list.count", vm.heap)?;
        defer_drop!(value, vm);

        let len = self.get(vm.heap).items.len();
        let mut count: usize = 0;
        for i in 0..len {
            vm.heap.check_time()?;
            let item = self.clone_item(i, vm);
            let is_eq = value.py_eq(&item, vm)?;
            item.drop_with_heap(vm);
            if is_eq {
                count += 1;
            }
        }

        let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
        Ok(Value::Int(count_i64))
    }

    /// `list.sort(*, key=None, reverse=False)` via HeapRead.
    ///
    /// Takes items out of the list for sorting, then writes them back.
    /// This matches CPython's behavior where the list is temporarily empty
    /// during sorting (accessing `list` during a sort key function sees `[]`).
    fn hr_sort(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<(), RunError> {
        let (key_arg, reverse_arg) =
            args.extract_keyword_only_pair("list.sort", "key", "reverse", vm.heap, vm.interns)?;

        let reverse = if let Some(v) = reverse_arg {
            let result = v.py_bool(vm);
            v.drop_with_heap(vm);
            result
        } else {
            false
        };

        let key_fn = match key_arg {
            Some(v) if matches!(v, Value::None) => {
                v.drop_with_heap(vm);
                None
            }
            other => other,
        };
        defer_drop!(key_fn, vm);

        // Take items out for sorting (short-lived borrow)
        let mut items: Vec<Value> = self.get_mut(vm.heap).items.drain(..).collect();

        let mut keys_guard;
        let (compare_values, vm) = if let Some(f) = key_fn {
            let keys: Vec<Value> = Vec::with_capacity(items.len());
            keys_guard = HeapGuard::new(keys, vm);
            let (keys, vm) = keys_guard.as_parts_mut();
            items
                .iter()
                .map(|item| {
                    let item = item.clone_with_heap(vm);
                    vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))
                })
                .process_results(|keys_iter| keys.extend(keys_iter))?;
            let (keys, vm) = keys_guard.as_parts();
            (keys.as_slice(), vm)
        } else {
            (items.as_slice(), vm)
        };

        let len = compare_values.len();
        let mut indices: Vec<usize> = (0..len).collect();
        sort_indices(&mut indices, compare_values, reverse, vm)?;
        apply_permutation(&mut items, &mut indices);

        // Write sorted items back
        self.get_mut(vm.heap).items = items;
        Ok(())
    }
}

impl From<List> for Vec<Value> {
    fn from(list: List) -> Self {
        list.items
    }
}

/// `PyTrait` implementation for `HeapRead<'h, List>`.
///
/// This provides the standard Python operations for list values accessed through
/// heap read handles. Mutable methods (setitem, iadd, call_attr) operate through
/// the HeapRead's `get_mut` to modify the list in-place on the heap.
impl<'h> PyTrait<'h> for HeapRead<'h, List> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::List
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).items.len())
    }

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        repr_sequence_fmt('[', ']', &self.get(vm.heap).items, f, vm, heap_ids)
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a_len = self.get(vm.heap).items.len();
        if a_len != other.get(vm.heap).items.len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        for i in 0..a_len {
            vm.heap.check_time()?;
            let a_val = self.clone_item(i, vm);
            let b_val = other.clone_item(i, vm);
            let result = a_val.py_eq(&b_val, vm);
            a_val.drop_with_heap(vm);
            b_val.drop_with_heap(vm);
            if !result? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn py_add(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let mut items = self.clone_all_items(vm);
        items.extend(other.clone_all_items(vm));
        let id = vm.heap.allocate(HeapData::List(List::new(items)))?;
        Ok(Some(Value::Ref(id)))
    }

    fn py_iadd(
        &mut self,
        other: &Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        self_id: Option<HeapId>,
    ) -> Result<bool, ResourceError> {
        let Value::Ref(other_id) = other else {
            return Ok(false);
        };

        if Some(*other_id) == self_id {
            // Self-extend: clone our own items with proper refcounting
            let items = self.clone_all_items(vm);
            if self.get(vm.heap).contains_refs {
                vm.heap.mark_potential_cycle();
            }
            self.get_mut(vm.heap).items.extend(items);
        } else {
            // Read source list via HeapRead, clone items into a temporary Vec
            let source = vm.heap.read(*other_id);
            let HeapReadOutput::List(source_list) = source else {
                return Ok(false);
            };
            let source_items = source_list.clone_all_items(vm);
            // Check if new items contain refs
            let has_new_refs = source_items.iter().any(|v| matches!(v, Value::Ref(_)));
            self.get_mut(vm.heap).items.extend(source_items);
            if self.get(vm.heap).contains_refs || has_new_refs {
                if has_new_refs {
                    self.get_mut(vm.heap).contains_refs = true;
                }
                vm.heap.mark_potential_cycle();
            }
        }

        Ok(true)
    }

    /// Subscript access via HeapRead. Handles both integer indices and slices.
    ///
    /// For integer indices: normalizes negative indices, bounds checks, and returns
    /// a cloned item. For slices: computes indices and returns a new list with
    /// cloned elements.
    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let slice = slice.clone();
            let len = self.get(vm.heap).len();
            let (start, stop, step) = slice
                .indices(len)
                .map_err(|()| ExcType::value_error_slice_step_zero())?;
            // Collect cloned items using short-lived borrows to avoid
            // holding a borrow across allocation
            let items = self.getitem_slice_items(start, stop, step, vm)?;
            let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
            return Ok(Value::Ref(heap_id));
        }

        let index = key.as_index(vm.heap, Type::List)?;
        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized = if index < 0 { index + len } else { index };

        if normalized < 0 || normalized >= len {
            return Err(ExcType::list_index_error());
        }

        let idx = usize::try_from(normalized).expect("list index validated non-negative");
        Ok(self.clone_item(idx, vm))
    }

    /// Subscript assignment via HeapRead. Takes ownership of key and value.
    ///
    /// Normalizes negative indices, bounds checks, swaps in the new value,
    /// and drops the old value. Updates `contains_refs` flag for GC tracking.
    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop_mut!(value, vm);

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
                    let key_type = key.py_type(vm);
                    return Err(ExcType::type_error_list_assignment_indices(key_type));
                }
            }
            _ => {
                let key_type = key.py_type(vm);
                return Err(ExcType::type_error_list_assignment_indices(key_type));
            }
        };

        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized = if index < 0 { index + len } else { index };

        if normalized < 0 || normalized >= len {
            return Err(ExcType::list_assignment_index_error());
        }

        let idx = usize::try_from(normalized).expect("index validated non-negative");

        if matches!(*value, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
            vm.heap.mark_potential_cycle();
        }

        // Replace value (old one dropped by defer_drop_mut guard)
        std::mem::swap(&mut self.get_mut(vm.heap).items[idx], value);

        Ok(())
    }

    /// Dispatches a method call on a list accessed through `HeapRead`.
    ///
    /// Because the list data stays in the heap, self-referential operations like
    /// `x.extend(x)` work correctly — the source list remains accessible through
    /// the heap during iteration.
    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        if attr.static_string() == Some(StaticStrings::Sort) {
            self.hr_sort(args, vm)?;
            return Ok(CallResult::Value(Value::None));
        }

        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(Type::List, attr.as_str(vm.interns)));
        };

        let value = match method {
            StaticStrings::Append => {
                let item = args.get_one_arg("list.append", vm.heap)?;
                self.append(item, vm);
                Ok(Value::None)
            }
            StaticStrings::Insert => self.hr_insert(args, vm),
            StaticStrings::Pop => self.hr_pop(args, vm),
            StaticStrings::Remove => self.hr_remove(args, vm),
            StaticStrings::Clear => {
                args.check_zero_args("list.clear", vm.heap)?;
                self.hr_clear(vm);
                Ok(Value::None)
            }
            StaticStrings::Copy => {
                args.check_zero_args("list.copy", vm.heap)?;
                self.hr_copy(vm)
            }
            StaticStrings::Extend => self.hr_extend(args, vm),
            StaticStrings::Index => self.hr_index(args, vm),
            StaticStrings::Count => self.hr_count(args, vm),
            StaticStrings::Reverse => {
                args.check_zero_args("list.reverse", vm.heap)?;
                self.get_mut(vm.heap).items.reverse();
                Ok(Value::None)
            }
            _ => {
                args.drop_with_heap(vm);
                return Err(ExcType::attribute_error(Type::List, method.into()));
            }
        };
        value.map(CallResult::Value)
    }
}

impl HeapItem for List {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.items.len() * std::mem::size_of::<Value>()
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
/// * `vm` - The VM for resolving value references and looking up interned strings
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
pub(crate) fn repr_sequence_fmt(
    start: char,
    end: char,
    items: &[Value],
    f: &mut impl Write,
    vm: &VM<'_, '_, impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
) -> std::fmt::Result {
    // Check depth limit before recursing
    let heap = &*vm.heap;
    let Some(token) = heap.incr_recursion_depth_for_repr() else {
        return f.write_str("...");
    };
    crate::defer_drop_immutable_heap!(token, heap);

    f.write_char(start)?;
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        first.py_repr_fmt(f, vm, heap_ids)?;
        for item in iter {
            if heap.check_time().is_err() {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
            item.py_repr_fmt(f, vm, heap_ids)?;
        }
    }
    f.write_char(end)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;

    use super::*;
    use crate::{
        PrintWriter,
        heap::HeapReader,
        intern::{InternerBuilder, Interns},
        resource::NoLimitTracker,
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
        let interns = create_test_interns();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, |heap| {
            let mut vm = VM::new(Vec::new(), heap, &interns, PrintWriter::Disabled);
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

        let result = HeapReader::with(&mut heap, |heap| {
            let mut vm = VM::new(Vec::new(), heap, &interns, PrintWriter::Disabled);
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
        let result = HeapReader::with(&mut heap, |heap| {
            let mut vm = VM::new(Vec::new(), heap, &interns, PrintWriter::Disabled);
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_err());

        Value::Ref(list_id).drop_with_heap(&mut heap);
    }
}
