use std::fmt::Write;

use ahash::AHashSet;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{Heap, HeapData, HeapGuard, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, FrozenSet, MontyIter, PyTrait, Set, Type, allocate_tuple},
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

        let HeapReadOutput::Dict(left) = vm.heap.read(self.dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };
        let HeapReadOutput::Dict(right) = vm.heap.read(other.dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };
        dict_keys_eq_dict(&left, &right, vm)
    }

    /// Compares this keys view to a mutable set using set membership semantics.
    pub(crate) fn eq_set<'h>(
        self,
        other: &HeapRead<'h, Set>,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };
        dict_keys_eq_set_like(
            &dict,
            other.get(vm.heap).len(),
            |key, vm| matches!(other.contains(key, vm), Ok(true)),
            vm,
        )
    }

    /// Compares this keys view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset<'h>(
        self,
        other: &HeapRead<'h, FrozenSet>,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };
        dict_keys_eq_set_like(
            &dict,
            other.get(vm.heap).len(),
            |key, vm| matches!(other.contains(key, vm), Ok(true)),
            vm,
        )
    }

    /// Materializes the view's current live keys into a plain `set`.
    ///
    /// Dict-view operators always produce ordinary `set` results in CPython,
    /// so the VM uses this helper as the left-hand-side snapshot for `& | ^ -`
    /// and for `isdisjoint(...)`.
    pub(crate) fn to_set(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Set> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_keys view must always reference a dict");
        };

        let len = dict.get(vm.heap).len();
        let mut result = Set::with_capacity(len);
        for i in 0..len {
            let key = dict.get(vm.heap).key_at(i).unwrap().clone_with_heap(vm);
            result.add(key, vm)?;
        }
        Ok(result)
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

impl<'h> PyTrait<'h> for HeapRead<'h, DictKeysView> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::DictKeys
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        self.get(vm.heap).eq_view(*other.get(vm.heap), vm)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str("dict_keys([")?;
        write_dict_keys_contents(f, self.get(vm.heap).dict(vm.heap), vm, heap_ids)?;
        Ok(f.write_str("])")?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &crate::value::EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let view = *self.get(vm.heap);
        match attr.static_string() {
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("dict_keys.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(CallResult::Value(Value::Bool(view.isdisjoint_from_value(other, vm)?)))
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

        let HeapReadOutput::Dict(left) = vm.heap.read(self.dict_id) else {
            panic!("dict_items view must always reference a dict");
        };
        let HeapReadOutput::Dict(right) = vm.heap.read(other.dict_id) else {
            panic!("dict_items view must always reference a dict");
        };
        left.py_eq(&right, vm)
    }

    /// Compares this items view to a mutable set using set membership semantics.
    pub(crate) fn eq_set<'h>(
        self,
        other: &HeapRead<'h, Set>,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_items view must always reference a dict");
        };
        dict_items_eq_set_like(
            &dict,
            other.get(vm.heap).len(),
            |item, vm| matches!(other.contains(item, vm), Ok(true)),
            vm,
        )
    }

    /// Compares this items view to a frozenset using set membership semantics.
    pub(crate) fn eq_frozenset<'h>(
        self,
        other: &HeapRead<'h, FrozenSet>,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_items view must always reference a dict");
        };
        dict_items_eq_set_like(
            &dict,
            other.get(vm.heap).len(),
            |item, vm| matches!(other.contains(item, vm), Ok(true)),
            vm,
        )
    }

    /// Materializes the view's current live `(key, value)` pairs into a plain `set`.
    ///
    /// Each item is allocated as a 2-tuple so later set-like operators and
    /// membership checks observe standard Python tuple semantics.
    pub(crate) fn to_set(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Set> {
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id) else {
            panic!("dict_items view must always reference a dict");
        };

        let len = dict.get(vm.heap).len();
        let mut result = Set::with_capacity(len);
        for i in 0..len {
            let (key, value) = dict.get(vm.heap).item_at(i).unwrap();
            let item = allocate_tuple(smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)], vm.heap)?;
            result.add(item, vm)?;
        }
        Ok(result)
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

impl<'h> PyTrait<'h> for HeapRead<'h, DictItemsView> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::DictItems
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        self.get(vm.heap).eq_view(*other.get(vm.heap), vm)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str("dict_items([")?;
        write_dict_items_contents(f, self.get(vm.heap).dict(vm.heap), vm, heap_ids)?;
        Ok(f.write_str("])")?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &crate::value::EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let view = *self.get(vm.heap);
        match attr.static_string() {
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("dict_items.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(CallResult::Value(Value::Bool(view.isdisjoint_from_value(other, vm)?)))
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

impl<'h> PyTrait<'h> for HeapRead<'h, DictValuesView> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::DictValues
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).dict(vm.heap).len())
    }

    fn py_eq(
        &self,
        _other: &Self,
        _vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, crate::resource::ResourceError> {
        Ok(false)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str("dict_values([")?;
        write_dict_values_contents(f, self.get(vm.heap).dict(vm.heap), vm, heap_ids)?;
        Ok(f.write_str("])")?)
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
fn dict_keys_eq_dict<'h>(
    left: &HeapRead<'h, Dict>,
    right: &HeapRead<'h, Dict>,
    vm: &mut VM<'h, '_, impl ResourceTracker>,
) -> Result<bool, ResourceError> {
    dict_keys_eq_set_like(
        left,
        right.get(vm.heap).len(),
        |key, vm| matches!(right.contains_key(key, vm), Ok(true)),
        vm,
    )
}

/// Compares a dict's live keys to another set-like container by membership.
fn dict_keys_eq_set_like<'h, T: ResourceTracker>(
    dict: &HeapRead<'h, Dict>,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'h, '_, T>) -> bool,
    vm: &mut VM<'h, '_, T>,
) -> Result<bool, ResourceError> {
    if dict.get(vm.heap).len() != other_len {
        return Ok(false);
    }

    let token = vm.heap.incr_recursion_depth()?;
    defer_drop!(token, vm);
    let len = dict.get(vm.heap).len();
    for i in 0..len {
        vm.heap.check_time()?;
        let key = dict.get(vm.heap).key_at(i).unwrap().clone_with_heap(vm);
        defer_drop!(key, vm);
        if !contains(key, vm) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Compares a dict's live items to another set-like container by membership.
fn dict_items_eq_set_like<'h, T: ResourceTracker>(
    dict: &HeapRead<'h, Dict>,
    other_len: usize,
    mut contains: impl FnMut(&Value, &mut VM<'h, '_, T>) -> bool,
    vm: &mut VM<'h, '_, T>,
) -> Result<bool, ResourceError> {
    if dict.get(vm.heap).len() != other_len {
        return Ok(false);
    }

    let token = vm.heap.incr_recursion_depth()?;
    defer_drop!(token, vm);
    let len = dict.get(vm.heap).len();
    for i in 0..len {
        vm.heap.check_time()?;
        let (key, value) = dict.get(vm.heap).item_at(i).unwrap();
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
) -> RunResult<()> {
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
) -> RunResult<()> {
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
) -> RunResult<()> {
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
    let mut value_guard = HeapGuard::new(value, vm);
    let (value, vm) = value_guard.as_parts_mut();

    // Fast path existing iterators
    if let Value::Ref(heap_id) = value
        && let HeapReadOutput::Iter(mut iter) = vm.heap.read(*heap_id)
    {
        let mut set_guard = HeapGuard::new(Set::new(), vm);
        let (set, vm) = set_guard.as_parts_mut();
        while let Some(item) = iter.advance(vm)? {
            set.add(item, vm)?;
        }
        return Ok(set_guard.into_inner());
    }

    let (value, vm) = value_guard.into_parts();
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
        if vm.heap.protect(larger).contains(value, vm)? {
            return Ok(false);
        }
    }
    Ok(true)
}
