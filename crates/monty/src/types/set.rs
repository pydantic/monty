use std::fmt::Write;

use ahash::AHashSet;
use hashbrown::HashTable;

use super::PyTrait;
use crate::{
    args::ArgValues,
    exception_private::ExcType,
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::{attr, Interns},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::Type,
    value::{Attr, Value},
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

    /// Drops all values in this storage, decrementing their reference counts.
    fn drop_all_values(self, heap: &mut Heap<impl ResourceTracker>) {
        for entry in self.entries {
            entry.value.drop_with_heap(heap);
        }
    }

    /// Copies entries without incrementing reference counts.
    ///
    /// Used to break borrow conflicts: copy entries first, then after the
    /// borrow ends, call `inc_refs_for_entries` to fix up refcounts.
    fn copy_entries(&self) -> Vec<(Value, u64)> {
        self.entries
            .iter()
            .map(|e| (e.value.copy_for_extend(), e.hash))
            .collect()
    }

    /// Increments reference counts for all Ref values in an entries vector.
    ///
    /// Call this after `copy_entries` once the original borrow is released.
    fn inc_refs_for_entries(entries: &[(Value, u64)], heap: &mut Heap<impl ResourceTracker>) {
        for (v, _) in entries {
            if let Value::Ref(id) = v {
                heap.inc_ref(*id);
            }
        }
    }

    /// Returns the number of elements in the set.
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the set is empty.
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if the element was added (not already present),
    /// `Ok(false)` if the element was already in the set.
    /// Returns `Err` if the element is unhashable.
    ///
    /// The caller transfers ownership of `value`. If the value is already in
    /// the set, it will be dropped.
    fn add(&mut self, value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        let Some(hash) = value.py_hash(heap, interns) else {
            let err = ExcType::type_error_unhashable_set_element(value.py_type(Some(heap)));
            value.drop_with_heap(heap);
            return Err(err);
        };

        // Check if value already exists
        let existing = self
            .indices
            .find(hash, |&idx| value.py_eq(&self.entries[idx].value, heap, interns));

        if existing.is_some() {
            // Value already in set, drop the new value
            value.drop_with_heap(heap);
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
    fn remove(&mut self, value: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        let hash = value
            .py_hash(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable_set_element(value.py_type(Some(heap))))?;

        let entry = self.indices.entry(
            hash,
            |&idx| value.py_eq(&self.entries[idx].value, heap, interns),
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
            removed_entry.value.drop_with_heap(heap);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Removes an element from the set without raising an error if not found.
    ///
    /// Returns `Ok(())` always (unless the key is unhashable).
    fn discard(&mut self, value: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<()> {
        self.remove(value, heap, interns)?;
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
        for entry in self.entries.drain(..) {
            entry.value.drop_with_heap(heap);
        }
        self.indices.clear();
    }

    /// Creates a deep clone with proper reference counting.
    fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
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
    fn contains(&self, value: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        let hash = value
            .py_hash(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable_set_element(value.py_type(Some(heap))))?;

        Ok(self
            .indices
            .find(hash, |&idx| value.py_eq(&self.entries[idx].value, heap, interns))
            .is_some())
    }

    /// Returns an iterator over the values in the set.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|e| &e.value)
    }

    /// Returns the value at the given index, if valid.
    ///
    /// Used by ForIterator for index-based iteration.
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
    fn eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.len() != other.len() {
            return false;
        }

        // Check that every element in self is in other
        for entry in &self.entries {
            match other.contains(&entry.value, heap, interns) {
                Ok(true) => {}
                _ => return false,
            }
        }
        true
    }

    /// Returns true if this set is a subset of other.
    fn is_subset(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        for entry in &self.entries {
            if !other.contains(&entry.value, heap, interns)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns true if this set is a superset of other.
    fn is_superset(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        other.is_subset(self, heap, interns)
    }

    /// Returns true if this set has no elements in common with other.
    fn is_disjoint(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        // Iterate over the smaller set for efficiency
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };

        for entry in &smaller.entries {
            if larger.contains(&entry.value, heap, interns)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns a new set containing elements in either set (union).
    fn union(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        let mut result = self.clone_with_heap(heap);
        for entry in &other.entries {
            let value = entry.value.clone_with_heap(heap);
            result.add(value, heap, interns)?;
        }
        Ok(result)
    }

    /// Returns a new set containing elements in both sets (intersection).
    fn intersection(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        let mut result = Self::new();
        // Iterate over the smaller set for efficiency
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };

        for entry in &smaller.entries {
            if larger.contains(&entry.value, heap, interns)? {
                let value = entry.value.clone_with_heap(heap);
                result.add(value, heap, interns)?;
            }
        }
        Ok(result)
    }

    /// Returns a new set containing elements in self but not in other (difference).
    fn difference(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        let mut result = Self::new();
        for entry in &self.entries {
            if !other.contains(&entry.value, heap, interns)? {
                let value = entry.value.clone_with_heap(heap);
                result.add(value, heap, interns)?;
            }
        }
        Ok(result)
    }

    /// Returns a new set containing elements in either set but not both (symmetric difference).
    fn symmetric_difference(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        let mut result = Self::new();

        // Add elements in self but not in other
        for entry in &self.entries {
            if !other.contains(&entry.value, heap, interns)? {
                let value = entry.value.clone_with_heap(heap);
                result.add(value, heap, interns)?;
            }
        }

        // Add elements in other but not in self
        for entry in &other.entries {
            if !self.contains(&entry.value, heap, interns)? {
                let value = entry.value.clone_with_heap(heap);
                result.add(value, heap, interns)?;
            }
        }

        Ok(result)
    }

    /// Adds all elements from other to this set (in-place union).
    fn update(&mut self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<()> {
        for entry in &other.entries {
            let value = entry.value.clone_with_heap(heap);
            self.add(value, heap, interns)?;
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
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
        type_name: &str,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return write!(f, "{type_name}()");
        }

        // frozenset needs type prefix: frozenset({...}), but set doesn't: {...}
        let needs_prefix = type_name != "set";
        if needs_prefix {
            write!(f, "{type_name}(")?;
        }

        f.write_char('{')?;
        let mut first = true;
        for entry in &self.entries {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            entry.value.py_repr_fmt(f, heap, heap_ids, interns)?;
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
pub struct Set(SetStorage);

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

    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if added, `Ok(false)` if already present.
    pub fn add(&mut self, value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
        self.0.add(value, heap, interns)
    }

    /// Removes an element from the set.
    ///
    /// Returns `Err(KeyError)` if the element is not present.
    pub fn remove(&mut self, value: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<()> {
        if self.0.remove(value, heap, interns)? {
            Ok(())
        } else {
            Err(ExcType::key_error(value, heap, interns))
        }
    }

    /// Removes an element from the set if present.
    ///
    /// Does not raise an error if the element is not found.
    pub fn discard(
        &mut self,
        value: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        self.0.discard(value, heap, interns)
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

    /// Creates a deep clone with proper reference counting.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self(self.0.clone_with_heap(heap))
    }

    /// Returns the internal storage (for set operations between Set and FrozenSet).
    pub(crate) fn storage(&self) -> &SetStorage {
        &self.0
    }

    /// Creates a set from the `set()` constructor call.
    ///
    /// - `set()` with no args returns an empty set
    /// - `set(iterable)` creates a set from any iterable (list, tuple, set, dict, range, str, bytes)
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("set")?;
        let set = match value {
            None => Self::new(),
            Some(v) => Self::from_iterable(v, heap, interns)?,
        };
        let heap_id = heap.allocate(HeapData::Set(set))?;
        Ok(Value::Ref(heap_id))
    }

    /// Creates a set from a ForIterator, adding elements one by one.
    ///
    /// Unlike list/tuple which can just collect into a Vec, sets need to add
    /// each element individually to handle duplicates and compute hashes.
    fn from_iterator(
        mut iter: ForIterator,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        let mut set = Self::with_capacity(iter.size_hint(heap));
        while let Some(item) = iter.for_next(heap, interns)? {
            set.add(item, heap, interns)?;
        }
        iter.drop_with_heap(heap);
        Ok(set)
    }

    /// Creates a set from an iterable value.
    ///
    /// This is a convenience method used by helper methods that need to convert
    /// arbitrary iterables to sets. It uses `ForIterator` internally.
    fn from_iterable(iterable: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        let iter = ForIterator::new(iterable, heap, interns)?;
        let set = Self::from_iterator(iter, heap, interns)?;
        Ok(set)
    }
}

impl PyTrait for Set {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Set
    }

    fn py_estimate_size(&self) -> usize {
        self.0.estimate_size()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        self.0.eq(&other.0, heap, interns)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.collect_dec_ref_ids(stack);
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        self.0.repr_fmt(f, heap, heap_ids, interns, "set")
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(attr_id) = attr.string_id() else {
            return Err(ExcType::attribute_error(Type::Set, attr.as_str(interns)));
        };

        match attr_id {
            attr::ADD => {
                let value = args.get_one_arg("set.add")?;
                self.add(value, heap, interns)?;
                Ok(Value::None)
            }
            attr::REMOVE => {
                let value = args.get_one_arg("set.remove")?;
                let result = self.remove(&value, heap, interns);
                value.drop_with_heap(heap);
                result?;
                Ok(Value::None)
            }
            attr::DISCARD => {
                let value = args.get_one_arg("set.discard")?;
                let result = self.discard(&value, heap, interns);
                value.drop_with_heap(heap);
                result?;
                Ok(Value::None)
            }
            attr::POP => {
                args.check_zero_args("set.pop")?;
                self.pop()
            }
            attr::CLEAR => {
                args.check_zero_args("set.clear")?;
                self.clear(heap);
                Ok(Value::None)
            }
            attr::COPY => {
                args.check_zero_args("set.copy")?;
                let copy = self.copy(heap);
                let heap_id = heap.allocate(HeapData::Set(copy))?;
                Ok(Value::Ref(heap_id))
            }
            attr::UPDATE => {
                let other = args.get_one_arg("set.update")?;
                self.update_from_value(other, heap, interns)?;
                Ok(Value::None)
            }
            attr::UNION => {
                let other = args.get_one_arg("set.union")?;
                let result = self.union_from_value(other, heap, interns)?;
                let heap_id = heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::INTERSECTION => {
                let other = args.get_one_arg("set.intersection")?;
                let result = self.intersection_from_value(other, heap, interns)?;
                let heap_id = heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::DIFFERENCE => {
                let other = args.get_one_arg("set.difference")?;
                let result = self.difference_from_value(other, heap, interns)?;
                let heap_id = heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::SYMMETRIC_DIFFERENCE => {
                let other = args.get_one_arg("set.symmetric_difference")?;
                let result = self.symmetric_difference_from_value(other, heap, interns)?;
                let heap_id = heap.allocate(HeapData::Set(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::ISSUBSET => {
                let other = args.get_one_arg("set.issubset")?;
                let result = self.issubset_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            attr::ISSUPERSET => {
                let other = args.get_one_arg("set.issuperset")?;
                let result = self.issuperset_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            attr::ISDISJOINT => {
                let other = args.get_one_arg("set.isdisjoint")?;
                let result = self.isdisjoint_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            _ => Err(ExcType::attribute_error(Type::Set, attr.as_str(interns))),
        }
    }

    fn py_sub(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // This is called from heap.rs with two Sets
        // We need interns for contains check, but py_sub doesn't have it
        // This is a limitation - we'll need to handle this differently
        // For now, return None to indicate not supported via this path
        Ok(None)
    }
}

/// Helper methods for set operations with arbitrary iterables.
impl Set {
    /// Updates this set with elements from an iterable value.
    fn update_from_value(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match &other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, now we can use heap mutably
            // IMPORTANT: Inc refs BEFORE dropping the source to avoid use-after-free
            SetStorage::inc_refs_for_entries(&entries, heap);
            other.drop_with_heap(heap);
            for (value, _hash) in entries {
                self.add(value, heap, interns)?;
            }
            return Ok(());
        }

        // Fall back to creating a temporary set from the iterable
        let temp_set = Set::from_iterable(other, heap, interns)?;
        self.0.update(&temp_set.0, heap, interns)?;
        temp_set.0.drop_all_values(heap);
        Ok(())
    }

    /// Returns a new set with elements from both this set and an iterable.
    fn union_from_value(
        &self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Set> {
        let other_storage = Self::get_storage_from_value(other, heap, interns)?;
        let result_storage = self.0.union(&other_storage, heap, interns)?;
        // Clean up other_storage if it was created from a non-set
        for entry in other_storage.entries {
            entry.value.drop_with_heap(heap);
        }
        Ok(Set(result_storage))
    }

    /// Returns a new set with elements common to both this set and an iterable.
    fn intersection_from_value(
        &self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Set> {
        let other_storage = Self::get_storage_from_value(other, heap, interns)?;
        let result_storage = self.0.intersection(&other_storage, heap, interns)?;
        for entry in other_storage.entries {
            entry.value.drop_with_heap(heap);
        }
        Ok(Set(result_storage))
    }

    /// Returns a new set with elements in this set but not in an iterable.
    fn difference_from_value(
        &self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Set> {
        let other_storage = Self::get_storage_from_value(other, heap, interns)?;
        let result_storage = self.0.difference(&other_storage, heap, interns)?;
        for entry in other_storage.entries {
            entry.value.drop_with_heap(heap);
        }
        Ok(Set(result_storage))
    }

    /// Returns a new set with elements in either set but not both.
    fn symmetric_difference_from_value(
        &self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Set> {
        let other_storage = Self::get_storage_from_value(other, heap, interns)?;
        let result_storage = self.0.symmetric_difference(&other_storage, heap, interns)?;
        for entry in other_storage.entries {
            entry.value.drop_with_heap(heap);
        }
        Ok(Set(result_storage))
    }

    /// Checks if this set is a subset of an iterable.
    fn issubset_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_subset(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_subset(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
    }

    /// Checks if this set is a superset of an iterable.
    fn issuperset_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_superset(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_superset(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
    }

    /// Checks if this set has no elements in common with an iterable.
    fn isdisjoint_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_disjoint(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_disjoint(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
    }

    /// Helper to get SetStorage from a Value (either directly or by conversion).
    fn get_storage_from_value(
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<SetStorage> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match &value {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(set) => Some(set.0.copy_entries()),
                HeapData::FrozenSet(set) => Some(set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build storage with proper refcounts
            // IMPORTANT: Inc refs BEFORE dropping the source to avoid use-after-free
            SetStorage::inc_refs_for_entries(&entries, heap);
            value.drop_with_heap(heap);
            return Ok(SetStorage::from_entries(entries));
        }

        // Convert iterable to set
        let temp_set = Set::from_iterable(value, heap, interns)?;
        Ok(temp_set.0)
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
pub struct FrozenSet(SetStorage);

impl FrozenSet {
    /// Creates a new empty frozenset.
    #[must_use]
    pub fn new() -> Self {
        Self(SetStorage::new())
    }

    /// Creates a frozenset with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self(SetStorage::with_capacity(capacity))
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

    /// Returns a shallow copy of the frozenset.
    #[must_use]
    pub fn copy(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self(self.0.clone_with_heap(heap))
    }

    /// Creates a deep clone with proper reference counting.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self(self.0.clone_with_heap(heap))
    }

    /// Returns the internal storage.
    pub(crate) fn storage(&self) -> &SetStorage {
        &self.0
    }

    /// Computes the hash of this frozenset.
    ///
    /// The hash is the XOR of all element hashes, making it order-independent.
    pub fn compute_hash(&self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<u64> {
        let mut hash: u64 = 0;
        for entry in &self.0.entries {
            // All elements must be hashable (enforced at construction)
            let elem_hash = entry.value.py_hash(heap, interns)?;
            hash ^= elem_hash;
        }
        Some(hash)
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
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("frozenset")?;
        let frozenset = match value {
            None => Self::new(),
            Some(v) => Self::from_set(Set::from_iterable(v, heap, interns)?),
        };
        let heap_id = heap.allocate(HeapData::FrozenSet(frozenset))?;
        Ok(Value::Ref(heap_id))
    }

    /// Returns a new frozenset with elements from both this and another set.
    pub(crate) fn union(
        &self,
        other: &SetStorage,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        Ok(Self(self.0.union(other, heap, interns)?))
    }

    /// Returns a new frozenset with elements common to both sets.
    pub(crate) fn intersection(
        &self,
        other: &SetStorage,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        Ok(Self(self.0.intersection(other, heap, interns)?))
    }

    /// Returns a new frozenset with elements in this set but not in other.
    pub(crate) fn difference(
        &self,
        other: &SetStorage,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        Ok(Self(self.0.difference(other, heap, interns)?))
    }

    /// Returns a new frozenset with elements in either set but not both.
    pub(crate) fn symmetric_difference(
        &self,
        other: &SetStorage,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        Ok(Self(self.0.symmetric_difference(other, heap, interns)?))
    }
}

impl PyTrait for FrozenSet {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::FrozenSet
    }

    fn py_estimate_size(&self) -> usize {
        self.0.estimate_size()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        self.0.eq(&other.0, heap, interns)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.collect_dec_ref_ids(stack);
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        self.0.repr_fmt(f, heap, heap_ids, interns, "frozenset")
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(attr_id) = attr.string_id() else {
            return Err(ExcType::attribute_error(Type::FrozenSet, attr.as_str(interns)));
        };

        match attr_id {
            attr::COPY => {
                args.check_zero_args("frozenset.copy")?;
                let copy = self.copy(heap);
                let heap_id = heap.allocate(HeapData::FrozenSet(copy))?;
                Ok(Value::Ref(heap_id))
            }
            attr::UNION => {
                let other = args.get_one_arg("frozenset.union")?;
                let other_storage = Set::get_storage_from_value(other, heap, interns)?;
                let result = self.union(&other_storage, heap, interns)?;
                for entry in other_storage.entries {
                    entry.value.drop_with_heap(heap);
                }
                let heap_id = heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::INTERSECTION => {
                let other = args.get_one_arg("frozenset.intersection")?;
                let other_storage = Set::get_storage_from_value(other, heap, interns)?;
                let result = self.intersection(&other_storage, heap, interns)?;
                for entry in other_storage.entries {
                    entry.value.drop_with_heap(heap);
                }
                let heap_id = heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::DIFFERENCE => {
                let other = args.get_one_arg("frozenset.difference")?;
                let other_storage = Set::get_storage_from_value(other, heap, interns)?;
                let result = self.difference(&other_storage, heap, interns)?;
                for entry in other_storage.entries {
                    entry.value.drop_with_heap(heap);
                }
                let heap_id = heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::SYMMETRIC_DIFFERENCE => {
                let other = args.get_one_arg("frozenset.symmetric_difference")?;
                let other_storage = Set::get_storage_from_value(other, heap, interns)?;
                let result = self.symmetric_difference(&other_storage, heap, interns)?;
                for entry in other_storage.entries {
                    entry.value.drop_with_heap(heap);
                }
                let heap_id = heap.allocate(HeapData::FrozenSet(result))?;
                Ok(Value::Ref(heap_id))
            }
            attr::ISSUBSET => {
                let other = args.get_one_arg("frozenset.issubset")?;
                let result = self.issubset_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            attr::ISSUPERSET => {
                let other = args.get_one_arg("frozenset.issuperset")?;
                let result = self.issuperset_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            attr::ISDISJOINT => {
                let other = args.get_one_arg("frozenset.isdisjoint")?;
                let result = self.isdisjoint_from_value(&other, heap, interns);
                other.drop_with_heap(heap);
                Ok(Value::Bool(result?))
            }
            _ => Err(ExcType::attribute_error(Type::FrozenSet, attr.as_str(interns))),
        }
    }

    fn py_sub(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // Same limitation as Set - needs interns
        Ok(None)
    }
}

/// Helper methods for frozenset operations with arbitrary iterables.
impl FrozenSet {
    /// Checks if this frozenset is a subset of an iterable.
    fn issubset_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_subset(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_subset(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
    }

    /// Checks if this frozenset is a superset of an iterable.
    fn issuperset_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_superset(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_superset(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
    }

    /// Checks if this frozenset has no elements in common with an iterable.
    fn isdisjoint_from_value(
        &self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match other {
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Set(other_set) => Some(other_set.0.copy_entries()),
                HeapData::FrozenSet(other_set) => Some(other_set.0.copy_entries()),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            // Borrow released, build temporary storage and check
            SetStorage::inc_refs_for_entries(&entries, heap);
            let other_storage = SetStorage::from_entries(entries);
            let result = self.0.is_disjoint(&other_storage, heap, interns);
            other_storage.drop_all_values(heap);
            return result;
        }

        // Handle all other iterables (list, tuple, range, str, bytes, dict, etc.)
        let temp = Set::from_iterable(other.clone_with_heap(heap), heap, interns)?;
        let result = self.0.is_disjoint(&temp.0, heap, interns);
        temp.0.drop_all_values(heap);
        result
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
