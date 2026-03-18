use std::fmt::Write;

use ahash::AHashSet;
use hashbrown::HashTable;

use super::{MontyIter, PyTrait};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapGuard, HeapId, HeapItem},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::Type,
    value::{EitherStr, Value},
};

/// Entry in the set storage, containing a value and its cached hash.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SetEntry {
    pub(crate) value: Value,
    /// Cached hash for efficient lookup and reinsertion.
    pub(crate) hash: u64,
}

/// Internal storage shared between Set and FrozenSet.
///
/// Uses a `HashTable<usize>` for O(1) lookups combined with a dense `Vec<SetEntry>`
/// to preserve insertion order (consistent with Python 3.7+ dict behavior).
/// The hash table maps value hashes to indices in the entries vector.
#[derive(Debug, Default)]
pub(crate) struct SetStorage {
    /// Maps hash to index in entries vector.
    indices: HashTable<usize>,
    /// Dense vector of entries maintaining insertion order.
    entries: Vec<SetEntry>,
}

impl SetStorage {
    /// Creates a new empty set storage.
    fn new() -> Self {
        Self::default()
    }

    /// Creates a new set storage with pre-allocated capacity.
    fn with_capacity(capacity: usize) -> Self {
        Self {
            indices: HashTable::with_capacity(capacity),
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Creates a SetStorage from a vector of (value, hash) pairs.
    ///
    /// This is used to avoid borrow conflicts when we need to copy another set's
    /// contents and then perform operations requiring mutable heap access.
    /// The caller is responsible for handling reference counting.
    fn from_entries(entries: Vec<(Value, u64)>) -> Self {
        let mut storage = Self::with_capacity(entries.len());
        for (idx, (value, hash)) in entries.into_iter().enumerate() {
            storage.entries.push(SetEntry { value, hash });
            storage.indices.insert_unique(hash, idx, |&i| storage.entries[i].hash);
        }
        storage
    }

    /// Clones entries with proper reference counting.
    fn clone_entries(&self, heap: &impl ContainsHeap) -> Vec<(Value, u64)> {
        self.entries
            .iter()
            .map(|e| (e.value.clone_with_heap(heap), e.hash))
            .collect()
    }

    /// Returns the number of elements in the set.
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the set is empty.
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns whether this set contains any heap references (`Value::Ref`).
    ///
    /// Used during allocation to determine if this container could create cycles.
    #[inline]
    fn has_refs(&self) -> bool {
        self.entries.iter().any(|e| matches!(e.value, Value::Ref(_)))
    }

    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if the element was added (not already present),
    /// `Ok(false)` if the element was already in the set.
    /// Returns `Err` if the element is unhashable.
    ///
    /// The caller transfers ownership of `value`. If the value is already in
    /// the set, it will be dropped.
    fn add(&mut self, value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        let hash = match value.py_hash(vm.heap, vm.interns) {
            Ok(Some(h)) => h,
            Ok(None) => {
                let err = ExcType::type_error_unhashable_set_element(value.py_type(vm.heap));
                value.drop_with_heap(vm.heap);
                return Err(err);
            }
            Err(e) => {
                value.drop_with_heap(vm.heap);
                return Err(e.into());
            }
        };

        // Check if value already exists.
        let existing = self
            .indices
            .find(hash, |&idx| value.py_eq(&self.entries[idx].value, vm).unwrap_or(false));

        if existing.is_some() {
            // Value already in set, drop the new value
            value.drop_with_heap(vm.heap);
            Ok(false)
        } else {
            // Add new entry
            let index = self.entries.len();
            self.entries.push(SetEntry { value, hash });
            self.indices.insert_unique(hash, index, |&idx| self.entries[idx].hash);
            Ok(true)
        }
    }

    /// Removes an element from the set.
    ///
    /// Returns `Ok(true)` if the element was removed, `Ok(false)` if not found.
    /// Returns `Err` if the key is unhashable.
    fn remove(&mut self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        let hash = value
            .py_hash(vm.heap, vm.interns)?
            .ok_or_else(|| ExcType::type_error_unhashable_set_element(value.py_type(vm.heap)))?;

        let entry = self.indices.entry(
            hash,
            |&idx| value.py_eq(&self.entries[idx].value, vm).unwrap_or(false),
            |&idx| self.entries[idx].hash,
        );

        if let hashbrown::hash_table::Entry::Occupied(occ) = entry {
            let index = *occ.get();
            let removed_entry = self.entries.remove(index);
            occ.remove();

            // Update indices for entries that shifted down
            for idx in &mut self.indices {
                if *idx > index {
                    *idx -= 1;
                }
            }

            // Drop the removed value
            removed_entry.value.drop_with_heap(vm);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Removes an element from the set without raising an error if not found.
    ///
    /// Returns `Ok(())` always (unless the key is unhashable).
    fn discard(&mut self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        self.remove(value, vm)?;
        Ok(())
    }

    /// Removes and returns an arbitrary element from the set.
    ///
    /// Returns `Err(KeyError)` if the set is empty.
    fn pop(&mut self) -> RunResult<Value> {
        if self.entries.is_empty() {
            return Err(ExcType::key_error_pop_empty_set());
        }

        // Remove the last entry (most efficient)
        let entry = self.entries.pop().expect("checked non-empty");

        // Remove from hash table
        self.indices
            .find_entry(entry.hash, |&idx| idx == self.entries.len())
            .expect("entry must exist")
            .remove();

        Ok(entry.value)
    }

    /// Removes all elements from the set.
    fn clear(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        self.entries.drain(..).drop_with_heap(heap);
        self.indices.clear();
    }

    /// Creates a deep clone with proper reference counting.
    fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
        Self {
            indices: self.indices.clone(),
            entries: self
                .entries
                .iter()
                .map(|entry| SetEntry {
                    value: entry.value.clone_with_heap(heap),
                    hash: entry.hash,
                })
                .collect(),
        }
    }

    /// Checks if the set contains a value.
    pub fn contains(&self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        let hash = value
            .py_hash(vm.heap, vm.interns)?
            .ok_or_else(|| ExcType::type_error_unhashable_set_element(value.py_type(vm.heap)))?;

        // Set values are typically shallow (strings, ints, tuples of primitives),
        // so recursion errors are unlikely. If one occurs, treat it as "not equal".
        Ok(self
            .indices
            .find(hash, |&idx| value.py_eq(&self.entries[idx].value, vm).unwrap_or(false))
            .is_some())
    }

    /// Returns an iterator over the values in the set.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|e| &e.value)
    }

    /// Returns the value at the given index, if valid.
    ///
    /// Used by MontyIter for index-based iteration.
    pub(crate) fn value_at(&self, index: usize) -> Option<&Value> {
        self.entries.get(index).map(|e| &e.value)
    }

    /// Collects heap IDs for reference counting cleanup.
    fn collect_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for entry in &mut self.entries {
            if let Value::Ref(id) = &entry.value {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                entry.value.dec_ref_forget();
            }
        }
    }

    /// Compares two sets for equality.
    fn eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.len() != other.len() {
            return Ok(false);
        }

        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        // Check that every element in self is in other
        for entry in &self.entries {
            if !matches!(other.contains(&entry.value, vm), Ok(true)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns true if this set is a subset of other.
    fn is_subset(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        for entry in &self.entries {
            if !other.contains(&entry.value, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns true if this set is a superset of other.
    fn is_superset(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        other.is_subset(self, vm)
    }

    /// Returns true if this set has no elements in common with other.
    fn is_disjoint(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Iterate over the smaller set for efficiency
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };

        for entry in &smaller.entries {
            if larger.contains(&entry.value, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns a new set containing elements in either set (union).
    fn union(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let mut result_guard = HeapGuard::new(self.clone_with_heap(vm), vm);
        let (result, vm) = result_guard.as_parts_mut();
        for entry in &other.entries {
            let value = entry.value.clone_with_heap(vm);
            result.add(value, vm)?;
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in both sets (intersection).
    fn intersection(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let mut result_guard = HeapGuard::new(Self::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();
        // Iterate over the smaller set for efficiency
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };

        for entry in &smaller.entries {
            if larger.contains(&entry.value, vm)? {
                let value = entry.value.clone_with_heap(vm);
                result.add(value, vm)?;
            }
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in self but not in other (difference).
    fn difference(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let mut result_guard = HeapGuard::new(Self::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();
        for entry in &self.entries {
            if !other.contains(&entry.value, vm)? {
                let value = entry.value.clone_with_heap(vm);
                result.add(value, vm)?;
            }
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in either set but not both (symmetric difference).
    fn symmetric_difference(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let mut result_guard = HeapGuard::new(Self::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();

        // Add elements in self but not in other
        for entry in &self.entries {
            if !other.contains(&entry.value, vm)? {
                let value = entry.value.clone_with_heap(vm);
                result.add(value, vm)?;
            }
        }

        // Add elements in other but not in self
        for entry in &other.entries {
            if !self.contains(&entry.value, vm)? {
                let value = entry.value.clone_with_heap(vm);
                result.add(value, vm)?;
            }
        }

        Ok(result_guard.into_inner())
    }

    /// Adds all elements from other to this set (in-place union).
    fn update(&mut self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        for entry in &other.entries {
            let value = entry.value.clone_with_heap(vm);
            self.add(value, vm)?;
        }
        Ok(())
    }

    /// Writes the repr format to a formatter.
    ///
    /// For sets, outputs `{elem1, elem2, ...}` (no type prefix).
    /// For frozensets, outputs `frozenset({elem1, elem2, ...})`.
    fn repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        type_name: &str,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return write!(f, "{type_name}()");
        }

        // Check depth limit before recursing
        let Some(token) = vm.heap.incr_recursion_depth_for_repr() else {
            return f.write_str("{...}");
        };
        crate::defer_drop_immutable_heap!(token, vm);

        // frozenset needs type prefix: frozenset({...}), but set doesn't: {...}
        let needs_prefix = type_name != "set";
        if needs_prefix {
            write!(f, "{type_name}(")?;
        }

        f.write_char('{')?;
        let mut first = true;
        for entry in &self.entries {
            if !first {
                if vm.heap.check_time().is_err() {
                    f.write_str(", ...[timeout]")?;
                    break;
                }
                f.write_str(", ")?;
            }
            first = false;
            entry.value.py_repr_fmt(f, vm, heap_ids)?;
        }
        f.write_char('}')?;

        if needs_prefix {
            f.write_char(')')?;
        }

        Ok(())
    }

    /// Estimates the memory size of this storage.
    fn estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.len() * std::mem::size_of::<SetEntry>()
    }
}

/// Python set type - mutable, unordered collection of unique hashable elements.
///
/// Sets support standard operations like add, remove, discard, pop, clear, as well
/// as set algebra operations like union, intersection, difference, and symmetric
/// difference.
///
/// # Reference Counting
/// When values are added, their reference counts are NOT incremented by the set -
/// the caller transfers ownership. When values are removed or the set is cleared,
/// their reference counts are decremented.
#[derive(Debug, Default)]
pub(crate) struct Set(SetStorage);

impl Set {
    /// Creates a new empty set.
    #[must_use]
    pub fn new() -> Self {
        Self(SetStorage::new())
    }

    /// Creates a set with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self(SetStorage::with_capacity(capacity))
    }

    /// Returns the number of elements in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns whether this set contains any heap references (`Value::Ref`).
    ///
    /// Used during allocation to determine if this container could create cycles.
    #[inline]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        self.0.has_refs()
    }

    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if added, `Ok(false)` if already present.
    pub fn add(&mut self, value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        self.0.add(value, vm)
    }

    /// Removes an element from the set.
    ///
    /// Returns `Err(KeyError)` if the element is not present.
    pub fn remove(&mut self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        if self.0.remove(value, vm)? {
            Ok(())
        } else {
            Err(ExcType::key_error(value, vm))
        }
    }

    /// Removes an element from the set if present.
    ///
    /// Does not raise an error if the element is not found.
    pub fn discard(&mut self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        self.0.discard(value, vm)
    }

    /// Removes and returns an arbitrary element from the set.
    ///
    /// Returns `Err(KeyError)` if the set is empty.
    pub fn pop(&mut self) -> RunResult<Value> {
        self.0.pop()
    }

    /// Removes all elements from the set.
    pub fn clear(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        self.0.clear(heap);
    }

    /// Returns a shallow copy of the set.
    #[must_use]
    pub fn copy(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self(self.0.clone_with_heap(heap))
    }

    /// Checks if the set contains a value.
    pub fn contains(&self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        self.0.contains(value, vm)
    }

    /// Returns the internal storage (for set operations between Set and FrozenSet).
    pub(crate) fn storage(&self) -> &SetStorage {
        &self.0
    }

    /// Returns an iterator over the set's elements in insertion order.
    ///
    /// This is primarily used by other runtime helpers that need to implement
    /// set-like protocols while still preserving Monty's single canonical set
    /// storage implementation.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Value> {
        self.0.iter()
    }

    /// Creates a set from the `set()` constructor call.
    ///
    /// - `set()` with no args returns an empty set
    /// - `set(iterable)` creates a set from any iterable (list, tuple, set, dict, range, str, bytes)
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("set", vm.heap)?;
        let set = match value {
            None => Self::new(),
            Some(v) => Self::from_iterable(v, vm)?,
        };
        let heap_id = vm.heap.allocate(HeapData::Set(set))?;
        Ok(Value::Ref(heap_id))
    }

    /// Creates a set from a MontyIter, adding elements one by one.
    ///
    /// Unlike list/tuple which can just collect into a Vec, sets need to add
    /// each element individually to handle duplicates and compute hashes.
    fn from_iterator(iter: MontyIter, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        defer_drop_mut!(iter, vm);
        let mut set = Self::with_capacity(iter.size_hint(vm.heap));
        while let Some(item) = iter.for_next(vm)? {
            set.add(item, vm)?;
        }
        Ok(set)
    }

    /// Creates a set from an iterable value.
    ///
    /// This is a convenience method used by helper methods that need to convert
    /// arbitrary iterables to sets. It uses `MontyIter` internally.
    fn from_iterable(iterable: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let iter = MontyIter::new(iterable, vm)?;
        let set = Self::from_iterator(iter, vm)?;
        Ok(set)
    }
}

impl DropWithHeap for Set {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.0.drop_with_heap(heap);
    }
}

impl DropWithHeap for SetStorage {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.entries.drop_with_heap(heap);
    }
}

impl DropWithHeap for FrozenSet {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.0.drop_with_heap(heap);
    }
}

impl DropWithHeap for SetEntry {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.value.drop_with_heap(heap);
    }
}

impl PyTrait for Set {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Set
    }

    fn py_len(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.len() != other.len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        self.0.eq(&other.0, vm)
    }

    fn py_bool(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        self.0.repr_fmt(f, vm, heap_ids, "set")
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let value = match attr.static_string() {
            Some(StaticStrings::Add) => {
                let value = args.get_one_arg("set.add", vm.heap)?;
                self.add(value, vm)?;
                Ok(Value::None)
            }
            Some(StaticStrings::Remove) => {
                let value = args.get_one_arg("set.remove", vm.heap)?;
                defer_drop!(value, vm);
                self.remove(value, vm)?;
                Ok(Value::None)
            }
            Some(StaticStrings::Discard) => {
                let value = args.get_one_arg("set.discard", vm.heap)?;
                defer_drop!(value, vm);
                self.discard(value, vm)?;
                Ok(Value::None)
            }
            Some(StaticStrings::Pop) => {
                args.check_zero_args("set.pop", vm.heap)?;
                self.pop()
            }
            Some(StaticStrings::Clear) => {
                args.check_zero_args("set.clear", vm.heap)?;
                self.clear(vm.heap);
                Ok(Value::None)
            }
            Some(StaticStrings::Copy) => {
                args.check_zero_args("set.copy", vm.heap)?;
                let copy = self.copy(vm.heap);
                let heap_id = vm.heap.allocate(HeapData::Set(copy))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Update) => {
                let other = args.get_one_arg("set.update", vm.heap)?;
                self.update_from_value(other, vm)?;
                Ok(Value::None)
            }
            Some(StaticStrings::Union) => {
                let other = args.get_one_arg("set.union", vm.heap)?;
                let result = self.union_from_value(other, vm)?;
                let heap_id = vm.heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Intersection) => {
                let other = args.get_one_arg("set.intersection", vm.heap)?;
                let result = self.intersection_from_value(other, vm)?;
                let heap_id = vm.heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Difference) => {
                let other = args.get_one_arg("set.difference", vm.heap)?;
                let result = self.difference_from_value(other, vm)?;
                let heap_id = vm.heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::SymmetricDifference) => {
                let other = args.get_one_arg("set.symmetric_difference", vm.heap)?;
                let result = self.symmetric_difference_from_value(other, vm)?;
                let heap_id = vm.heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Issubset) => {
                let other = args.get_one_arg("set.issubset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.issubset_from_value(other, vm)?))
            }
            Some(StaticStrings::Issuperset) => {
                let other = args.get_one_arg("set.issuperset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.issuperset_from_value(other, vm)?))
            }
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("set.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.isdisjoint_from_value(other, vm)?))
            }
            _ => {
                args.drop_with_heap(vm);
                return Err(ExcType::attribute_error(Type::Set, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }

    fn py_sub(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // This is called from heap.rs with two Sets
        // We need interns for contains check, but py_sub doesn't have it
        // This is a limitation - we'll need to handle this differently
        // For now, return None to indicate not supported via this path
        Ok(None)
    }
}

/// Pure set/frozenset binary operators shared by both concrete container types.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SetBinaryOp {
    And,
    Or,
    Xor,
    Sub,
}

/// Helper methods for set operations with arbitrary iterables.
impl Set {
    /// Implements operator-form set algebra, which only accepts set/frozenset operands.
    ///
    /// Unlike method forms such as `set.union(iterable)`, the binary operators
    /// `& | ^ -` are intentionally strict and return `None` for operands outside
    /// the set-like values CPython accepts here (`set`, `frozenset`,
    /// `dict_keys`, and `dict_items`) so the VM can raise the standard
    /// unsupported-operands `TypeError`.
    pub(crate) fn binary_op_value(
        &self,
        other: &Value,
        op: SetBinaryOp,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Self>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);

        let result = match op {
            SetBinaryOp::And => Self(self.0.intersection(other_storage, vm)?),
            SetBinaryOp::Or => Self(self.0.union(other_storage, vm)?),
            SetBinaryOp::Xor => Self(self.0.symmetric_difference(other_storage, vm)?),
            SetBinaryOp::Sub => Self(self.0.difference(other_storage, vm)?),
        };
        Ok(Some(result))
    }

    /// Updates this set with elements from an iterable value.
    fn update_from_value(&mut self, other: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
        let heap = &mut *vm.heap;
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match &other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            other.drop_with_heap(heap);
            for (value, _hash) in entries {
                self.add(value, vm)?;
            }
            return Ok(());
        }

        // Fall back to creating a temporary set from the iterable
        let temp_set = Self::from_iterable(other, vm)?;
        defer_drop!(temp_set, vm);
        self.0.update(&temp_set.0, vm)?;
        Ok(())
    }

    /// Returns a new set with elements from both this set and an iterable.
    fn union_from_value(&self, other: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let other_storage = Self::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let result_storage = self.0.union(other_storage, vm)?;
        Ok(Self(result_storage))
    }

    /// Returns a new set with elements common to both this set and an iterable.
    fn intersection_from_value(&self, other: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let other_storage = Self::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let result_storage = self.0.intersection(other_storage, vm)?;
        Ok(Self(result_storage))
    }

    /// Returns a new set with elements in this set but not in an iterable.
    fn difference_from_value(&self, other: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let other_storage = Self::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let result_storage = self.0.difference(other_storage, vm)?;
        Ok(Self(result_storage))
    }

    /// Returns a new set with elements in either set but not both.
    fn symmetric_difference_from_value(
        &self,
        other: Value,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Self> {
        let other_storage = Self::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let result_storage = self.0.symmetric_difference(other_storage, vm)?;
        Ok(Self(result_storage))
    }

    /// Checks if this set is a subset of an iterable.
    fn issubset_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_subset(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Self::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_subset(&temp.0, vm)
    }

    /// Checks if this set is a superset of an iterable.
    fn issuperset_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_superset(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Self::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_superset(&temp.0, vm)
    }

    /// Checks if this set has no elements in common with an iterable.
    fn isdisjoint_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_disjoint(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Self::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_disjoint(&temp.0, vm)
    }

    /// Helper to get SetStorage from a Value (either directly or by conversion).
    fn get_storage_from_value(value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<SetStorage> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match &value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(set) => Some(set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(set) => Some(set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            value.drop_with_heap(vm);
            return Ok(SetStorage::from_entries(entries));
        }

        // Convert iterable to set
        let temp_set = Self::from_iterable(value, vm)?;
        Ok(temp_set.0)
    }
}

impl HeapItem for Set {
    fn py_estimate_size(&self) -> usize {
        self.0.estimate_size()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.collect_dec_ref_ids(stack);
    }
}

/// Python frozenset type - immutable, unordered collection of unique hashable elements.
///
/// FrozenSets support the same set algebra operations as sets (union, intersection,
/// difference, symmetric difference) but are immutable and therefore hashable.
///
/// # Hashability
/// Unlike mutable sets, frozensets can be used as dict keys or set elements because
/// they are immutable. The hash is computed as the XOR of element hashes (order-independent).
#[derive(Debug, Default)]
pub(crate) struct FrozenSet(SetStorage);

impl FrozenSet {
    /// Creates a new empty frozenset.
    #[must_use]
    pub fn new() -> Self {
        Self(SetStorage::new())
    }

    /// Returns the number of elements in the frozenset.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the frozenset is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns whether this frozenset contains any heap references (`Value::Ref`).
    ///
    /// Used during allocation to determine if this container could create cycles.
    #[inline]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        self.0.has_refs()
    }

    /// Returns a shallow copy of the frozenset.
    #[must_use]
    pub fn copy(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self(self.0.clone_with_heap(heap))
    }

    /// Returns the internal storage.
    pub(crate) fn storage(&self) -> &SetStorage {
        &self.0
    }

    /// Checks if the frozenset contains a value.
    pub fn contains(&self, value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        self.0.contains(value, vm)
    }

    /// Computes the hash of this frozenset.
    ///
    /// The hash is the XOR of all element hashes, making it order-independent.
    /// Checks recursion depth before recursing into element hashes.
    pub fn compute_hash(
        &self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<Option<u64>, ResourceError> {
        let token = heap.incr_recursion_depth()?;
        defer_drop!(token, heap);
        let mut hash: u64 = 0;
        for entry in &self.0.entries {
            // All elements must be hashable (enforced at construction)
            match entry.value.py_hash(heap, interns)? {
                Some(h) => hash ^= h,
                None => return Ok(None),
            }
        }
        Ok(Some(hash))
    }

    /// Creates a frozenset from a Set, consuming the Set's storage.
    ///
    /// This is used when we need to convert a mutable set to an immutable frozenset
    /// without cloning.
    pub fn from_set(set: Set) -> Self {
        Self(set.0)
    }

    /// Creates a frozenset from the `frozenset()` constructor call.
    ///
    /// - `frozenset()` with no args returns an empty frozenset
    /// - `frozenset(iterable)` creates a frozenset from any iterable (list, tuple, set, dict, range, str, bytes)
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("frozenset", vm.heap)?;
        let frozenset = match value {
            None => Self::new(),
            Some(v) => Self::from_set(Set::from_iterable(v, vm)?),
        };
        let heap_id = vm.heap.allocate(HeapData::FrozenSet(frozenset))?;
        Ok(Value::Ref(heap_id))
    }

    /// Returns a new frozenset with elements from both this and another set.
    pub(crate) fn union(&self, other: &SetStorage, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        Ok(Self(self.0.union(other, vm)?))
    }

    /// Returns a new frozenset with elements common to both sets.
    pub(crate) fn intersection(
        &self,
        other: &SetStorage,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Self> {
        Ok(Self(self.0.intersection(other, vm)?))
    }

    /// Returns a new frozenset with elements in this set but not in other.
    pub(crate) fn difference(&self, other: &SetStorage, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        Ok(Self(self.0.difference(other, vm)?))
    }

    /// Returns a new frozenset with elements in either set but not both.
    pub(crate) fn symmetric_difference(
        &self,
        other: &SetStorage,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Self> {
        Ok(Self(self.0.symmetric_difference(other, vm)?))
    }
}

impl PyTrait for FrozenSet {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::FrozenSet
    }

    fn py_len(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.len() != other.len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        self.0.eq(&other.0, vm)
    }

    fn py_bool(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        self.0.repr_fmt(f, vm, heap_ids, "frozenset")
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let heap = &mut *vm.heap;
        let interns = vm.interns;
        let value = match attr.static_string() {
            Some(StaticStrings::Copy) => {
                args.check_zero_args("frozenset.copy", heap)?;
                let copy = self.copy(heap);
                let heap_id = heap.allocate(HeapData::FrozenSet(copy))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Union) => {
                let other = args.get_one_arg("frozenset.union", heap)?;
                let other_storage = Set::get_storage_from_value(other, vm)?;
                defer_drop!(other_storage, vm);
                let result = self.union(other_storage, vm)?;
                let heap_id = vm.heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Intersection) => {
                let other = args.get_one_arg("frozenset.intersection", heap)?;
                let other_storage = Set::get_storage_from_value(other, vm)?;
                defer_drop!(other_storage, vm);
                let result = self.intersection(other_storage, vm)?;
                let heap_id = vm.heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Difference) => {
                let other = args.get_one_arg("frozenset.difference", heap)?;
                let other_storage = Set::get_storage_from_value(other, vm)?;
                defer_drop!(other_storage, vm);
                let result = self.difference(other_storage, vm)?;
                let heap_id = vm.heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::SymmetricDifference) => {
                let other = args.get_one_arg("frozenset.symmetric_difference", heap)?;
                let other_storage = Set::get_storage_from_value(other, vm)?;
                defer_drop!(other_storage, vm);
                let result = self.symmetric_difference(other_storage, vm)?;
                let heap_id = vm.heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Issubset) => {
                let other = args.get_one_arg("frozenset.issubset", heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.issubset_from_value(other, vm)?))
            }
            Some(StaticStrings::Issuperset) => {
                let other = args.get_one_arg("frozenset.issuperset", heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.issuperset_from_value(other, vm)?))
            }
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("frozenset.isdisjoint", heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.isdisjoint_from_value(other, vm)?))
            }
            _ => {
                args.drop_with_heap(heap);
                return Err(ExcType::attribute_error(Type::FrozenSet, attr.as_str(interns)));
            }
        };
        value.map(CallResult::Value)
    }

    fn py_sub(
        &self,
        _other: &Self,
        _vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // Same limitation as Set - needs interns
        Ok(None)
    }
}

impl HeapItem for FrozenSet {
    fn py_estimate_size(&self) -> usize {
        self.0.estimate_size()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.collect_dec_ref_ids(stack);
    }
}

/// Helper methods for frozenset operations with arbitrary iterables.
impl FrozenSet {
    /// Implements operator-form set algebra, which only accepts set/frozenset operands.
    ///
    /// CPython returns the type of the left operand for pure set/frozenset binary
    /// operators, so this helper keeps the result as `frozenset` even when the
    /// right operand is a mutable `set`. Like `set`, the accepted right-hand
    /// side includes CPython's set-like dict views.
    pub(crate) fn binary_op_value(
        &self,
        other: &Value,
        op: SetBinaryOp,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Self>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);

        let result = match op {
            SetBinaryOp::And => Self(self.0.intersection(other_storage, vm)?),
            SetBinaryOp::Or => Self(self.0.union(other_storage, vm)?),
            SetBinaryOp::Xor => Self(self.0.symmetric_difference(other_storage, vm)?),
            SetBinaryOp::Sub => Self(self.0.difference(other_storage, vm)?),
        };
        Ok(Some(result))
    }

    /// Checks if this frozenset is a subset of an iterable.
    fn issubset_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Build temporary storage and check
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_subset(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_subset(&temp.0, vm)
    }

    /// Checks if this frozenset is a superset of an iterable.
    fn issuperset_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Build temporary storage and check
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_superset(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_superset(&temp.0, vm)
    }

    /// Checks if this frozenset has no elements in common with an iterable.
    fn isdisjoint_from_value(&self, other: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(other_set) => Some(other_set.0.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Build temporary storage and check
            let other_storage = SetStorage::from_entries(entries);
            defer_drop!(other_storage, vm);
            return self.0.is_disjoint(other_storage, vm);
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(vm), vm)?;
        defer_drop!(temp, vm);
        self.0.is_disjoint(&temp.0, vm)
    }
}

/// Returns temporary set storage only for operator-valid set operands.
///
/// This is stricter than `Set::get_storage_from_value(...)`: operator forms
/// only accept CPython's set-like operands (`set`, `frozenset`, `dict_keys`,
/// and `dict_items`), while method forms accept any iterable.
fn get_storage_from_set_operand(
    value: &Value,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Option<SetStorage>> {
    let Value::Ref(id) = value else {
        return Ok(None);
    };

    match vm.heap.get(*id) {
        HeapData::Set(set) => Ok(Some(SetStorage::from_entries(set.0.clone_entries(vm.heap)))),
        HeapData::FrozenSet(set) => Ok(Some(SetStorage::from_entries(set.0.clone_entries(vm.heap)))),
        // Dict views are `Copy` — matched value is not borrowed from the heap,
        // so `to_set` can take `&mut VM` below without conflict.
        HeapData::DictKeysView(view) => {
            let Set(storage) = view.to_set(vm)?;
            Ok(Some(storage))
        }
        HeapData::DictItemsView(view) => {
            let Set(storage) = view.to_set(vm)?;
            Ok(Some(storage))
        }
        _ => Ok(None),
    }
}

// Custom serde implementations for SetStorage, Set, and FrozenSet.
// Only serialize entries; rebuild the indices hash table on deserialize.

impl serde::Serialize for SetStorage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SetStorage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries: Vec<SetEntry> = serde::Deserialize::deserialize(deserializer)?;
        // Rebuild the indices hash table from the entries
        let mut indices = HashTable::with_capacity(entries.len());
        for (idx, entry) in entries.iter().enumerate() {
            indices.insert_unique(entry.hash, idx, |&i| entries[i].hash);
        }
        Ok(Self { indices, entries })
    }
}

impl serde::Serialize for Set {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Set {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(SetStorage::deserialize(deserializer)?))
    }
}

impl serde::Serialize for FrozenSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for FrozenSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(SetStorage::deserialize(deserializer)?))
    }
}
