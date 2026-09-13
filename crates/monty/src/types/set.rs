use std::{cell::Cell, fmt::Write, mem};

use ahash::AHashSet;
use hashbrown::HashTable;
use monty_types::{ResourceError, ResourceTracker};
use smallvec::SmallVec;

use super::{PyTrait, iter::checked_preallocation_hint};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::HashValue,
    heap::{
        BorrowedHeapRead, BorrowedHeapReadMut, ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem,
        HeapObjectRead, HeapRead, HeapReadOutput, heap_read_ref_as_field, heap_read_ref_as_field_mut,
    },
    identity::Identity,
    intern::StaticStrings,
    types::{
        LazyHeapSet, Type,
        dict::{ProbeOutcome, eq_is_native, probe_native_eq},
        list::repr_items_fmt,
    },
    value::{EitherStr, VALUE_SIZE, Value},
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

    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if the element was added (not already present),
    /// `Ok(false)` if the element was already in the set.
    /// Returns `Err` if the element is unhashable.
    ///
    /// The caller transfers ownership of `value`. If the value is already in
    /// the set, it will be dropped.
    fn add(&mut self, value: Value, vm: &mut VM<'_>) -> RunResult<bool> {
        let mut value_guard = DropGuard::new(value, vm);
        let (value, vm) = value_guard.as_parts_mut();
        let hash = set_element_hash(value, vm)?;

        // Check if value already exists. CPython compares the stored element on
        // the left, which an asymmetric user `__eq__` can tell apart — the
        // mutation-safe twin ([`HeapRead::find_index`]) does the same.
        let existing = self
            .indices
            .find(hash, |&idx| self.entries[idx].value.py_eq(value, vm).unwrap_or(false));

        if existing.is_some() {
            Ok(false)
        } else {
            let index = self.entries.len();
            let value = value_guard.into_inner();
            self.entries.push(SetEntry { value, hash });
            self.indices.insert_unique(hash, index, |&idx| self.entries[idx].hash);
            Ok(true)
        }
    }
}

impl<'h> HeapRead<'h, SetStorage> {
    /// Removes an element from the set.
    ///
    /// Returns `Ok(true)` if the element was removed, `Ok(false)` if not found.
    /// Returns `Err` if the key is unhashable.
    fn remove(&mut self, value: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let hash = set_element_hash(value, vm)?;

        let Some(index) = self.find_index(value, hash, vm)? else {
            return Ok(false);
        };

        // Remove via short-lived mutable borrow
        let storage = self.get_mut(vm.heap);
        let removed_entry = storage.entries.remove(index);
        storage.indices.clear();
        for (idx, e) in storage.entries.iter().enumerate() {
            storage.indices.insert_unique(e.hash, idx, |&i| storage.entries[i].hash);
        }

        removed_entry.value.drop_with(vm);
        Ok(true)
    }

    /// Removes an element from the set without raising an error if not found.
    ///
    /// Returns `Ok(())` always (unless the key is unhashable).
    fn discard(&mut self, value: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        self.remove(value, vm)?;
        Ok(())
    }

    /// Removes and returns an arbitrary element from the set.
    ///
    /// Returns `Err(KeyError)` if the set is empty.
    fn pop(&mut self, vm: &mut VM<'h>) -> RunResult<Value> {
        if self.get(vm.heap).is_empty() {
            return Err(ExcType::key_error_pop_empty_set());
        }

        // Remove the last entry (most efficient)
        let storage = self.get_mut(vm.heap);
        let entry = storage.entries.pop().expect("checked non-empty");

        // Remove from hash table
        storage
            .indices
            .find_entry(entry.hash, |&idx| idx == storage.entries.len())
            .expect("entry must exist")
            .remove();

        Ok(entry.value)
    }

    /// Removes all elements from the set.
    fn clear(&mut self, vm: &mut VM<'h>) {
        let entries = mem::take(&mut self.get_mut(vm.heap).entries);
        self.get_mut(vm.heap).indices.clear();
        entries.drop_with(vm);
    }
}

impl SetStorage {
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
}

impl<'h> HeapRead<'h, SetStorage> {
    /// Checks if the set contains a value.
    pub fn contains(&self, value: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let hash = set_element_hash(value, vm)?;
        Ok(self.find_index(value, hash, vm)?.is_some())
    }

    /// Finds the index of the entry equal to `value`, or `None` if absent.
    ///
    /// Snapshots candidates so each can be validated around user `__eq__`
    /// without holding a borrow of the hash table; only one that moved restarts
    /// the probe. The twin of [`HeapRead::<Dict>::find_index_hash`], which
    /// carries the full reasoning — the two must stay in sync.
    fn find_index(&self, value: &Value, hash: u64, vm: &mut VM<'h>) -> RunResult<Option<usize>> {
        // When no comparison can dispatch to user code, nothing can mutate the
        // set mid-probe: skip revalidation and the miss continuation, as in
        // the dict twin.
        let value_native = eq_is_native(value, vm.heap);

        'restart: loop {
            // Collected inline rather than through `probe_candidates`, for the
            // reason given in the dict twin.
            let mut candidate_indices: SmallVec<[usize; 2]> = SmallVec::new();
            let mut candidate_values: SmallVec<[Value; 2]> = SmallVec::new();
            let mut all_native = value_native;
            let storage = self.get(vm.heap);
            // Native pairs are compared inline during the probe walk; only
            // pairs that may need user code are cloned for the guarded loop —
            // see the dict twin.
            let found = storage
                .indices
                .find(hash, |&idx| {
                    let entry = &storage.entries[idx];
                    if entry.hash != hash {
                        return false;
                    }
                    if candidate_indices.is_empty()
                        && let Some(eq) = probe_native_eq(&entry.value, value, vm)
                    {
                        eq
                    } else {
                        candidate_indices.push(idx);
                        candidate_values.push(entry.value.clone_with_heap(vm.heap));
                        all_native = all_native && eq_is_native(&entry.value, vm.heap);
                        false
                    }
                })
                .copied();
            // Guarded before the early returns — see the dict twin.
            defer_drop!(candidate_values, vm);
            if let Some(index) = found {
                return Ok(Some(index));
            }
            if candidate_indices.is_empty() {
                return Ok(None);
            }

            for (&candidate_index, candidate_value) in candidate_indices.iter().zip(candidate_values.iter()) {
                if !all_native && !self.probe_valid(candidate_index, hash, candidate_value, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                // CPython compares the stored value on the left.
                let eq = candidate_value.py_eq(value, vm)?;
                if !all_native && !self.probe_valid(candidate_index, hash, candidate_value, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                if eq {
                    return Ok(Some(candidate_index));
                }
            }

            // A comparison can itself have added a colliding value the snapshot
            // never saw, so a pass that ran any hands over to the mutation-aware
            // continuation — unless none could run user code.
            if all_native {
                return Ok(None);
            }
            match self.probe_after_compare(hash, value, candidate_values, vm)? {
                ProbeOutcome::Found(index) => return Ok(Some(index)),
                ProbeOutcome::Missing => return Ok(None),
                // a candidate moved: fall through to the next probe from scratch
                ProbeOutcome::Restart => (),
            }
        }
    }

    /// Continues a probe whose comparisons all missed, in case one of them
    /// mutated the set and added a colliding value.
    ///
    /// The twin of [`HeapRead::<Dict>::probe_after_compare`]: re-reads the
    /// candidates until a pass finds nothing new, never re-running user
    /// `__eq__` on a value. Inline-compared native pairs may repeat — see there.
    fn probe_after_compare(
        &self,
        hash: u64,
        value: &Value,
        already_compared: &[Value],
        vm: &mut VM<'h>,
    ) -> RunResult<ProbeOutcome> {
        // The clones keep every compared value alive so its heap slot cannot
        // be recycled into a new value that would then be skipped by identity.
        let compared: SmallVec<[Value; 2]> = already_compared.iter().map(|v| v.clone_with_heap(vm.heap)).collect();
        defer_drop_mut!(compared, vm);
        // Identity set for O(1) seen-checks; see the dict twin for why the
        // linear alternative is quadratic.
        let mut compared_ids: AHashSet<Identity> = compared.iter().map(Value::id).collect();

        loop {
            // Polled up front so every entry checks the limits at least once —
            // see the dict twin.
            vm.heap.tracker.check_memory_time()?;
            let (candidate_indices, candidate_values) = self.probe_candidates(hash, vm);
            defer_drop!(candidate_values, vm);
            let mut compared_any = false;

            for (&candidate_index, candidate_value) in candidate_indices.iter().zip(candidate_values.iter()) {
                if !compared_ids.insert(candidate_value.id()) {
                    continue;
                }
                if !self.probe_valid(candidate_index, hash, candidate_value, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    return Ok(ProbeOutcome::Restart);
                }
                compared.push(candidate_value.clone_with_heap(vm.heap));
                compared_any = true;
                // CPython compares the stored value on the left.
                let eq = candidate_value.py_eq(value, vm)?;
                if !self.probe_valid(candidate_index, hash, candidate_value, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    return Ok(ProbeOutcome::Restart);
                }
                if eq {
                    return Ok(ProbeOutcome::Found(candidate_index));
                }
            }

            if !compared_any {
                return Ok(ProbeOutcome::Missing);
            }
        }
    }

    /// Snapshots the live entries colliding on `hash`: their indices, plus an
    /// owned reference to each of their values.
    ///
    /// Cloning the values lets `py_eq` run without the hash-table borrow held;
    /// the caller owns the returned values and must drop them. Only the
    /// mutation-aware continuation calls this — the fast path above inlines the
    /// same collection.
    fn probe_candidates(&self, hash: u64, vm: &VM<'h>) -> (SmallVec<[usize; 2]>, SmallVec<[Value; 2]>) {
        let mut indices: SmallVec<[usize; 2]> = SmallVec::new();
        let mut values: SmallVec<[Value; 2]> = SmallVec::new();
        let storage = self.get(vm.heap);
        storage.indices.find(hash, |&idx| {
            if storage.entries[idx].hash == hash {
                indices.push(idx);
                values.push(storage.entries[idx].value.clone_with_heap(vm.heap));
            }
            false
        });
        (indices, values)
    }

    /// Checks that a snapshotted candidate still names the same live entry.
    ///
    /// The caller checks before and after `py_eq`; a mismatch restarts the probe.
    #[inline]
    fn probe_valid(&self, index: usize, hash: u64, value: &Value, vm: &VM<'h>) -> bool {
        let storage = self.get(vm.heap);
        storage
            .entries
            .get(index)
            .is_some_and(|entry| entry.hash == hash && entry.value.is(value))
    }
}

impl SetStorage {
    /// Returns an iterator over the values in the set.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|e| &e.value)
    }

    /// Returns the value at the given index, if valid.
    ///
    /// Used by Python iterator objects for index-based iteration.
    pub(crate) fn value_at(&self, index: usize) -> Option<&Value> {
        self.entries.get(index).map(|e| &e.value)
    }

    /// Collects heap IDs for reference counting cleanup.
    fn collect_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for entry in &mut self.entries {
            if let Value::Ref(id) = &entry.value {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                entry.value.dec_ref_forget();
            }
        }
    }
}

impl<'h> HeapRead<'h, SetStorage> {
    /// Compares two sets for equality.
    fn eq(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<bool> {
        if self.get(vm.heap).len() != other.get(vm.heap).len() {
            return Ok(false);
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(elem) = iter.next(vm)? {
            if !other.contains(elem, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns a stack-borrowed lending iterator over the set's elements in
    /// insertion order, holding a recursion-depth token for its lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`SetIter`]) because that's the obvious entry point for "iterate
    /// this container".
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter(&self, vm: &mut VM<'h>) -> RunResult<SetIter<'_, 'h>> {
        SetIter::new(self, vm)
    }
}

/// Stack-borrowed lending iterator over a heap-allocated set's elements in
/// insertion order.
///
/// Borrows a [`HeapRead`] for its lifetime, so the heap entry is pinned by
/// the reader count for the duration of iteration.
///
/// **Lending shape.** [`next`](Self::next) returns `Option<&Value>`. The
/// iterator itself owns the most-recently-yielded element (using
/// [`Value::Undefined`] as the empty sentinel) and drops the previous
/// element at the start of each `next` call, so call sites do **not** need
/// a per-item `defer_drop!`.
///
/// **Recursion guard.** Acquires a [`RecursionToken`] at construction and
/// releases it via [`DropWithContext`]. The iterator MUST be wrapped in
/// [`defer_drop_mut!`] so the token (and any in-flight element) is released
/// on every exit path — set iteration usually feeds into `py_eq` /
/// `py_hash` / membership checks which recurse on cyclic structures (e.g.
/// frozensets of frozensets).
///
/// **Mutation policy.** The initial length is captured at construction. If
/// the set's size changes between [`next`](Self::next) calls, the next step
/// returns `RuntimeError: Set changed size during iteration` (matching
/// CPython and Monty's set-iterator behavior).
pub(crate) struct SetIter<'a, 'h> {
    storage: &'a HeapRead<'h, SetStorage>,
    index: usize,
    expected_len: usize,
    token: RecursionToken,
    /// Most-recently-yielded element. `Value::Undefined` when nothing is
    /// held — drops on that variant are no-ops, so `next` can
    /// unconditionally release the previous slot before fetching the next.
    current: Value,
}

impl<'a, 'h> SetIter<'a, 'h> {
    fn new(storage: &'a HeapRead<'h, SetStorage>, vm: &mut VM<'h>) -> RunResult<Self> {
        let expected_len = storage.get(vm.heap).entries.len();
        let token = vm.recursion_token()?;
        Ok(Self {
            storage,
            index: 0,
            expected_len,
            token,
            current: Value::Undefined,
        })
    }

    /// Advances the iterator and returns a borrow of the next element, or
    /// `Ok(None)` on exhaustion. The returned reference is valid until the
    /// next call to `next` (or until the iterator is dropped).
    ///
    /// Returns `Err(RuntimeError)` if the set's size has changed since
    /// construction.
    pub(crate) fn next<'i>(&'i mut self, vm: &mut VM<'h>) -> RunResult<Option<&'i Value>> {
        // Drop the previously-yielded element (no-op when `current` is `Undefined`).
        mem::replace(&mut self.current, Value::Undefined).drop_with(vm.heap);
        vm.heap.tracker.check_time_every(self.index)?;
        let current = self.storage.get(vm.heap);
        if current.entries.len() != self.expected_len {
            return Err(ExcType::runtime_error_set_changed_size());
        }
        if self.index >= self.expected_len {
            return Ok(None);
        }
        self.current = current.entries[self.index].value.clone_with_heap(vm.heap);
        self.index += 1;
        Ok(Some(&self.current))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for SetIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

impl SetStorage {
    /// Returns true if this set is a subset of other.
    fn is_subset(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        for entry in &self.entries {
            if !vm.heap.protect(other).contains(&entry.value, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns true if this set is a superset of other.
    fn is_superset(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        other.is_subset(self, vm)
    }

    /// Returns true if this set has no elements in common with other.
    fn is_disjoint(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        // Iterate over the smaller set for efficiency
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };

        for entry in &smaller.entries {
            if vm.heap.protect(larger).contains(&entry.value, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl<'h> HeapRead<'h, SetStorage> {
    /// Returns a new set containing elements in either set (union).
    fn union(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<SetStorage> {
        let mut result_guard = DropGuard::new(self.get(vm.heap).clone_with_heap(vm), vm);
        let (result, vm) = result_guard.as_parts_mut();
        let len = other.get(vm.heap).len();
        for idx in 0..len {
            let value = other.get(vm.heap).entries[idx].value.clone_with_heap(vm);
            result.add(value, vm)?;
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in both sets (intersection).
    fn intersection(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<SetStorage> {
        let mut result_guard = DropGuard::new(SetStorage::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();
        // Iterate over the smaller set for efficiency. CPython swaps only when
        // `other` is strictly larger, so on a tie it walks `other` and tests
        // membership in `self` — observable through an asymmetric `__eq__`.
        let (smaller, larger) = if self.get(vm.heap).len() < other.get(vm.heap).len() {
            (self, other)
        } else {
            (other, self)
        };

        let len = smaller.get(vm.heap).len();
        for idx in 0..len {
            let value = smaller.get(vm.heap).entries[idx].value.clone_with_heap(vm);
            let mut value_guard = DropGuard::new(value, vm);
            let (value, vm) = value_guard.as_parts_mut();
            if larger.contains(value, vm)? {
                let (value, vm) = value_guard.into_parts();
                result.add(value, vm)?;
            }
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in self but not in other (difference).
    fn difference(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<SetStorage> {
        let mut result_guard = DropGuard::new(SetStorage::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();
        let len = self.get(vm.heap).len();
        for idx in 0..len {
            let value = self.get(vm.heap).entries[idx].value.clone_with_heap(vm);
            let mut value_guard = DropGuard::new(value, vm);
            let (value, vm) = value_guard.as_parts_mut();
            if !other.contains(value, vm)? {
                let (value, vm) = value_guard.into_parts();
                result.add(value, vm)?;
            }
        }
        Ok(result_guard.into_inner())
    }

    /// Returns a new set containing elements in either set but not both (symmetric difference).
    fn symmetric_difference(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<SetStorage> {
        let mut result_guard = DropGuard::new(SetStorage::new(), vm);
        let (result, vm) = result_guard.as_parts_mut();

        // Add elements in self but not in other
        let len = self.get(vm.heap).len();
        for idx in 0..len {
            let value = self.get(vm.heap).entries[idx].value.clone_with_heap(vm);
            let mut value_guard = DropGuard::new(value, vm);
            let (value, vm) = value_guard.as_parts_mut();
            if !other.contains(value, vm)? {
                let (value, vm) = value_guard.into_parts();
                result.add(value, vm)?;
            }
        }

        // Add elements in other but not in self
        let len = other.get(vm.heap).len();
        for idx in 0..len {
            let value = other.get(vm.heap).entries[idx].value.clone_with_heap(vm);
            let mut value_guard = DropGuard::new(value, vm);
            let (value, vm) = value_guard.as_parts_mut();
            if !self.contains(value, vm)? {
                let (value, vm) = value_guard.into_parts();
                result.add(value, vm)?;
            }
        }

        Ok(result_guard.into_inner())
    }
}

impl<'h> HeapRead<'h, SetStorage> {
    /// Writes the repr format to a formatter.
    fn repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h>,
        heap_ids: &mut LazyHeapSet,
        type_name: &str,
    ) -> RunResult<()> {
        let len = self.get(vm.heap).len();
        if len == 0 {
            return Ok(write!(f, "{type_name}()")?);
        }

        // Check depth limit before recursing
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("{...}")?);
        };
        let vm = &mut *guard;

        // frozenset needs type prefix: frozenset({...}), but set doesn't: {...}
        let needs_prefix = type_name != "set";
        if needs_prefix {
            write!(f, "{type_name}(")?;
        }

        // Format a refcount-bumped snapshot of the elements: CPython's set repr
        // copies to a list first, so a user `__repr__` mutating the set
        // mid-format changes nothing (and can't invalidate indices here).
        vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
        let items = Vec::with_capacity(len);
        defer_drop_mut!(items, vm);
        for i in 0..len {
            // The whole snapshot runs before `repr_items_fmt` reaches its first
            // checkpoint, so it polls the deadline itself — otherwise a big set
            // overshoots the time limit by the entire copy.
            vm.heap.tracker.check_time_every(i)?;
            // No user code runs during the snapshot, so `len` is still current.
            let value = self.get(vm.heap).value_at(i).expect("index in range");
            items.push(value.clone_with_heap(vm.heap));
        }

        f.write_char('{')?;
        repr_items_fmt(items, f, vm, heap_ids)?;
        f.write_char('}')?;

        if needs_prefix {
            f.write_char(')')?;
        }

        Ok(())
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

    /// Validates and clamps a capacity before native set preallocation.
    pub(crate) fn preallocation_capacity(requested: usize, tracker: &ResourceTracker) -> Result<usize, ResourceError> {
        checked_preallocation_hint(requested, mem::size_of::<SetEntry>(), tracker)
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
    pub fn add(&mut self, value: Value, vm: &mut VM<'_>) -> RunResult<bool> {
        self.0.add(value, vm)
    }
}

impl<'h> HeapRead<'h, Set> {
    /// Removes an element from the set.
    ///
    /// Returns `Err(KeyError)` if the element is not present.
    pub fn remove(&mut self, value: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        if self.storage_mut().remove(value, vm)? {
            Ok(())
        } else {
            Err(ExcType::key_error(value, vm))
        }
    }

    /// Removes an element from the set if present.
    ///
    /// Does not raise an error if the element is not found.
    pub fn discard(&mut self, value: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        self.storage_mut().discard(value, vm)
    }

    /// Removes and returns an arbitrary element from the set.
    ///
    /// Returns `Err(KeyError)` if the set is empty.
    pub fn pop(&mut self, vm: &mut VM<'h>) -> RunResult<Value> {
        self.storage_mut().pop(vm)
    }

    /// Removes all elements from the set.
    pub fn clear(&mut self, vm: &mut VM<'h>) {
        self.storage_mut().clear(vm);
    }

    /// Returns a shallow copy of the set.
    #[must_use]
    pub fn copy(&self, vm: &VM<'h>) -> Set {
        Set(self.get(vm.heap).0.clone_with_heap(vm.heap))
    }

    fn storage(&self) -> BorrowedHeapRead<'_, 'h, SetStorage> {
        heap_read_ref_as_field!(self, Set, 0)
    }

    fn storage_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, SetStorage> {
        heap_read_ref_as_field_mut!(self, Set, 0)
    }
}

impl Set {
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
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("set", vm.heap)?;
        let set = match value {
            None => Self::new(),
            Some(v) => Self::from_iterable(v, vm)?,
        };
        let heap_id = vm.heap.allocate(HeapData::Set(set));
        Ok(Value::Ref(heap_id))
    }

    /// Creates a set from an iterable value, adding and hashing items incrementally.
    fn from_iterable(iterable: Value, vm: &mut VM<'_>) -> RunResult<Self> {
        let iterator = iterable.into_py_iter(vm)?;
        defer_drop!(iterator, vm);
        let mut iterator = iterator.read(vm);
        let hint = iterator.iter_size_hint(vm);
        let capacity = checked_preallocation_hint(hint, mem::size_of::<SetEntry>(), &vm.heap.tracker)?;
        let mut set_guard = DropGuard::new(Self::with_capacity(capacity), vm);
        let (set, vm) = set_guard.as_parts_mut();
        while let Some(item) = iterator.py_next(vm)? {
            set.add(item, vm)?;
        }
        Ok(set_guard.into_inner())
    }
}

impl<'h> HeapRead<'h, Set> {
    /// Adds an element to the set, transferring ownership.
    ///
    /// Returns `Ok(true)` if the element was added (not already present),
    /// `Ok(false)` if the element was already in the set (and the value is dropped).
    /// Returns `Err` if the element is unhashable (and the value is dropped).
    ///
    /// Uses the mutation-safe storage lookup ([`HeapRead::find_index`]) to
    /// detect duplicates, so a user `__eq__` mutating the set mid-add cannot
    /// leave the probe holding stale indices.
    pub fn add(&mut self, value: Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let mut value_guard = DropGuard::new(value, vm);
        let (value, vm) = value_guard.as_parts();
        let hash = set_element_hash(value, vm)?;

        if self.storage().find_index(value, hash, vm)?.is_some() {
            return Ok(false);
        }

        // Add new entry
        let (value, vm) = value_guard.into_parts();
        let storage = &mut self.get_mut(vm.heap).0;
        let index = storage.entries.len();
        storage.entries.push(SetEntry { value, hash });
        storage
            .indices
            .insert_unique(hash, index, |&idx| storage.entries[idx].hash);
        Ok(true)
    }

    pub(crate) fn contains(&self, value: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        self.storage().contains(value, vm)
    }

    /// `set.update(iterable)` via HeapRead.
    fn hr_update(&mut self, other: Value, vm: &mut VM<'h>) -> RunResult<()> {
        // Try direct extraction from Set/FrozenSet
        let values_opt = {
            match &other {
                Value::Ref(id) => match vm.heap.get(*id) {
                    HeapData::Set(s) => Some(&s.0),
                    HeapData::FrozenSet(fs) => Some(&fs.storage),
                    _ => None,
                },
                _ => None,
            }
        }
        .map(|storage| {
            storage
                .iter()
                .map(|value| value.clone_with_heap(vm.heap))
                .collect::<Vec<_>>()
        });

        if let Some(values) = values_opt {
            other.drop_with(vm);
            let values = values.into_iter();
            defer_drop_mut!(values, vm);
            for value in values {
                self.add(value, vm)?;
            }
            return Ok(());
        }

        // Fall back to iterable
        let temp_set = Set::from_iterable(other, vm)?;
        let entries = temp_set.0.entries.into_iter();
        defer_drop_mut!(entries, vm);
        for entry in entries {
            self.add(entry.value, vm)?;
        }
        Ok(())
    }

    /// Set algebra operations (union, intersection, difference, symmetric_difference)
    /// via HeapRead. Clones self's storage once, then calls the existing `SetStorage`
    /// methods on the standalone copy.
    fn set_algebra(&self, other: Value, op: SetAlgebra, vm: &mut VM<'h>) -> RunResult<Value> {
        let other_storage = Set::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);

        let result = match op {
            SetAlgebra::Union => self.storage().union(&other_storage, vm)?,
            SetAlgebra::Intersection => self.storage().intersection(&other_storage, vm)?,
            SetAlgebra::Difference => self.storage().difference(&other_storage, vm)?,
            SetAlgebra::SymmetricDifference => self.storage().symmetric_difference(&other_storage, vm)?,
        };

        let heap_id = vm.heap.allocate(HeapData::Set(Set(result)));
        Ok(Value::Ref(heap_id))
    }

    /// Set comparison operations (issubset, issuperset, isdisjoint) via HeapRead.
    /// Clones self's storage once for the comparison.
    fn comparison_op(&self, other: &Value, op: SetComparison, vm: &mut VM<'h>) -> RunResult<bool> {
        // Get other's storage
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(s) => Some(s.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(fs) => Some(fs.storage.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        let other_storage = if let Some(entries) = entries_opt {
            SetStorage::from_entries(entries)
        } else {
            let temp = Set::from_iterable(other.clone_with_heap(vm), vm)?;
            temp.0
        };
        defer_drop!(other_storage, vm);

        let self_storage = self.get(vm.heap).0.clone_with_heap(vm.heap);
        defer_drop!(self_storage, vm);

        match op {
            SetComparison::Subset => self_storage.is_subset(other_storage, vm),
            SetComparison::Superset => self_storage.is_superset(other_storage, vm),
            SetComparison::Disjoint => self_storage.is_disjoint(other_storage, vm),
        }
    }
}

/// Which set algebra operation to perform.
#[derive(Debug, Clone, Copy)]
enum SetAlgebra {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// Which set comparison operation to perform.
#[derive(Debug, Clone, Copy)]
enum SetComparison {
    Subset,
    Superset,
    Disjoint,
}

impl<C: ContainsHeap> DropWithContext<C> for Set {
    fn drop_with(self, heap: &mut C) {
        self.0.drop_with(heap);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for SetStorage {
    fn drop_with(self, heap: &mut C) {
        self.entries.drop_with(heap);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FrozenSet {
    fn drop_with(self, heap: &mut C) {
        self.storage.drop_with(heap);
    }
}

impl<'h> HeapRead<'h, FrozenSet> {
    /// Checks if the frozenset contains a value, using the candidate collection pattern
    /// to avoid holding a borrow on the storage during `py_eq` calls.
    pub(crate) fn contains(&self, value: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        self.storage().contains(value, vm)
    }

    /// Binary set operation via HeapRead. Creates a new frozenset from the result.
    ///
    /// Clones self's storage entries to release the heap borrow before calling
    /// the set operations (which need `&mut VM` for hashing and equality checks).
    /// Applies set sub when the other value is a set operand.
    fn sub_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<FrozenSet>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(FrozenSet::wrap(self.storage().difference(&other_storage, vm)?)))
    }

    /// Applies set and when the other value is a set operand.
    fn and_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<FrozenSet>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(FrozenSet::wrap(self.storage().intersection(&other_storage, vm)?)))
    }

    /// Applies set or when the other value is a set operand.
    fn or_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<FrozenSet>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(FrozenSet::wrap(self.storage().union(&other_storage, vm)?)))
    }

    /// Applies set xor when the other value is a set operand.
    fn xor_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<FrozenSet>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(FrozenSet::wrap(
            self.storage().symmetric_difference(&other_storage, vm)?,
        )))
    }

    /// Set algebra operations for frozenset via HeapRead.
    fn set_algebra(&self, other: Value, op: SetAlgebra, vm: &mut VM<'h>) -> RunResult<Value> {
        let other_storage = Set::get_storage_from_value(other, vm)?;
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);

        let result = match op {
            SetAlgebra::Union => self.storage().union(&other_storage, vm)?,
            SetAlgebra::Intersection => self.storage().intersection(&other_storage, vm)?,
            SetAlgebra::Difference => self.storage().difference(&other_storage, vm)?,
            SetAlgebra::SymmetricDifference => self.storage().symmetric_difference(&other_storage, vm)?,
        };

        let heap_id = vm.heap.allocate(HeapData::FrozenSet(FrozenSet::wrap(result)));
        Ok(Value::Ref(heap_id))
    }

    /// Set comparison operations for frozenset via HeapRead.
    fn comparison_op(&self, other: &Value, op: SetComparison, vm: &mut VM<'h>) -> RunResult<bool> {
        let entries_opt = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(s) => Some(s.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(fs) => Some(fs.storage.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        let other_storage = if let Some(entries) = entries_opt {
            SetStorage::from_entries(entries)
        } else {
            let temp = Set::from_iterable(other.clone_with_heap(vm), vm)?;
            temp.0
        };
        defer_drop!(other_storage, vm);

        let self_storage = self.get(vm.heap).storage.clone_with_heap(vm.heap);
        defer_drop!(self_storage, vm);

        match op {
            SetComparison::Subset => self_storage.is_subset(other_storage, vm),
            SetComparison::Superset => self_storage.is_superset(other_storage, vm),
            SetComparison::Disjoint => self_storage.is_disjoint(other_storage, vm),
        }
    }

    fn storage(&self) -> BorrowedHeapRead<'_, 'h, SetStorage> {
        heap_read_ref_as_field!(self, FrozenSet, storage)
    }
}

impl<C: ContainsHeap> DropWithContext<C> for SetEntry {
    fn drop_with(self, heap: &mut C) {
        self.value.drop_with(heap);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Set> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.contains(item, vm).map(Some)
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Set
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(SetIterator::from_set(self.id(), self.get(vm.heap).len(), vm))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // `set` and `frozenset` compare equal by their members, regardless of
        // mutability. `set == dict_keys`/`dict_items` is handled by the reflected
        // pass via the dict-view impls.
        match other.read_heap(vm) {
            Some(HeapReadOutput::Set(other)) => Ok(Some(self.storage().eq(&other.storage(), vm)?)),
            Some(HeapReadOutput::FrozenSet(other)) => Ok(Some(self.storage().eq(&other.storage(), vm)?)),
            _ => Ok(None),
        }
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).is_empty())
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.sub_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::Set(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.and_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::Set(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.or_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::Set(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.xor_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::Set(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        self.storage().repr_fmt(f, vm, heap_ids, "set")
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
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
                self.pop(vm)
            }
            Some(StaticStrings::Clear) => {
                args.check_zero_args("set.clear", vm.heap)?;
                self.clear(vm);
                Ok(Value::None)
            }
            Some(StaticStrings::Copy) => {
                args.check_zero_args("set.copy", vm.heap)?;
                let copy = self.copy(vm);
                let heap_id = vm.heap.allocate(HeapData::Set(copy));
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Update) => {
                let other = args.get_one_arg("set.update", vm.heap)?;
                self.hr_update(other, vm)?;
                Ok(Value::None)
            }
            Some(StaticStrings::Union) => {
                let other = args.get_one_arg("set.union", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Union, vm)
            }
            Some(StaticStrings::Intersection) => {
                let other = args.get_one_arg("set.intersection", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Intersection, vm)
            }
            Some(StaticStrings::Difference) => {
                let other = args.get_one_arg("set.difference", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Difference, vm)
            }
            Some(StaticStrings::SymmetricDifference) => {
                let other = args.get_one_arg("set.symmetric_difference", vm.heap)?;
                self.set_algebra(other, SetAlgebra::SymmetricDifference, vm)
            }
            Some(StaticStrings::Issubset) => {
                let other = args.get_one_arg("set.issubset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Subset, vm)?))
            }
            Some(StaticStrings::Issuperset) => {
                let other = args.get_one_arg("set.issuperset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Superset, vm)?))
            }
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("set.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Disjoint, vm)?))
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::Set, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }
}

/// Helper methods for set operations with arbitrary iterables.
impl<'h> HeapRead<'h, Set> {
    /// Implements operator-form set algebra, which only accepts set/frozenset operands.
    ///
    /// Unlike method forms such as `set.union(iterable)`, the binary operators
    /// `& | ^ -` are intentionally strict and return `None` for operands outside
    /// the set-like values CPython accepts here (`set`, `frozenset`,
    /// `dict_keys`, and `dict_items`) so the VM can raise the standard
    /// unsupported-operands `TypeError`.
    /// Applies set sub when the other value is a set operand.
    fn sub_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Set>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(Set(self.storage().difference(&other_storage, vm)?)))
    }

    /// Applies set and when the other value is a set operand.
    fn and_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Set>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(Set(self.storage().intersection(&other_storage, vm)?)))
    }

    /// Applies set or when the other value is a set operand.
    fn or_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Set>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(Set(self.storage().union(&other_storage, vm)?)))
    }

    /// Applies set xor when the other value is a set operand.
    fn xor_value(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Set>> {
        let Some(other_storage) = get_storage_from_set_operand(other, vm)? else {
            return Ok(None);
        };
        defer_drop!(other_storage, vm);
        let other_storage = vm.heap.protect(other_storage);
        Ok(Some(Set(self.storage().symmetric_difference(&other_storage, vm)?)))
    }
}

impl Set {
    /// Helper to get SetStorage from a Value (either directly or by conversion).
    fn get_storage_from_value(value: Value, vm: &mut VM<'_>) -> RunResult<SetStorage> {
        // Try to get entries from a Set/FrozenSet directly
        let entries_opt = match &value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Set(set) => Some(set.0.clone_entries(vm.heap)),
                HeapData::FrozenSet(set) => Some(set.storage.clone_entries(vm.heap)),
                _ => None,
            },
            _ => None,
        };

        if let Some(entries) = entries_opt {
            value.drop_with(vm);
            return Ok(SetStorage::from_entries(entries));
        }

        // Convert iterable to set
        let temp_set = Self::from_iterable(value, vm)?;
        Ok(temp_set.0)
    }
}

impl HeapItem for Set {
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
pub(crate) struct FrozenSet {
    storage: SetStorage,
    /// Lazily-computed Python hash.
    cached_hash: Cell<Option<HashValue>>,
}

impl FrozenSet {
    /// Wraps an existing `SetStorage` as a frozenset.
    ///
    /// The freshly-wrapped frozenset starts with an empty hash cache; the
    /// hash is computed lazily on first `py_hash` call. Inherited cache
    /// state is *not* propagated from a source frozenset — each instance
    /// manages its own cache.
    #[must_use]
    pub fn wrap(storage: SetStorage) -> Self {
        Self {
            storage,
            cached_hash: Cell::new(None),
        }
    }

    /// Creates a new empty frozenset.
    #[must_use]
    pub fn new() -> Self {
        Self::wrap(SetStorage::new())
    }

    /// Returns the number of elements in the frozenset.
    #[must_use]
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Returns true if the frozenset is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Returns the internal storage.
    pub(crate) fn storage(&self) -> &SetStorage {
        &self.storage
    }
}

impl FrozenSet {
    /// Creates a frozenset from a Set, consuming the Set's storage.
    ///
    /// This is used when we need to convert a mutable set to an immutable frozenset
    /// without cloning.
    pub fn from_set(set: Set) -> Self {
        Self::wrap(set.0)
    }

    /// Creates a frozenset from the `frozenset()` constructor call.
    ///
    /// - `frozenset()` with no args returns an empty frozenset
    /// - `frozenset(iterable)` creates a frozenset from any iterable (list, tuple, set, dict, range, str, bytes)
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("frozenset", vm.heap)?;
        let frozenset = match value {
            None => Self::new(),
            Some(v) => Self::from_set(Set::from_iterable(v, vm)?),
        };
        let heap_id = vm.heap.allocate(HeapData::FrozenSet(frozenset));
        Ok(Value::Ref(heap_id))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, FrozenSet> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.contains(item, vm).map(Some)
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::FrozenSet
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(SetIterator::from_frozen_set(self.id(), self.get(vm.heap).len(), vm))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // `frozenset` and `set` compare equal by their members, regardless of
        // mutability. `frozenset == dict_keys`/`dict_items` is handled by the
        // reflected pass via the dict-view impls.
        match other.read_heap(vm) {
            Some(HeapReadOutput::FrozenSet(other)) => Ok(Some(self.storage().eq(&other.storage(), vm)?)),
            Some(HeapReadOutput::Set(other)) => Ok(Some(self.storage().eq(&other.storage(), vm)?)),
            _ => Ok(None),
        }
    }

    /// Hashes the frozenset by XORing all element hashes.
    ///
    /// XOR is commutative, so the hash is independent of insertion order — two
    /// frozensets with the same members hash equally regardless of how they were built.
    /// Caches the computed hash on first call (frozensets are immutable).
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        if let Some(cached) = self.get(vm.heap).cached_hash.get() {
            return Ok(Some(cached));
        }
        let mut hash: u64 = 0;
        let storage = self.storage();
        let iter = storage.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(item) = iter.next(vm)? {
            hash ^= set_element_hash(item, vm)?;
        }
        let hash = HashValue::new(hash);
        self.get(vm.heap).cached_hash.set(Some(hash));
        Ok(Some(hash))
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).is_empty())
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.sub_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::FrozenSet(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.and_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::FrozenSet(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.or_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::FrozenSet(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(result) = self.xor_value(other, vm)? else {
            return Ok(None);
        };
        let result_id = vm.heap.allocate(HeapData::FrozenSet(result));
        Ok(Some(Value::Ref(result_id)))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        self.storage().repr_fmt(f, vm, heap_ids, "frozenset")
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let value = match attr.static_string() {
            Some(StaticStrings::Copy) => {
                args.check_zero_args("frozenset.copy", vm.heap)?;
                let cloned = self.get(vm.heap).storage.clone_with_heap(vm.heap);
                let heap_id = vm.heap.allocate(HeapData::FrozenSet(FrozenSet::wrap(cloned)));
                Ok(Value::Ref(heap_id))
            }
            Some(StaticStrings::Union) => {
                let other = args.get_one_arg("frozenset.union", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Union, vm)
            }
            Some(StaticStrings::Intersection) => {
                let other = args.get_one_arg("frozenset.intersection", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Intersection, vm)
            }
            Some(StaticStrings::Difference) => {
                let other = args.get_one_arg("frozenset.difference", vm.heap)?;
                self.set_algebra(other, SetAlgebra::Difference, vm)
            }
            Some(StaticStrings::SymmetricDifference) => {
                let other = args.get_one_arg("frozenset.symmetric_difference", vm.heap)?;
                self.set_algebra(other, SetAlgebra::SymmetricDifference, vm)
            }
            Some(StaticStrings::Issubset) => {
                let other = args.get_one_arg("frozenset.issubset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Subset, vm)?))
            }
            Some(StaticStrings::Issuperset) => {
                let other = args.get_one_arg("frozenset.issuperset", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Superset, vm)?))
            }
            Some(StaticStrings::Isdisjoint) => {
                let other = args.get_one_arg("frozenset.isdisjoint", vm.heap)?;
                defer_drop!(other, vm);
                Ok(Value::Bool(self.comparison_op(other, SetComparison::Disjoint, vm)?))
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::FrozenSet, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }
}

impl HeapItem for FrozenSet {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.storage.collect_dec_ref_ids(stack);
    }
}

/// Returns temporary set storage only for operator-valid set operands.
///
/// This is stricter than `Set::get_storage_from_value(...)`: operator forms
/// only accept CPython's set-like operands (`set`, `frozenset`, `dict_keys`,
/// and `dict_items`), while method forms accept any iterable.
fn get_storage_from_set_operand(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<SetStorage>> {
    let Value::Ref(id) = value else {
        return Ok(None);
    };

    match vm.heap.read(*id) {
        HeapReadOutput::Set(set) => Ok(Some(SetStorage::from_entries(
            set.get(vm.heap).0.clone_entries(vm.heap),
        ))),
        HeapReadOutput::FrozenSet(set) => Ok(Some(SetStorage::from_entries(
            set.get(vm.heap).storage.clone_entries(vm.heap),
        ))),
        HeapReadOutput::DictKeysView(view) => {
            let Set(storage) = view.to_set(vm)?;
            Ok(Some(storage))
        }
        HeapReadOutput::DictItemsView(view) => {
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
        // Skip `cached_hash` — it's recomputable from the entries and we
        // don't want to lock the snapshot format to the current hash function.
        self.storage.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for FrozenSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::wrap(SetStorage::deserialize(deserializer)?))
    }
}

/// Set-like source retained by a set iterator.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum SetIteratorSource {
    Set(HeapId),
    FrozenSet(HeapId),
}

/// Iterator over set or frozen-set storage.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SetIterator {
    source: SetIteratorSource,
    index: usize,
    expected_len: usize,
}

impl SetIterator {
    /// Allocates an iterator retaining a mutable set.
    fn from_set(id: HeapId, expected_len: usize, vm: &mut VM<'_>) -> Value {
        Self::allocate(SetIteratorSource::Set(id), expected_len, vm)
    }

    /// Allocates an iterator retaining a frozen set.
    fn from_frozen_set(id: HeapId, expected_len: usize, vm: &mut VM<'_>) -> Value {
        Self::allocate(SetIteratorSource::FrozenSet(id), expected_len, vm)
    }

    /// Returns the retained set-like source id for GC tracing.
    pub(crate) fn source_id(&self) -> HeapId {
        match self.source {
            SetIteratorSource::Set(id) | SetIteratorSource::FrozenSet(id) => id,
        }
    }

    /// Returns the captured number of values not yet yielded.
    pub(crate) fn size_hint(&self) -> usize {
        self.expected_len.saturating_sub(self.index)
    }

    /// Allocates an iterator and retains its source.
    fn allocate(source: SetIteratorSource, expected_len: usize, vm: &mut VM<'_>) -> Value {
        let source_id = match source {
            SetIteratorSource::Set(id) | SetIteratorSource::FrozenSet(id) => id,
        };
        let id = vm.heap.allocate(HeapData::SetIterator(Self {
            source,
            index: 0,
            expected_len,
        }));
        vm.heap.inc_ref(source_id);
        Value::Ref(id)
    }
}

impl HeapItem for SetIterator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.source_id());
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, SetIterator> {
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::SetIterator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.clone_value(vm.heap))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let (source_id, index, expected_len, mutable) = {
            let iter = self.get(vm.heap);
            (
                iter.source_id(),
                iter.index,
                iter.expected_len,
                matches!(iter.source, SetIteratorSource::Set(_)),
            )
        };
        let item = match vm.heap.get(source_id) {
            HeapData::Set(set) => {
                if mutable && set.len() != expected_len {
                    return Err(ExcType::runtime_error_set_changed_size());
                }
                set.storage().value_at(index)
            }
            HeapData::FrozenSet(set) => set.storage().value_at(index),
            _ => unreachable!("set iterator must retain set-like storage"),
        }
        .map(|value| value.clone_with_heap(vm.heap));
        if item.is_some() {
            self.get_mut(vm.heap).index += 1;
        }
        Ok(item)
    }
}

fn set_element_hash(value: &Value, vm: &mut VM<'_>) -> RunResult<u64> {
    if let Some(hash) = value.py_hash(vm)? {
        Ok(hash.raw())
    } else {
        let element_type = value.py_type_name(vm).into_owned();
        let unhashable_type = unhashable_type_name(value, vm)?;
        Err(ExcType::type_error_unhashable_set_element(
            &element_type,
            &unhashable_type,
        ))
    }
}

/// Finds the nested value responsible for a tuple-like element being unhashable.
fn unhashable_type_name(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let Value::Ref(id) = value else {
        return Ok(value.py_type_name(vm).into_owned());
    };
    let len = match vm.heap.get(*id) {
        HeapData::Tuple(tuple) => tuple.as_slice().len(),
        HeapData::NamedTuple(namedtuple) => namedtuple.as_vec().len(),
        _ => return Ok(value.py_type_name(vm).into_owned()),
    };

    for index in 0..len {
        let item = match vm.heap.get(*id) {
            HeapData::Tuple(tuple) => tuple.as_slice()[index].clone_with_heap(vm.heap),
            HeapData::NamedTuple(namedtuple) => namedtuple.as_vec()[index].clone_with_heap(vm.heap),
            _ => unreachable!("tuple-like heap value changed type"),
        };
        defer_drop!(item, vm);
        if item.py_hash(vm)?.is_none() {
            return unhashable_type_name(item, vm);
        }
    }

    Ok(value.py_type_name(vm).into_owned())
}
