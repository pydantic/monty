use std::{fmt::Write, mem};

use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    types::{
        Dict, FrozenSet, LazyHeapSet, PyTrait, Set, Type, allocate_tuple,
        dict::{DictItemIterator, DictKeyIterator, DictValueIterator},
        iter::checked_preallocation_hint,
    },
    value::{EitherStr, Value},
};

/// Shared accessors for heap-backed dictionary view objects.
///
/// All dictionary views are thin live references to an underlying `dict`. They do
/// not snapshot keys, items, or values; instead every observable operation reads
/// through to the current dict state. Keeping that behavior centralized avoids
/// subtle divergence between keys/items/values views.
pub(crate) trait DictView {
    /// Returns the heap id of the underlying dictionary this view keeps alive.
    fn dict_id(&self) -> HeapId;

    /// Returns the live dictionary backing this view.
    fn dict<'a>(&self, heap: &'a Heap) -> &'a Dict {
        let HeapData::Dict(dict) = heap.get(self.dict_id()) else {
            panic!("dict view must always reference a dict");
        };
        dict
    }
}

/// Live view returned by `dict.keys()`.
///
/// `dict_keys` is set-like in CPython, so this view supports the shared live-view
/// behavior plus equality against other keys views and ordinary set-like values.
/// The remaining set algebra operations are added incrementally in the VM layer.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct DictKeysView {
    dict_id: HeapId,
}

impl DictKeysView {
    /// Creates a new keys view over an existing dictionary heap entry.
    #[must_use]
    pub fn new(dict_id: HeapId) -> Self {
        Self { dict_id }
    }

    /// Returns the underlying dictionary heap id.
    #[must_use]
    pub fn dict_id(self) -> HeapId {
        self.dict_id
    }
}

impl<'h> HeapRead<'h, DictKeysView> {
    fn dict(&self, vm: &mut VM<'h>) -> HeapRead<'h, Dict> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.get(vm.heap).dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };
        dict
    }

    /// Compares this keys view to a mutable set using set membership semantics.
    pub(crate) fn eq_set(&self, other: &HeapRead<'h, Set>, vm: &mut VM<'h>) -> RunResult<bool> {
        dict_keys_eq_set_like(
            &self.dict(vm),
            other.get(vm.heap).len(),
            |key, vm| other.contains(key, vm),
            vm,
        )
    }

    /// Compares this keys view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset(&self, other: &HeapRead<'h, FrozenSet>, vm: &mut VM<'h>) -> RunResult<bool> {
        dict_keys_eq_set_like(
            &self.dict(vm),
            other.get(vm.heap).len(),
            |key, vm| other.contains(key, vm),
            vm,
        )
    }

    /// Materializes the view's current live keys into a plain `set`.
    ///
    /// Dict-view operators always produce ordinary `set` results in CPython,
    /// so the VM uses this helper as the left-hand-side snapshot for `& | ^ -`
    /// and for `isdisjoint(...)`.
    pub(crate) fn to_set(&self, vm: &mut VM<'h>) -> RunResult<Set> {
        let dict = self.dict(vm);
        let capacity = Set::preallocation_capacity(dict.get(vm.heap).len(), vm.heap.tracker())?;
        let iter = dict.iter(vm)?;
        defer_drop_mut!(iter, vm);
        let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
        let (result, vm) = result_guard.as_parts_mut();
        while let Some((key, value)) = iter.next_owned(vm)? {
            value.drop_with(vm);
            result.add(key, vm)?;
        }
        Ok(result_guard.into_inner())
    }

    /// Intersects live keys with an arbitrary iterable without materializing the view.
    fn intersection(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let other = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(other, vm);
        let capacity = Set::preallocation_capacity(other.len(), vm.heap.tracker())?;
        let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
        let (result, vm) = result_guard.as_parts_mut();
        let dict = self.dict(vm);
        for key in other.iter() {
            if dict.contains_key(key, vm)? {
                result.add(key.clone_with_heap(vm), vm)?;
            }
        }
        let (result, vm) = result_guard.into_parts();
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Set(result)))))
    }

    /// Applies a reflected set-like operation after validating the left iterable.
    fn reflected_binary_op(
        &self,
        other: &Value,
        vm: &mut VM<'h>,
        operation: impl FnOnce(&Set, &Set, &mut VM<'_>) -> RunResult<Set>,
    ) -> RunResult<Option<Value>> {
        let lhs = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(lhs, vm);
        let rhs = self.to_set(vm)?;
        defer_drop!(rhs, vm);
        let result = operation(lhs, rhs, vm)?;
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Set(result)))))
    }

    /// Implements `dict_keys.isdisjoint(iterable)` with CPython's iterable semantics.
    pub(crate) fn isdisjoint_from_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let self_set = self.to_set(vm)?;
        defer_drop!(self_set, vm);
        let other_set = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(other_set, vm);
        sets_are_disjoint(self_set, other_set, vm)
    }
}

impl DictView for DictKeysView {
    fn dict_id(&self) -> HeapId {
        self.dict_id
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, DictKeysView> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// Delegates to the backing dict's key lookup.
    fn py_contains_impl(&self, _self_id: HeapId, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let dict_id = self.get(vm.heap).dict_id();
        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
            panic!("dict_keys view must reference a dict");
        };
        dict.contains_key(item, vm).map(Some)
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DictKeys
    }

    fn py_iter(&self, _: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let dict_id = self.get(vm.heap).dict_id();
        Ok(DictKeyIterator::allocate(
            dict_id,
            self.get(vm.heap).dict(vm.heap).len(),
            vm,
        ))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::DictKeysView(other)) => {
                if self.get(vm.heap).dict_id == other.get(vm.heap).dict_id {
                    return Ok(Some(true));
                }
                let left = self.dict(vm);
                let right = other.dict(vm);
                dict_keys_eq_set_like(
                    &left,
                    right.get(vm.heap).len(),
                    |key, vm| right.contains_key(key, vm),
                    vm,
                )
                .map(Some)
            }
            Some(HeapReadOutput::Set(other)) => Ok(Some(self.eq_set(&other, vm)?)),
            Some(HeapReadOutput::FrozenSet(other)) => Ok(Some(self.eq_frozenset(&other, vm)?)),
            _ => Ok(None),
        }
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_sub)
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        self.intersection(other, vm)
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_or)
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_xor)
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_sub)
    }

    fn py_rand_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.intersection(other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_or)
    }

    fn py_rxor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_xor)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("dict_keys([")?;
        write_dict_keys_contents(f, &self.dict(vm), vm, heap_ids)?;
        Ok(f.write_str("])")?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        match attr.static_string() {
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("dict_keys.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(CallResult::Value(Value::Bool(self.isdisjoint_from_value(other, vm)?)))
            }
            _ => Err(ExcType::attribute_error(Type::DictKeys, attr.as_str(vm.interns))),
        }
    }
}

impl HeapItem for DictKeysView {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.dict_id);
    }
}

/// Live view returned by `dict.items()`.
///
/// The view stays linked to the original dictionary so iteration, `len()`, and
/// repr all reflect subsequent dictionary mutations. Like CPython, equality is
/// set-like: items views compare by their live `(key, value)` pairs.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct DictItemsView {
    dict_id: HeapId,
}

impl DictItemsView {
    /// Creates a new items view over an existing dictionary heap entry.
    #[must_use]
    pub fn new(dict_id: HeapId) -> Self {
        Self { dict_id }
    }

    /// Returns the underlying dictionary heap id.
    #[must_use]
    pub fn dict_id(self) -> HeapId {
        self.dict_id
    }
}

impl<'h> HeapRead<'h, DictItemsView> {
    fn dict(&self, vm: &mut VM<'h>) -> HeapRead<'h, Dict> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.get(vm.heap).dict_id) else {
            panic!("dict_items view must always reference a dict");
        };
        dict
    }

    /// Compares this items view to a mutable set using set membership semantics.
    pub(crate) fn eq_set(&self, other: &HeapRead<'h, Set>, vm: &mut VM<'h>) -> RunResult<bool> {
        dict_items_eq_set_like(
            &self.dict(vm),
            other.get(vm.heap).len(),
            |item, vm| other.contains(item, vm),
            vm,
        )
    }

    /// Compares this items view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset(&self, other: &HeapRead<'h, FrozenSet>, vm: &mut VM<'h>) -> RunResult<bool> {
        dict_items_eq_set_like(
            &self.dict(vm),
            other.get(vm.heap).len(),
            |item, vm| other.contains(item, vm),
            vm,
        )
    }

    /// Materializes the view's current live `(key, value)` pairs into a plain `set`.
    ///
    /// Each item is allocated as a 2-tuple so later set-like operators and
    /// membership checks observe standard Python tuple semantics.
    pub(crate) fn to_set(&self, vm: &mut VM<'h>) -> RunResult<Set> {
        let dict = self.dict(vm);
        let capacity = Set::preallocation_capacity(dict.get(vm.heap).len(), vm.heap.tracker())?;
        let iter = dict.iter(vm)?;
        defer_drop_mut!(iter, vm);
        let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
        let (result, vm) = result_guard.as_parts_mut();
        while let Some((key, value)) = iter.next_owned(vm)? {
            let item = allocate_tuple(smallvec![key, value], vm.heap);
            result.add(item, vm)?;
        }
        Ok(result_guard.into_inner())
    }

    /// Intersects live items with an arbitrary iterable.
    fn intersection(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let other = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(other, vm);
        let result = if other.is_empty() {
            Set::new()
        } else if self.items_are_hashable(vm)? {
            let items = self.to_set(vm)?;
            defer_drop!(items, vm);
            apply_dict_view_and(items, other, vm)?
        } else {
            self.intersect_unhashable_items(other, vm)?
        };
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Set(result)))))
    }

    /// Returns whether every live value can participate in an item tuple hash.
    fn items_are_hashable(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        let dict = self.dict(vm);
        let iter = dict.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((_key, value)) = iter.next(vm)? {
            if value.py_hash(vm)?.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Intersects without hashing item tuples whose values are unhashable.
    fn intersect_unhashable_items(&self, other: &Set, vm: &mut VM<'h>) -> RunResult<Set> {
        let capacity = Set::preallocation_capacity(other.len(), vm.heap.tracker())?;
        let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
        let (result, vm) = result_guard.as_parts_mut();
        for candidate in other.iter() {
            if self.contains_item(candidate, vm)? {
                result.add(candidate.clone_with_heap(vm), vm)?;
            }
        }
        Ok(result_guard.into_inner())
    }

    /// Tests item membership by equality, avoiding a hash of the view's values.
    fn contains_item(&self, candidate: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let dict = self.dict(vm);
        let iter = dict.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((key, value)) = iter.next(vm)? {
            let item = allocate_tuple(smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)], vm.heap);
            defer_drop!(item, vm);
            if candidate.py_eq(item, vm)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Applies a reflected set-like operation after validating the left iterable.
    fn reflected_binary_op(
        &self,
        other: &Value,
        vm: &mut VM<'h>,
        operation: impl FnOnce(&Set, &Set, &mut VM<'_>) -> RunResult<Set>,
    ) -> RunResult<Option<Value>> {
        let lhs = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(lhs, vm);
        let rhs = self.to_set(vm)?;
        defer_drop!(rhs, vm);
        let result = operation(lhs, rhs, vm)?;
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Set(result)))))
    }

    /// Implements `dict_items.isdisjoint(iterable)` with CPython's iterable semantics.
    pub(crate) fn isdisjoint_from_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let self_set = self.to_set(vm)?;
        defer_drop!(self_set, vm);
        let other_set = collect_iterable_to_set(other.clone_with_heap(vm), vm)?;
        defer_drop!(other_set, vm);
        sets_are_disjoint(self_set, other_set, vm)
    }
}

impl DictView for DictItemsView {
    fn dict_id(&self) -> HeapId {
        self.dict_id
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, DictItemsView> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// Membership takes a `(key, value)` probe: look the key up, then compare
    /// the stored value. A non-pair probe can never be a member.
    fn py_contains_impl(&self, _self_id: HeapId, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let dict_id = self.get(vm.heap).dict_id();
        let Some((key, value)) = cloned_items_view_candidate(item, vm) else {
            return Ok(Some(false));
        };
        defer_drop!(key, vm);
        defer_drop!(value, vm);
        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
            panic!("dict_items view must reference a dict");
        };
        match dict.dict_get(key, vm) {
            Ok(Some(existing_value)) => {
                // Stored value on the left, as CPython's `dictitems_contains` does.
                let result = existing_value.py_eq(value, vm);
                existing_value.drop_with(vm);
                result.map(Some)
            }
            Ok(None) => Ok(Some(false)),
            Err(e) => Err(e),
        }
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DictItems
    }

    fn py_iter(&self, _: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let dict_id = self.get(vm.heap).dict_id();
        Ok(DictItemIterator::allocate(
            dict_id,
            self.get(vm.heap).dict(vm.heap).len(),
            vm,
        ))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::DictItemsView(other)) => {
                if self.get(vm.heap).dict_id == other.get(vm.heap).dict_id {
                    return Ok(Some(true));
                }
                let left = self.dict(vm);
                let right = other.dict(vm);
                Ok(Some(left.eq_dict(&right, vm)?))
            }
            Some(HeapReadOutput::Set(other)) => Ok(Some(self.eq_set(&other, vm)?)),
            Some(HeapReadOutput::FrozenSet(other)) => Ok(Some(self.eq_frozenset(&other, vm)?)),
            _ => Ok(None),
        }
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_sub)
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        self.intersection(other, vm)
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_or)
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        dict_view_binary_op_value(self.to_set(vm)?, other, vm, apply_dict_view_xor)
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_sub)
    }

    fn py_rand_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.intersection(other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_or)
    }

    fn py_rxor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.reflected_binary_op(other, vm, apply_dict_view_xor)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("dict_items([")?;
        write_dict_items_contents(f, &self.dict(vm), vm, heap_ids)?;
        Ok(f.write_str("])")?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        match attr.static_string() {
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("dict_items.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(CallResult::Value(Value::Bool(self.isdisjoint_from_value(other, vm)?)))
            }
            _ => Err(ExcType::attribute_error(Type::DictItems, attr.as_str(vm.interns))),
        }
    }
}

impl HeapItem for DictItemsView {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.dict_id);
    }
}

/// Live view returned by `dict.values()`.
///
/// Unlike keys/items views, `dict_values` is intentionally not set-like in
/// CPython. Milestone one only needs it to be a real view object with the same
/// live iteration, repr, and membership behavior users expect from Python.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct DictValuesView {
    dict_id: HeapId,
}

impl DictValuesView {
    /// Creates a new values view over an existing dictionary heap entry.
    #[must_use]
    pub fn new(dict_id: HeapId) -> Self {
        Self { dict_id }
    }

    /// Returns the underlying dictionary heap id.
    #[must_use]
    pub fn dict_id(self) -> HeapId {
        self.dict_id
    }
}

impl DictView for DictValuesView {
    fn dict_id(&self) -> HeapId {
        self.dict_id
    }
}

impl<'h> HeapRead<'h, DictValuesView> {
    fn dict(&self, vm: &mut VM<'h>) -> HeapRead<'h, Dict> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.get(vm.heap).dict_id) else {
            panic!("dict_values view must always reference a dict");
        };
        dict
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, DictValuesView> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// Values are not indexed, so this is a linear scan of the backing dict.
    fn py_contains_impl(&self, _self_id: HeapId, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let dict_id = self.get(vm.heap).dict_id();
        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
            panic!("dict_values view must reference a dict");
        };
        let iter = dict.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(value) = iter.next_value(vm)? {
            // Stored value on the left, matching CPython's iteration fallback.
            if value.py_eq(item, vm)? {
                return Ok(Some(true));
            }
        }
        Ok(Some(false))
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DictValues
    }

    fn py_iter(&self, _: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let dict_id = self.get(vm.heap).dict_id();
        Ok(DictValueIterator::allocate(
            dict_id,
            self.get(vm.heap).dict(vm.heap).len(),
            vm,
        ))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // `dict_values` views use identity equality (handled before the heap read).
        Ok(None)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("dict_values([")?;
        write_dict_values_contents(f, &self.dict(vm), vm, heap_ids)?;
        Ok(f.write_str("])")?)
    }
}

impl HeapItem for DictValuesView {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.dict_id);
    }
}

/// Compares a dict's live keys to another set-like container by membership.
fn dict_keys_eq_set_like<'h>(
    dict: &HeapRead<'h, Dict>,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'h>) -> RunResult<bool>,
    vm: &mut VM<'h>,
) -> RunResult<bool> {
    if dict.get(vm.heap).len() != other_len {
        return Ok(false);
    }

    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    let len = dict.get(vm.heap).len();
    for i in 0..len {
        vm.heap.check_time()?;
        let key = dict.get(vm.heap).key_at(i).unwrap().clone_with_heap(vm);
        defer_drop!(key, vm);
        if !contains(key, vm)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Compares a dict's live items to another set-like container by membership.
fn dict_items_eq_set_like<'h>(
    dict: &HeapRead<'h, Dict>,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'h>) -> RunResult<bool>,
    vm: &mut VM<'h>,
) -> RunResult<bool> {
    if dict.get(vm.heap).len() != other_len {
        return Ok(false);
    }

    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    let len = dict.get(vm.heap).len();
    for i in 0..len {
        vm.heap.check_time()?;
        let (key, value) = dict.get(vm.heap).item_at(i).unwrap();
        let item = allocate_tuple(smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)], vm.heap);
        defer_drop!(item, vm);
        if !contains(item, vm)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Writes the repr payload for a keys view without its outer wrapper.
fn write_dict_keys_contents<'h>(
    f: &mut impl Write,
    dict: &HeapRead<'h, Dict>,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    let iter = dict.iter(vm)?;
    defer_drop_mut!(iter, vm);
    let mut first = true;
    while let Some((key, _value)) = iter.next(vm)? {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        key.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Writes the repr payload for an items view without its outer wrapper.
fn write_dict_items_contents<'h>(
    f: &mut impl Write,
    dict: &HeapRead<'h, Dict>,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    let iter = dict.iter(vm)?;
    defer_drop_mut!(iter, vm);
    let mut first = true;
    while let Some((key, value)) = iter.next(vm)? {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        f.write_char('(')?;
        key.py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(", ")?;
        value.py_repr_fmt(f, vm, heap_ids)?;
        f.write_char(')')?;
    }
    Ok(())
}

/// Writes the repr payload for a values view without its outer wrapper.
fn write_dict_values_contents<'h>(
    f: &mut impl Write,
    dict: &HeapRead<'h, Dict>,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    let iter = dict.iter(vm)?;
    defer_drop_mut!(iter, vm);
    let mut first = true;
    while let Some((_key, value)) = iter.next(vm)? {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        value.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Applies a dict-view set-like binary operator and returns a plain `set`.
fn dict_view_binary_op_value(
    lhs_set: Set,
    rhs: &Value,
    vm: &mut VM<'_>,
    operation: impl FnOnce(&Set, &Set, &mut VM<'_>) -> RunResult<Set>,
) -> RunResult<Option<Value>> {
    defer_drop!(lhs_set, vm);
    let rhs_set = collect_iterable_to_set(rhs.clone_with_heap(vm), vm)?;
    defer_drop!(rhs_set, vm);

    let result = operation(lhs_set, rhs_set, vm)?;
    let result_id = vm.heap.allocate(HeapData::Set(result));
    Ok(Some(Value::Ref(result_id)))
}

/// Computes dictionary-view intersection.
fn apply_dict_view_and(lhs: &Set, rhs: &Set, vm: &mut VM<'_>) -> RunResult<Set> {
    let capacity = Set::preallocation_capacity(lhs.len().min(rhs.len()), vm.heap.tracker())?;
    let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
    let (result, vm) = result_guard.as_parts_mut();
    let (smaller, larger) = if lhs.len() <= rhs.len() { (lhs, rhs) } else { (rhs, lhs) };
    for value in smaller.iter() {
        if vm.heap.protect(larger).contains(value, vm)? {
            result.add(value.clone_with_heap(vm), vm)?;
        }
    }
    Ok(result_guard.into_inner())
}

/// Computes dictionary-view union.
fn apply_dict_view_or(lhs: &Set, rhs: &Set, vm: &mut VM<'_>) -> RunResult<Set> {
    let capacity = Set::preallocation_capacity(lhs.len().saturating_add(rhs.len()), vm.heap.tracker())?;
    let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
    let (result, vm) = result_guard.as_parts_mut();
    for value in lhs.iter().chain(rhs.iter()) {
        result.add(value.clone_with_heap(vm), vm)?;
    }
    Ok(result_guard.into_inner())
}

/// Computes dictionary-view symmetric difference.
fn apply_dict_view_xor(lhs: &Set, rhs: &Set, vm: &mut VM<'_>) -> RunResult<Set> {
    let capacity = Set::preallocation_capacity(lhs.len().saturating_add(rhs.len()), vm.heap.tracker())?;
    let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
    let (result, vm) = result_guard.as_parts_mut();
    for value in lhs.iter() {
        if !vm.heap.protect(rhs).contains(value, vm)? {
            result.add(value.clone_with_heap(vm), vm)?;
        }
    }
    for value in rhs.iter() {
        if !vm.heap.protect(lhs).contains(value, vm)? {
            result.add(value.clone_with_heap(vm), vm)?;
        }
    }
    Ok(result_guard.into_inner())
}

/// Computes dictionary-view difference.
fn apply_dict_view_sub(lhs: &Set, rhs: &Set, vm: &mut VM<'_>) -> RunResult<Set> {
    let capacity = Set::preallocation_capacity(lhs.len(), vm.heap.tracker())?;
    let mut result_guard = DropGuard::new(Set::with_capacity(capacity), vm);
    let (result, vm) = result_guard.as_parts_mut();
    for value in lhs.iter() {
        if !vm.heap.protect(rhs).contains(value, vm)? {
            result.add(value.clone_with_heap(vm), vm)?;
        }
    }
    Ok(result_guard.into_inner())
}

/// Collects an arbitrary iterable into a temporary `set`.
///
/// Dict-view operators accept any iterable on the right-hand side in CPython,
/// including one-shot iterator objects. Reusing the same collection path keeps
/// binary operators and `isdisjoint(...)` consistent with each other.
pub(crate) fn collect_iterable_to_set(value: Value, vm: &mut VM<'_>) -> Result<Set, RunError> {
    defer_drop!(value, vm);
    let iter = value.py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);
    let cap = checked_preallocation_hint(iter.iter_size_hint(vm), mem::size_of::<Value>() * 2, vm.heap.tracker())?;
    let mut set_guard = DropGuard::new(Set::with_capacity(cap), vm);
    let (set, vm) = set_guard.as_parts_mut();
    while let Some(item) = iter.py_next(vm)? {
        set.add(item, vm)?;
    }
    Ok(set_guard.into_inner())
}

/// Returns whether two temporary sets have no elements in common.
fn sets_are_disjoint(left: &Set, right: &Set, vm: &mut VM<'_>) -> RunResult<bool> {
    let (smaller, larger) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };

    for value in smaller.iter() {
        if vm.heap.protect(larger).contains(value, vm)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Extracts and clones the `(key, value)` probe accepted by `dict_items.__contains__`.
///
/// CPython treats only 2-tuples as valid probes for items-view membership. Monty
/// also accepts namedtuples of length two so tuple-like runtime values behave
/// sensibly even though namedtuples are not modeled as a true tuple subclass.
pub(crate) fn cloned_items_view_candidate(item: &Value, heap: &impl ContainsHeap) -> Option<(Value, Value)> {
    let Value::Ref(heap_id) = item else {
        return None;
    };

    match heap.heap().get(*heap_id) {
        HeapData::Tuple(tuple) => {
            let items = tuple.as_slice();
            if items.len() == 2 {
                Some((items[0].clone_with_heap(heap), items[1].clone_with_heap(heap)))
            } else {
                None
            }
        }
        HeapData::NamedTuple(namedtuple) => {
            let items = namedtuple.as_vec();
            if items.len() == 2 {
                Some((items[0].clone_with_heap(heap), items[1].clone_with_heap(heap)))
            } else {
                None
            }
        }
        _ => None,
    }
}
