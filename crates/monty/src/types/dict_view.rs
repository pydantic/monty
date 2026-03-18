use std::fmt::Write;

use ahash::AHashSet;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem},
    heap_data::HeapDataMut,
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, FrozenSet, MontyIter, PyTrait, Set, Type, allocate_tuple, iter::advance_on_heap},
    value::Value,
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
    fn dict<'a>(&self, heap: &'a Heap<impl ResourceTracker>) -> &'a Dict {
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

    /// Compares this keys view to another keys view using set semantics.
    ///
    /// Two keys views compare equal when they expose the same live key set,
    /// even if they are distinct view objects over distinct dictionaries.
    pub(crate) fn eq_view(self, other: Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.dict_id == other.dict_id {
            return Ok(true);
        }

        Heap::with_two(vm, self.dict_id, other.dict_id, |vm, left, right| {
            let (HeapData::Dict(left_dict), HeapData::Dict(right_dict)) = (left, right) else {
                panic!("dict_keys view must always reference dicts");
            };
            dict_keys_eq_dict(left_dict, right_dict, vm)
        })
    }

    /// Compares this keys view to a mutable set using set membership semantics.
    pub(crate) fn eq_set(self, other: &Set, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_keys view must always reference a dict");
            };
            dict_keys_eq_set_like(
                dict,
                other.len(),
                |key, vm| matches!(other.contains(key, vm), Ok(true)),
                vm,
            )
        })
    }

    /// Compares this keys view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset(
        self,
        other: &FrozenSet,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_keys view must always reference a dict");
            };
            dict_keys_eq_set_like(
                dict,
                other.len(),
                |key, vm| matches!(other.contains(key, vm), Ok(true)),
                vm,
            )
        })
    }

    /// Materializes the view's current live keys into a plain `set`.
    ///
    /// Dict-view operators always produce ordinary `set` results in CPython,
    /// so the VM uses this helper as the left-hand-side snapshot for `& | ^ -`
    /// and for `isdisjoint(...)`.
    pub(crate) fn to_set(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Set> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_keys view must always reference a dict");
            };

            let mut result = Set::with_capacity(dict.len());
            for (key, _) in dict.iter() {
                result.add(key.clone_with_heap(vm), vm)?;
            }
            Ok(result)
        })
    }

    /// Implements `dict_keys.isdisjoint(iterable)` with CPython's iterable semantics.
    pub(crate) fn isdisjoint_from_value(
        self,
        other: &Value,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<bool> {
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

impl PyTrait for DictKeysView {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::DictKeys
    }

    fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        other: &Self,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        self.eq_view(*other, vm)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        f.write_str("dict_keys([")?;
        write_dict_keys_contents(f, self.dict(vm.heap), vm, heap_ids)?;
        f.write_str("])")
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &crate::value::EitherStr,
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
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

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

    /// Compares this items view to another items view using live dict item semantics.
    pub(crate) fn eq_view(self, other: Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.dict_id == other.dict_id {
            return Ok(true);
        }

        Heap::with_two(vm, self.dict_id, other.dict_id, |vm, left, right| {
            let (HeapData::Dict(left), HeapData::Dict(right)) = (left, right) else {
                panic!("dict_items view must always reference dicts");
            };
            if left.len() != right.len() {
                return Ok(false);
            }
            let token = vm.heap.incr_recursion_depth()?;
            defer_drop!(token, vm);
            for (key, value) in left {
                vm.heap.check_time()?;
                if let Ok(Some(other_v)) = right.get(key, vm) {
                    if !value.py_eq(other_v, vm)? {
                        return Ok(false);
                    }
                } else {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }

    /// Compares this items view to a mutable set using set membership semantics.
    pub(crate) fn eq_set(self, other: &Set, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_items view must always reference a dict");
            };
            dict_items_eq_set_like(
                dict,
                other.len(),
                |item, vm| matches!(other.contains(item, vm), Ok(true)),
                vm,
            )
        })
    }

    /// Compares this items view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset(
        self,
        other: &FrozenSet,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_items view must always reference a dict");
            };
            dict_items_eq_set_like(
                dict,
                other.len(),
                |item, vm| matches!(other.contains(item, vm), Ok(true)),
                vm,
            )
        })
    }

    /// Materializes the view's current live `(key, value)` pairs into a plain `set`.
    ///
    /// Each item is allocated as a 2-tuple so later set-like operators and
    /// membership checks observe standard Python tuple semantics.
    pub(crate) fn to_set(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Set> {
        Heap::with_entry_mut(vm, self.dict_id, |vm, data| {
            let HeapDataMut::Dict(dict) = data else {
                panic!("dict_items view must always reference a dict");
            };

            let mut result = Set::with_capacity(dict.len());
            for (key, value) in dict.iter() {
                let item = allocate_tuple(smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)], vm.heap)?;
                result.add(item, vm)?;
            }
            Ok(result)
        })
    }

    /// Implements `dict_items.isdisjoint(iterable)` with CPython's iterable semantics.
    pub(crate) fn isdisjoint_from_value(
        self,
        other: &Value,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<bool> {
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

impl PyTrait for DictItemsView {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::DictItems
    }

    fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        other: &Self,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        self.eq_view(*other, vm)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        f.write_str("dict_items([")?;
        write_dict_items_contents(f, self.dict(vm.heap), vm, heap_ids)?;
        f.write_str("])")
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &crate::value::EitherStr,
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
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

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

impl PyTrait for DictValuesView {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::DictValues
    }

    fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        Ok(false)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        f.write_str("dict_values([")?;
        write_dict_values_contents(f, self.dict(vm.heap), vm, heap_ids)?;
        f.write_str("])")
    }
}

impl HeapItem for DictValuesView {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.dict_id);
    }
}

/// Compares two dicts for key-set equality using membership checks.
fn dict_keys_eq_dict(
    left: &Dict,
    right: &Dict,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> Result<bool, ResourceError> {
    dict_keys_eq_set_like(
        left,
        right.len(),
        |key, vm| matches!(right.get(key, vm), Ok(Some(_))),
        vm,
    )
}

/// Compares a dict's live keys to another set-like container by membership.
fn dict_keys_eq_set_like<T: ResourceTracker>(
    dict: &Dict,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'_, '_, T>) -> bool,
    vm: &mut VM<'_, '_, T>,
) -> Result<bool, ResourceError> {
    if dict.len() != other_len {
        return Ok(false);
    }

    let token = vm.heap.incr_recursion_depth()?;
    defer_drop!(token, vm);
    for (key, _) in dict {
        vm.heap.check_time()?;
        if !contains(key, vm) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Compares a dict's live items to another set-like container by membership.
fn dict_items_eq_set_like<T: ResourceTracker>(
    dict: &Dict,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'_, '_, T>) -> bool,
    vm: &mut VM<'_, '_, T>,
) -> Result<bool, ResourceError> {
    if dict.len() != other_len {
        return Ok(false);
    }

    let token = vm.heap.incr_recursion_depth()?;
    defer_drop!(token, vm);
    for (key, value) in dict {
        vm.heap.check_time()?;
        let item = allocate_tuple(smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)], vm.heap)?;
        defer_drop!(item, vm);
        if !contains(item, vm) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Writes the repr payload for a keys view without its outer wrapper.
fn write_dict_keys_contents(
    f: &mut impl Write,
    dict: &Dict,
    vm: &VM<'_, '_, impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
) -> std::fmt::Result {
    let mut first = true;
    for (key, _) in dict {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        key.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Writes the repr payload for an items view without its outer wrapper.
fn write_dict_items_contents(
    f: &mut impl Write,
    dict: &Dict,
    vm: &VM<'_, '_, impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
) -> std::fmt::Result {
    let mut first = true;
    for (key, value) in dict {
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
fn write_dict_values_contents(
    f: &mut impl Write,
    dict: &Dict,
    vm: &VM<'_, '_, impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
) -> std::fmt::Result {
    let mut first = true;
    for (_, value) in dict {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        value.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Collects an arbitrary iterable into a temporary `set`.
///
/// Dict-view operators accept any iterable on the right-hand side in CPython,
/// including one-shot iterator objects. Reusing the same collection path keeps
/// binary operators and `isdisjoint(...)` consistent with each other.
pub(crate) fn collect_iterable_to_set(
    value: Value,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> Result<Set, RunError> {
    let is_existing_iterator =
        matches!(&value, Value::Ref(heap_id) if matches!(vm.heap.get(*heap_id), HeapData::Iter(_)));

    if is_existing_iterator {
        let mut iterable_guard = crate::heap::HeapGuard::new(value, vm);
        let (iterable, vm) = iterable_guard.as_parts_mut();
        let Value::Ref(iter_id) = iterable else {
            unreachable!("existing iterator check should guarantee a heap iterator");
        };
        let mut set_guard = crate::heap::HeapGuard::new(Set::new(), vm);
        let (set, vm) = set_guard.as_parts_mut();
        while let Some(item) = advance_on_heap(vm.heap, *iter_id, vm.interns)? {
            set.add(item, vm)?;
        }
        return Ok(set_guard.into_inner());
    }

    let iter = MontyIter::new(value, vm)?;
    crate::defer_drop_mut!(iter, vm);
    let mut set_guard = crate::heap::HeapGuard::new(Set::with_capacity(iter.size_hint(vm.heap)), vm);
    let (set, vm) = set_guard.as_parts_mut();
    while let Some(item) = iter.for_next(vm)? {
        set.add(item, vm)?;
    }
    Ok(set_guard.into_inner())
}

/// Returns whether two temporary sets have no elements in common.
fn sets_are_disjoint(left: &Set, right: &Set, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
    let (smaller, larger) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };

    for value in smaller.iter() {
        if larger.contains(value, vm)? {
            return Ok(false);
        }
    }
    Ok(true)
}
