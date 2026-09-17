//! The insertion-ordered hash table behind `dict` and `set`.
//!
//! Both types are a `HashTable<usize>` of buckets over a dense `Vec` of
//! entries, and both probe that table by calling back into guest code:
//! `__eq__` decides a collision, and the object it runs on can reach the very
//! container being probed. An index derived before such a call means nothing
//! after it — the entry may have moved, or the vector may be shorter than the
//! index. That bug class was previously guarded by two hand-kept copies of the
//! same probe, one per type, each documented as "the twin … the two must stay
//! in sync".
//!
//! This module holds the single copy. [`PyHashTable`] owns the bucket table so
//! no caller can desync it from the entries, and [`TableSource`] carries the
//! probe and the walk, the two operations that run guest code. Everything that
//! hands out an index does so only from inside a method that has already
//! revalidated it.

use std::{marker::PhantomData, mem};

use ahash::AHashSet;
use hashbrown::HashTable;
use monty_types::{ResourceError, ResourceTracker};
use smallvec::SmallVec;

use crate::{
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{RunError, RunResult},
    identity::Identity,
    resource_checks::check_entry_table_growth,
    types::dict::{eq_is_native, probe_native_eq},
    value::Value,
};

/// One entry of a Python hash table.
///
/// `dict` stores a key and a value, `set` stores just the element, but both
/// probe by one `Value` and cache its hash — that pair is all the shared probe
/// needs to see.
pub(crate) trait TableEntry {
    /// The value this entry is found by: a dict's key, a set's element.
    fn probe_key(&self) -> &Value;

    /// The cached hash of [`probe_key`](Self::probe_key).
    ///
    /// Reused rather than recomputed, as CPython does, so a probe never calls
    /// user `__hash__` on something already in the table.
    fn hash(&self) -> u64;
}

/// An insertion-ordered hash table: buckets of indices into a dense entry vector.
///
/// The bucket table is private and every structural change goes through a
/// method here, so it cannot drift out of step with the entries — the desync
/// that a hand-maintained `indices.insert_unique` next to a `entries.push` in
/// each of `dict` and `set` invited.
///
/// Reading entries is open, since walking them in order is ordinary work
/// (repr, serde, cloning). Finding one is not: use [`TableSource::find_index`],
/// which is the only probe that survives a guest callback.
#[derive(Debug)]
pub(crate) struct PyHashTable<E> {
    /// Maps an entry's hash to its index in `entries`.
    indices: HashTable<usize>,
    /// Dense entry vector, in insertion order.
    entries: Vec<E>,
}

impl<E> Default for PyHashTable<E> {
    fn default() -> Self {
        Self {
            indices: HashTable::new(),
            entries: Vec::new(),
        }
    }
}

impl<E: TableEntry> PyHashTable<E> {
    /// Creates an empty table sized for `capacity` entries.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            indices: HashTable::with_capacity(capacity),
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Indexes entries that already carry their hashes, in the order given.
    ///
    /// The bucket table is sized to the entry count: a `HashTable` keeps the
    /// buckets it grew to across `clear` and `remove`, so a table rebuilt this
    /// way costs what it holds rather than what it once held.
    pub(crate) fn from_entries(entries: Vec<E>) -> Self {
        let mut table = Self {
            indices: HashTable::with_capacity(entries.len()),
            entries,
        };
        table.reindex();
        table
    }

    /// The entries, in insertion order.
    #[inline]
    pub(crate) fn entries(&self) -> &[E] {
        &self.entries
    }

    /// The entries, mutably, for changing what an entry holds in place.
    ///
    /// Must not be used to add, remove or reorder entries, nor to change an
    /// entry's hash: the buckets index entries by position and are keyed on
    /// that hash, so either desyncs the table. Overwriting a whole entry goes
    /// through [`replace_at`](Self::replace_at); this is for refcount work.
    #[inline]
    pub(crate) fn entries_mut(&mut self) -> &mut [E] {
        &mut self.entries
    }

    /// Replaces the entry at `index`, returning the old one.
    ///
    /// The bucket pointing at `index` is left alone, so the replacement must
    /// probe the same as what it displaces — that it carries the same hash is
    /// asserted in debug builds. A dict rewriting the value at an existing key
    /// is the intended use.
    #[inline]
    pub(crate) fn replace_at(&mut self, index: usize, entry: E) -> E {
        debug_assert_eq!(
            entry.hash(),
            self.entries[index].hash(),
            "a replacement must carry the hash its bucket was indexed under"
        );
        mem::replace(&mut self.entries[index], entry)
    }

    /// The number of entries.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table holds no entries.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends an entry and indexes it.
    ///
    /// The caller must have established that `entry` is absent, through
    /// [`TableSource::find_index`] or a probe that runs no guest code — a
    /// duplicate would be unreachable rather than rejected.
    #[inline]
    pub(crate) fn push(&mut self, entry: E) {
        let index = self.entries.len();
        let hash = entry.hash();
        self.entries.push(entry);
        self.indices.insert_unique(hash, index, |&i| self.entries[i].hash());
    }

    /// Preflights the reallocation one [`push`](Self::push) would cause.
    ///
    /// The entry vector and the bucket table beside it can reallocate on the
    /// same insertion, with no allocation in between, so the two increments are
    /// summed into a single check against the pre-insertion usage.
    pub(crate) fn check_growth(&self, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        check_entry_table_growth(
            self.entries.len(),
            self.entries.capacity(),
            mem::size_of::<E>(),
            &self.indices,
            tracker,
        )
    }

    /// Removes the entry at `index`, returning it, and reindexes the rest.
    ///
    /// Removal shifts every later entry down, so every bucket after it would
    /// otherwise point one place too far; the table is rebuilt wholesale rather
    /// than patched. That makes removal O(n), which is a known cost of the
    /// dense-vector layout rather than of this type.
    pub(crate) fn remove_at(&mut self, index: usize) -> E {
        let entry = self.entries.remove(index);
        self.reindex();
        entry
    }

    /// Removes and returns the last entry, if any.
    ///
    /// Cheaper than [`remove_at`](Self::remove_at): nothing shifts, so only the
    /// one bucket has to go and the rest of the index stays valid. `set.pop`
    /// takes the tail for exactly this reason.
    pub(crate) fn pop_last(&mut self) -> Option<E> {
        let entry = self.entries.pop()?;
        self.indices
            .find_entry(entry.hash(), |&index| index == self.entries.len())
            .expect("every entry is indexed")
            .remove();
        Some(entry)
    }

    /// Takes every entry out, leaving the table empty.
    ///
    /// The entries are returned rather than dropped because they own heap
    /// references; the caller releases them through [`DropWithContext`].
    ///
    /// [`DropWithContext`]: crate::heap::DropWithContext
    pub(crate) fn take_entries(&mut self) -> Vec<E> {
        self.indices.clear();
        mem::take(&mut self.entries)
    }

    /// Consumes the table, yielding its entries for the caller to release.
    pub(crate) fn into_entries(self) -> Vec<E> {
        self.entries
    }

    /// Rebuilds the bucket table from the current entries.
    fn reindex(&mut self) {
        self.indices.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            self.indices
                .insert_unique(entry.hash(), index, |&i| self.entries[i].hash());
        }
    }

    /// Walks the bucket chain for `hash`, stopping at the first entry `matches`
    /// accepts, and returns that entry's index.
    ///
    /// The bucket borrow is held throughout, so this is sound only if nothing
    /// can mutate the table meanwhile: either `matches` runs no guest code, or
    /// the table is unreachable from guest code (still being built, or a fresh
    /// algebra result). A published table probed with user `__eq__` must use
    /// [`TableSource::find_index`], which revalidates around the callback.
    #[inline]
    pub(crate) fn probe_raw(&self, hash: u64, mut matches: impl FnMut(usize, &E) -> bool) -> Option<usize> {
        self.indices
            .find(hash, |&index| matches(index, &self.entries[index]))
            .copied()
    }
}

/// The outcome of the probe continuation that runs after every snapshotted
/// candidate has been compared and missed.
pub(crate) enum ProbeOutcome {
    /// The key was found in this entry.
    Found(usize),
    /// Every live candidate has been compared, none matched.
    Missing,
    /// A candidate moved mid-probe, so the whole probe must start over.
    Restart,
}

/// A heap-resident container whose table can be probed and walked while guest
/// code runs against it.
///
/// Implemented by the `HeapRead` handles for `dict` and `set`. The handle, not
/// a borrow of the table, is what the probe holds onto: [`table`](Self::table)
/// is re-read from the heap after every callback, which is what makes the
/// index it eventually returns meaningful. An implementation must therefore
/// read through the heap each time rather than cache anything.
pub(crate) trait TableSource<'h> {
    /// The entry type of the table this source exposes.
    type Entry: TableEntry;

    /// Re-reads the live table out of the heap.
    fn table<'r>(&self, vm: &'r VM<'h>) -> &'r PyHashTable<Self::Entry>;

    /// The `RuntimeError` raised when the container changes size mid-walk.
    ///
    /// CPython words this differently for the two types, so it belongs to the
    /// container rather than to the walk.
    fn changed_size_error() -> RunError;

    /// Finds the index of the entry equal to `key`, or `None` if absent.
    ///
    /// `hash` must be `key`'s hash — passed, not computed, so set algebra can
    /// reuse the one cached beside an element. Candidates are snapshotted so
    /// `py_eq` runs without the bucket borrow held, and revalidated either side
    /// of it; one that moved restarts the probe, as in CPython's `lookdict`.
    /// The returned index is live only until the next call into the VM.
    fn find_index(&self, key: &Value, hash: u64, vm: &mut VM<'h>) -> RunResult<Option<usize>> {
        // False for anything whose `__eq__` could mutate the container; when
        // true, revalidation and the miss continuation are skipped (the
        // str/int path).
        let key_native = eq_is_native(key, vm.heap);

        'restart: loop {
            // Collected inline rather than through `probe_candidates`: every
            // lookup runs this, and moving the buffers out of a call shows up
            // in the dict benchmarks.
            let mut candidate_indices: SmallVec<[usize; 2]> = SmallVec::new();
            let mut candidate_keys: SmallVec<[Value; 2]> = SmallVec::new();
            let mut all_native = key_native;
            let table = self.table(vm);
            // The predicate doubles as the probe walk: native pairs are
            // compared inline (a `true` stops at the match), and only pairs
            // that may need user code are cloned for the guarded loop below.
            // Once one is deferred, later candidates queue behind it so
            // comparisons keep CPython's probe order.
            let found = table.probe_raw(hash, |index, entry| {
                if entry.hash() != hash {
                    return false;
                }
                let entry_key = entry.probe_key();
                if candidate_indices.is_empty()
                    && let Some(eq) = probe_native_eq(entry_key, key, vm)
                {
                    eq
                } else {
                    candidate_indices.push(index);
                    candidate_keys.push(entry_key.clone_with_heap(vm.heap));
                    all_native = all_native && eq_is_native(entry_key, vm.heap);
                    false
                }
            });
            // Guarded before the early returns below: the deferred keys are
            // owned, and only the `candidate_indices.is_empty()` condition in
            // the predicate above keeps them empty on the `found` path.
            defer_drop!(candidate_keys, vm);
            if let Some(index) = found {
                return Ok(Some(index));
            }
            if candidate_indices.is_empty() {
                return Ok(None);
            }

            for (&candidate_index, candidate_key) in candidate_indices.iter().zip(candidate_keys.iter()) {
                if !all_native && !self.probe_valid(candidate_index, hash, candidate_key, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                // CPython compares the stored key on the left.
                let eq = candidate_key.py_eq(key, vm)?;
                if !all_native && !self.probe_valid(candidate_index, hash, candidate_key, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                if eq {
                    return Ok(Some(candidate_index));
                }
            }

            // Nothing matched. Comparisons can themselves add colliding keys
            // the snapshot never saw, so a pass that ran any hands over to the
            // mutation-aware continuation — unless none could run user code.
            if all_native {
                return Ok(None);
            }
            match self.probe_after_compare(hash, key, candidate_keys, vm)? {
                ProbeOutcome::Found(index) => return Ok(Some(index)),
                ProbeOutcome::Missing => return Ok(None),
                // a candidate moved: fall through to the next probe from scratch
                ProbeOutcome::Restart => (),
            }
        }
    }

    /// Continues a probe whose comparisons all missed, in case one of them
    /// mutated the container and added a colliding key.
    ///
    /// Re-reads the candidates until a pass finds nothing new, skipping keys
    /// already compared so no user `__eq__` runs twice, as CPython's live probe
    /// chain does. Inline-compared native pairs are deliberately left out of
    /// the seen set: repeating one is side-effect-free, and tracking them would
    /// put clone and identity bookkeeping on the pure-native fast path.
    fn probe_after_compare(
        &self,
        hash: u64,
        key: &Value,
        already_compared: &[Value],
        vm: &mut VM<'h>,
    ) -> RunResult<ProbeOutcome> {
        // The clones keep every compared key alive so its heap slot cannot be
        // recycled into a new key that would then be skipped by identity.
        let compared: SmallVec<[Value; 2]> = already_compared.iter().map(|k| k.clone_with_heap(vm.heap)).collect();
        defer_drop_mut!(compared, vm);
        // Identity set for O(1) seen-checks — a linear scan is quadratic over
        // a fully colliding container, all skips, before the poll below is
        // reached.
        let mut compared_ids: AHashSet<Identity> = compared.iter().map(Value::id).collect();

        loop {
            // Polled up front so every entry checks the limits at least once:
            // the comparisons that brought us here ran user code, restarting
            // the VM dispatch countdown, so no checkpoint fires otherwise —
            // including on the no-mutation pass that returns `Missing`.
            vm.heap.tracker.check_memory_time()?;
            let (candidate_indices, candidate_keys) = self.probe_candidates(hash, vm);
            defer_drop!(candidate_keys, vm);
            let mut compared_any = false;

            for (&candidate_index, candidate_key) in candidate_indices.iter().zip(candidate_keys.iter()) {
                if !compared_ids.insert(candidate_key.id()) {
                    continue;
                }
                if !self.probe_valid(candidate_index, hash, candidate_key, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    return Ok(ProbeOutcome::Restart);
                }
                compared.push(candidate_key.clone_with_heap(vm.heap));
                compared_any = true;
                // CPython compares the stored key on the left.
                let eq = candidate_key.py_eq(key, vm)?;
                if !self.probe_valid(candidate_index, hash, candidate_key, vm) {
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
    /// owned reference to each of their keys.
    ///
    /// Cloning the keys lets `py_eq` run without the bucket borrow held; the
    /// caller owns the returned keys and must drop them. Only the
    /// mutation-aware continuation calls this — [`find_index`](Self::find_index)
    /// inlines the same collection.
    fn probe_candidates(&self, hash: u64, vm: &VM<'h>) -> (SmallVec<[usize; 2]>, SmallVec<[Value; 2]>) {
        let mut indices: SmallVec<[usize; 2]> = SmallVec::new();
        let mut keys: SmallVec<[Value; 2]> = SmallVec::new();
        let table = self.table(vm);
        table.probe_raw(hash, |index, entry| {
            if entry.hash() == hash {
                indices.push(index);
                keys.push(entry.probe_key().clone_with_heap(vm.heap));
            }
            false
        });
        (indices, keys)
    }

    /// Checks that a snapshotted candidate still names the same live entry.
    ///
    /// The caller checks before and after `py_eq`; a mismatch restarts the probe.
    #[inline]
    fn probe_valid(&self, index: usize, hash: u64, key: &Value, vm: &VM<'h>) -> bool {
        self.table(vm)
            .entries()
            .get(index)
            .is_some_and(|entry| entry.hash() == hash && entry.probe_key().is(key))
    }
}

/// The resize-checked step shared by the `dict` and `set` iterators.
///
/// Walking either container can run guest code that mutates it, so the position
/// is re-checked against the live length at every step and a change ends the
/// walk with the container's `RuntimeError` rather than reading past the
/// entries. Only the position lives here — the yielded values, recursion token
/// and lending slots belong to the iterator, which differs per type.
pub(crate) struct TableWalk<'a, 'h, S: TableSource<'h>> {
    source: &'a S,
    index: usize,
    expected_len: usize,
    /// `'h` is the heap brand the source is read under; the walk never stores
    /// anything carrying it, but must not outlive it either.
    heap: PhantomData<fn(&'h ())>,
}

impl<'a, 'h, S: TableSource<'h>> TableWalk<'a, 'h, S> {
    /// Starts a walk over `source`, pinning the length it must keep.
    pub(crate) fn new(source: &'a S, vm: &VM<'h>) -> Self {
        Self {
            source,
            index: 0,
            expected_len: source.table(vm).len(),
            heap: PhantomData,
        }
    }

    /// The source being walked, for reading the entry an index names.
    pub(crate) fn source(&self) -> &'a S {
        self.source
    }

    /// The entry index to read next, or `None` once the walk is exhausted.
    ///
    /// Polls the limits (amortized) and re-reads the live length first, so a
    /// mutation from guest code between steps is caught here rather than by an
    /// out-of-bounds index. The returned index is live only until the next call
    /// into the VM.
    pub(crate) fn advance(&mut self, vm: &mut VM<'h>) -> RunResult<Option<usize>> {
        vm.heap.tracker.check_time_every(self.index)?;
        if self.source.table(vm).len() != self.expected_len {
            return Err(S::changed_size_error());
        }
        if self.index >= self.expected_len {
            Ok(None)
        } else {
            let index = self.index;
            self.index += 1;
            Ok(Some(index))
        }
    }
}
