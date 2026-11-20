//! The insertion-ordered hash table behind `dict` and `set`.
//!
//! Both types are a `HashTable<usize>` of buckets over a `Vec` of entry
//! slots, and both probe that table by calling back into guest code:
//! `__eq__` decides a collision, and the object it runs on can reach the very
//! container being probed. An index derived before such a call means nothing
//! after it — the entry may have moved, or the vector may be shorter than the
//! index. That bug class was previously guarded by two hand-kept copies of the
//! same probe, one per type, each documented as "the twin … the two must stay
//! in sync".
//!
//! This module holds the single copy. [`PyHashTable`] owns the bucket table so
//! no caller can desync it from the entries. A container implements
//! [`TableSource`]; the probe ([`TableProbe`]) and the walk ([`TableWalk`]),
//! the two operations that run guest code, are written once against it.
//!
//! Deletion leaves a **tombstone** rather than shifting the entries down, so a
//! slot index stays valid across one. Shifting was the second half of the same
//! bug class: a walk holding index `i` stepped over whichever entry slid into
//! `i` behind it, silently dropping it where CPython raises. Slots are
//! therefore not dense — `len` counts the live ones, the slot vector spans the
//! dead ones too.
//!
//! Tombstone policy:
//! - Deletion retains slots so existing cursors keep their positions.
//! - Insertion compacts with at least `COMPACT_MIN_SLOTS` slots when dead slots reach `slots / 2`.
//!   Compaction removes holes but retains capacity; it does not shrink the source's allocations.
//! - Clearing removes every slot; the bucket allocation can remain for reuse.
//! - Copies and snapshot vectors contain only live entries, in source order, and reserve by live count.
//!   Reading or copying a source never compacts it; empty sources skip their retained slots.
//! - Dumps preserve every slot, including tombstones, because serialized cursors use physical positions.
//! - Explicit reordering may renumber slots; it must use the container's mutation-detection contract.
//!
//! No slot number leaves this module. Code elsewhere reads the entries under
//! one borrow ([`PyHashTable::live`]), steps a position that is checked on
//! every step ([`TableWalk`], [`TableCursor`], [`TableScan`]), or acts on the
//! opaque [`Slot`] a probe returns. None of these can be computed by the
//! caller and read back after guest code has run.

use std::{marker::PhantomData, mem, slice};

use ahash::AHashSet;
use hashbrown::HashTable;
use monty_types::{ResourceError, ResourceTracker};
use smallvec::SmallVec;

use crate::{
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapReader},
    identity::Identity,
    resource_checks::check_entry_table_growth,
    types::dict::{eq_is_native, probe_native_eq},
    value::Value,
};

/// Slot count below which tombstones are left alone.
///
/// Compaction is only worth the reindex once a table is big enough for the
/// dead slots to cost a scan; under this a walk crosses them for free.
const COMPACT_MIN_SLOTS: usize = 8;

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

    /// A dead slot, left behind by a deletion so later slots keep their index.
    ///
    /// Must hold no heap reference: a tombstone is skipped by the refcount and
    /// GC walks, so anything it retained would leak. `Value::Undefined` is the
    /// marker both entry types use, since it can never be a Python value.
    fn tombstone() -> Self;

    /// Whether this slot still holds an entry.
    fn is_live(&self) -> bool;

    /// The `RuntimeError` raised when the container changes size mid-walk.
    ///
    /// CPython words this differently for the two types, so it belongs to the
    /// entry type rather than to the walk.
    fn changed_size_error() -> RunError;

    /// The `RuntimeError` raised when a live entry turns up after the walk has
    /// been handed every entry it was owed — a compaction renumbered the slots
    /// under the cursor.
    ///
    /// `None` for a container whose CPython iterator has no such guard, which
    /// is the case for `set`: `dictiterobject` carries `di->len` alongside its
    /// size check, `setiterobject` carries only the size check.
    fn changed_keys_error() -> Option<RunError> {
        None
    }
}

/// An entry whose owned values and initialized cached hash can be retained without Python callbacks.
pub(crate) trait CloneTableEntry: TableEntry {
    /// Retains the entry's values and preserves its cached hash.
    /// Keep implementations inline so copies avoid a helper call for every entry.
    fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self;
}

/// An insertion-ordered hash table: buckets of indices into a dense entry vector.
///
/// The bucket table is private and every structural change goes through a
/// method here, so it cannot drift out of step with the entries — the desync
/// that a hand-maintained `indices.insert_unique` next to a `entries.push` in
/// each of `dict` and `set` invited.
///
/// Entries are read under one borrow through [`live`](Self::live), which no
/// guest code can interrupt. Finding one goes through
/// [`TableProbe::find_index`], the only probe that survives a guest callback.
///
/// Slots are not dense. A deletion tombstones its slot in place so the
/// positions a walk is holding stay meaningful, which makes
/// [`len`](Self::len) (live entries) and the slot count two different numbers.
#[derive(Debug)]
pub(crate) struct PyHashTable<E> {
    /// Maps an entry's hash to its slot in `entries`. Tombstoned slots are
    /// removed from here, so a probe never reaches a dead one.
    indices: HashTable<usize>,
    /// Slots in insertion order, live entries and tombstones interleaved.
    ///
    /// Live entries are counted by `indices.len()`, not a field of their own:
    /// `Dict` embeds this inline, so another word would widen every heap slot.
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

impl<C: ContainsHeap, E: DropWithContext<C>> DropWithContext<C> for PyHashTable<E> {
    fn drop_with(self, context: &mut C) {
        // Tombstones own no references; destruction needs neither filtering nor clearing the index.
        self.entries.drop_with(context);
    }
}

/// Dumps every slot, tombstones included, as a plain sequence of entries.
///
/// Compacting would renumber the slots a dumped iterator's [`TableCursor`]
/// points at. A dump from before tombstones is simply a table with none.
impl<E: serde::Serialize> serde::Serialize for PyHashTable<E> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl<'de, E: TableEntry + serde::Deserialize<'de>> serde::Deserialize<'de> for PyHashTable<E> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries: Vec<E> = Vec::deserialize(deserializer)?;
        let mut table = Self {
            indices: HashTable::with_capacity(entries.len()),
            entries,
        };
        table.reindex();
        Ok(table)
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

    /// Visits live entries without Python callbacks, polling across physical slots, including tombstones.
    #[inline]
    fn for_each_copy_entry(&self, tracker: &ResourceTracker, visit: impl FnMut(&E)) -> RunResult<()> {
        let dense = self.entries.len() == self.len();
        // Dense tables need neither tombstone checks nor a remaining-entry counter.
        match (tracker.has_time_limit(), dense) {
            (false, true) => self.visit_copy_slots::<false, true>(tracker, visit),
            (false, false) => self.visit_copy_slots::<false, false>(tracker, visit),
            (true, true) => self.visit_copy_slots::<true, true>(tracker, visit),
            (true, false) => self.visit_copy_slots::<true, false>(tracker, visit),
        }
    }

    /// Visits live entries in insertion order without Python callbacks or source mutation.
    /// `DENSE` requires every slot to be live, as checked by `for_each_copy_entry`.
    /// Const flags remove unused checks from copies without deadlines and from dense tables.
    // Keep specializations separate so the compiler cannot merge them into a loop with runtime flag checks.
    #[inline(never)]
    fn visit_copy_slots<const POLL_DEADLINE: bool, const DENSE: bool>(
        &self,
        tracker: &ResourceTracker,
        mut visit: impl FnMut(&E),
    ) -> RunResult<()> {
        // Live count differs from slot count after deletion; dense specializations eliminate this counter.
        let mut remaining = self.len();
        for (slot, entry) in self.copy_slots().iter().enumerate() {
            // check_time_every polls once per interval of physical slots. Counting only live entries
            // would leave long tombstone gaps uninterruptible; the disabled specialization has no clock checks.
            if POLL_DEADLINE {
                tracker.check_time_every(slot)?;
            }
            // Equal slot/live counts prove there are no tombstones, so DENSE removes the liveness test.
            if DENSE || entry.is_live() {
                visit(entry);
                // For dense sources the slice ends at the last live entry; no separate stop count is needed.
                if DENSE {
                    continue;
                }
                remaining -= 1;
                // Sparse sources may have a long deleted suffix. Once all live entries are copied,
                // stop without scanning it, while leaving the source and its cursor positions untouched.
                if remaining == 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Empty copies must not scan retained tombstones.
    fn copy_slots(&self) -> &[E] {
        if self.is_empty() { &[] } else { &self.entries }
    }

    /// The live entries, in insertion order.
    ///
    /// The borrow is what makes this safe: nothing can mutate the table while
    /// it is held, so the entries it hands out are the table's current ones.
    #[inline]
    pub(crate) fn live(&self) -> Live<'_, E> {
        Live {
            slots: self.entries.iter(),
            remaining: self.len(),
        }
    }

    /// The entry a probe found.
    ///
    /// The slot must come from a probe of this table with no mutation since —
    /// see [`Slot`].
    #[inline]
    pub(crate) fn entry(&self, slot: &Slot) -> &E {
        self.get_live(slot.0).expect(PROBE_SLOT_LIVE)
    }

    /// Overwrites the entry a probe found, returning the old one.
    ///
    /// The replacement must probe the same as what it displaces, since its
    /// bucket stays where it is. A cache storing a new result for an existing
    /// key is the intended use.
    #[inline]
    #[expect(clippy::needless_pass_by_value, reason = "taking the slot spends it")]
    pub(crate) fn replace(&mut self, slot: Slot, entry: E) -> E {
        let old = self
            .entries
            .get_mut(slot.0)
            .filter(|e| e.is_live())
            .expect(PROBE_SLOT_LIVE);
        debug_assert_eq!(
            old.hash(),
            entry.hash(),
            "replace keeps the bucket, so the hash must match"
        );
        mem::replace(old, entry)
    }

    /// The entry a probe found, for rewriting in place.
    ///
    /// The entry must keep probing the same, since its bucket stays where it
    /// is. A dict rebinding the value at an existing key is the intended use.
    #[inline]
    #[expect(clippy::needless_pass_by_value, reason = "taking the slot spends it")]
    pub(crate) fn entry_mut(&mut self, slot: Slot) -> &mut E {
        self.entries
            .get_mut(slot.0)
            .filter(|e| e.is_live())
            .expect(PROBE_SLOT_LIVE)
    }

    /// Removes the entry a probe found, returning it.
    #[inline]
    #[expect(clippy::needless_pass_by_value, reason = "taking the slot spends it")]
    pub(crate) fn remove(&mut self, slot: Slot) -> E {
        self.tombstone_at(slot.0)
    }

    /// Validates `cursor`'s guards and yields the next live entry.
    ///
    /// For the heap-resident iterators, which keep their cursor in a different
    /// heap object from the table and so cannot hold a [`TableWalk`]. The
    /// cursor changes on an error too (an error is sticky, as in CPython), so
    /// the caller must store it back whatever the outcome.
    pub(crate) fn step(&self, cursor: &mut TableCursor) -> RunResult<Option<&E>> {
        Ok(cursor
            .advance::<E>(self.len(), |from| self.next_live_slot(from))?
            .map(|slot| &self.entries[slot]))
    }

    /// The live entry at `slot`, or `None` if the slot is out of range or was
    /// tombstoned by a deletion since the index was taken.
    #[inline]
    fn get_live(&self, slot: usize) -> Option<&E> {
        self.entries.get(slot).filter(|entry| entry.is_live())
    }

    /// The first live slot at or after `from`.
    #[inline]
    fn next_live_slot(&self, from: usize) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.entries[i].is_live())
    }

    /// The number of live entries — what `len()` reports to Python.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether the table holds no live entries.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Appends a live entry, dropping accumulated tombstones first if they have
    /// grown to half the slots.
    ///
    /// The caller must have established that `entry` is absent, through
    /// [`TableProbe::find_index`] or a probe that runs no guest code — a
    /// duplicate would be unreachable rather than rejected.
    ///
    /// Compacting only here mirrors CPython, which drops its dead slots in
    /// `dictresize` and nowhere else. It renumbers the survivors, so an
    /// in-progress iteration can miss one; [`TableCursor`] is what catches
    /// that, exactly as CPython's `di_used`/`di->len` pair does.
    #[inline]
    pub(crate) fn push(&mut self, entry: E) {
        debug_assert!(entry.is_live(), "push takes a live entry");
        if self.entries.len() >= COMPACT_MIN_SLOTS && self.entries.len() - self.len() >= self.entries.len() / 2 {
            self.compact();
        }
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

    /// Tombstones the entry at `slot` and returns it.
    ///
    /// Ownership of whatever the entry held transfers to the caller. Later
    /// slots keep their index, which is the point — a walk holding one is
    /// still looking at the entry it read. Only the one bucket leaves
    /// `indices`, so this is O(1) rather than the full reindex that shifting
    /// forced.
    ///
    /// The slot must be live; a probe or [`next_live_slot`](Self::next_live_slot)
    /// is how one is reached, and both skip the dead.
    fn tombstone_at(&mut self, slot: usize) -> E {
        debug_assert!(self.entries[slot].is_live(), "tombstoning a dead slot");
        let hash = self.entries[slot].hash();
        if let Ok(bucket) = self.indices.find_entry(hash, |&i| i == slot) {
            bucket.remove();
        }
        mem::replace(&mut self.entries[slot], E::tombstone())
    }

    /// Tombstones and returns the last live entry, if any.
    ///
    /// The tail in iteration order, which is what `set.pop` and `dict.popitem`
    /// take. Tombstones after it are left where they are; the next `push`
    /// clears them if they have accumulated.
    pub(crate) fn pop_last(&mut self) -> Option<E> {
        let slot = self.entries.iter().rposition(E::is_live)?;
        Some(self.tombstone_at(slot))
    }

    /// Takes every live entry out, leaving the table empty.
    ///
    /// The entries are returned rather than dropped because they own heap
    /// references; the caller releases them through [`DropWithContext`].
    /// Tombstones are dropped here rather than returned — they hold nothing.
    ///
    /// [`DropWithContext`]: crate::heap::DropWithContext
    pub(crate) fn take_entries(&mut self) -> Vec<E> {
        self.indices.clear();
        let mut entries = mem::take(&mut self.entries);
        entries.retain(E::is_live);
        entries
    }

    /// Drops every tombstone and renumbers `indices` onto the survivors.
    fn compact(&mut self) {
        self.entries.retain(E::is_live);
        self.reindex();
    }

    /// Rebuilds the bucket table from the current live slots.
    fn reindex(&mut self) {
        self.indices.clear();
        for index in 0..self.entries.len() {
            if !self.entries[index].is_live() {
                continue;
            }
            let hash = self.entries[index].hash();
            self.indices.insert_unique(hash, index, |&i| self.entries[i].hash());
        }
    }

    /// Finds the live entry cached under `hash` that `matches` accepts.
    ///
    /// For a lookup that runs no guest code, such as comparing string contents.
    /// A heap-resident table cannot stay borrowed across a guest callback (that
    /// needs `&mut VM`), so a probe with user `__eq__` goes through
    /// [`TableProbe::find_index`] or, for a table the caller owns, [`try_find`](Self::try_find).
    pub(crate) fn find(&self, hash: u64, matches: impl FnMut(&E) -> bool) -> Option<Slot> {
        self.position(hash, matches).map(Slot)
    }

    /// [`find`](Self::find) with a fallible comparison, stopping at the first error.
    ///
    /// For running user `__eq__` against a table guest code cannot reach — a
    /// set still being built, or a fresh algebra result — which therefore
    /// cannot change mid-probe. Stopping on the first error matches CPython.
    pub(crate) fn try_find(
        &self,
        hash: u64,
        mut matches: impl FnMut(&E) -> RunResult<bool>,
    ) -> RunResult<Option<Slot>> {
        // `HashTable::find` takes a `-> bool` predicate, so the error is parked
        // and later candidates are skipped until the walk ends.
        let mut error = None;
        let found = self.position(hash, |entry| {
            error.is_none()
                && matches(entry).unwrap_or_else(|err| {
                    error = Some(err);
                    false
                })
        });
        match error {
            Some(err) => Err(err),
            None => Ok(found.map(Slot)),
        }
    }

    /// The slot of the entry [`find`](Self::find) would return.
    fn position(&self, hash: u64, mut matches: impl FnMut(&E) -> bool) -> Option<usize> {
        self.probe(hash, |_, entry| entry.hash() == hash && matches(entry))
    }

    /// Walks the bucket chain for `hash`, stopping at the first entry `matches`
    /// accepts, and returns its slot.
    ///
    /// Private so no slot number escapes the module: every public lookup wraps
    /// it in an opaque [`Slot`].
    #[inline]
    fn probe(&self, hash: u64, mut matches: impl FnMut(usize, &E) -> bool) -> Option<usize> {
        self.indices
            .find(hash, |&index| matches(index, &self.entries[index]))
            .copied()
    }
}

/// Panic message for a [`Slot`] used after the table changed under it.
const PROBE_SLOT_LIVE: &str = "a probe slot is used before the table next changes";

/// The position of an entry a probe found.
///
/// Only a probe makes one, and it is neither `Clone` nor `Copy`, so a caller
/// cannot derive a position from an ordinal or keep one for later. It is valid
/// until the table next changes: consume it before anything that can run
/// guest code. [`PyHashTable::entry`], [`replace`](PyHashTable::replace) and
/// [`remove`](PyHashTable::remove) are the ways to use it.
#[must_use]
#[derive(Debug)]
pub(crate) struct Slot(usize);

/// The live entries of a table, in insertion order. See [`PyHashTable::live`].
pub(crate) struct Live<'a, E> {
    slots: slice::Iter<'a, E>,
    /// Live entries not yet yielded, so the size hint is exact.
    remaining: usize,
}

impl<'a, E: TableEntry> Iterator for Live<'a, E> {
    type Item = &'a E;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.slots.find(|entry| entry.is_live())?;
        self.remaining -= 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, f: F) -> B {
        self.slots.filter(|entry| entry.is_live()).fold(init, f)
    }
}

impl<E: TableEntry> ExactSizeIterator for Live<'_, E> {}

/// The outcome of the probe continuation that runs after every snapshotted
/// candidate has been compared and missed.
enum ProbeOutcome {
    /// The key was found in this entry.
    Found(Slot),
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
/// slot it eventually returns meaningful. An implementation must therefore
/// read through the heap each time rather than cache anything.
pub(crate) trait TableSource<'h> {
    /// The entry type of the table this source exposes.
    type Entry: TableEntry;

    /// Re-reads the live table out of the heap.
    fn table<'r>(&'r self, vm: &'r VM<'h>) -> &'r PyHashTable<Self::Entry>;

    /// Copies live entries into compact storage, releasing a partial copy if its resource checks fail.
    /// Source hashes and indices must be initialized.
    #[inline]
    fn copy_table(&self, vm: &mut VM<'h>) -> RunResult<PyHashTable<Self::Entry>>
    where
        Self::Entry: CloneTableEntry + DropWithContext<VM<'h>>,
    {
        if self.table(vm).is_empty() {
            Ok(PyHashTable::default())
        } else {
            copy_nonempty_table(self, vm)
        }
    }

    /// Retains a projection of live entries before later Python callbacks can mutate the source.
    /// Preflights the output buffer and releases a partial snapshot on timeout.
    fn snapshot_entries<T: DropWithContext<VM<'h>>>(
        &self,
        vm: &mut VM<'h>,
        mut clone: impl FnMut(&Self::Entry, &HeapReader<'h>) -> T,
    ) -> RunResult<Vec<T>> {
        let len = self.table(vm).len();
        if len == 0 {
            return Ok(Vec::new());
        }
        vm.heap
            .tracker
            .check_allocation(len.saturating_mul(mem::size_of::<T>()))?;
        let mut guard = DropGuard::new(Vec::with_capacity(len), vm);
        let (entries, vm) = guard.as_parts_mut();
        self.table(vm)
            .for_each_copy_entry(&vm.heap.tracker, |entry| entries.push(clone(entry, vm.heap)))?;
        Ok(guard.into_inner())
    }
}

/// Keeps bulk-copy allocation and cleanup out of the empty-copy path and native method dispatch.
#[inline(never)]
fn copy_nonempty_table<'h, S: TableSource<'h> + ?Sized>(source: &S, vm: &mut VM<'h>) -> RunResult<PyHashTable<S::Entry>>
where
    S::Entry: CloneTableEntry + DropWithContext<VM<'h>>,
{
    let len = source.table(vm).len();
    // Bucket rounding and control bytes fit within three words per entry, plus 32 bytes of padding.
    let entry_bytes = mem::size_of::<S::Entry>() + 3 * mem::size_of::<usize>();
    vm.heap
        .tracker
        .check_allocation(len.saturating_mul(entry_bytes).saturating_add(32))?;
    let mut guard = DropGuard::new(PyHashTable::<S::Entry>::with_capacity(len), vm);
    let (copy, vm) = guard.as_parts_mut();
    // Keep insertion in the scan: an outlined callback adds a call for every entry.
    source.table(vm).for_each_copy_entry(
        &vm.heap.tracker,
        #[inline(always)]
        |entry| {
            let cloned = entry.clone_with_heap(vm.heap);
            debug_assert!(cloned.is_live());
            debug_assert_eq!(cloned.hash(), entry.hash());

            // Fresh copies have no tombstones, so appending needs no compaction check.
            let index = copy.entries.len();
            copy.entries.push(cloned);
            copy.indices
                .insert_unique(entry.hash(), index, |&i| copy.entries[i].hash());
        },
    )?;
    Ok(guard.into_inner())
}

/// The guarded probe, for every [`TableSource`].
///
/// A blanket impl rather than a default method on [`TableSource`], so no
/// container can override the probe; implementing the source is all a
/// container does.
pub(crate) trait TableProbe<'h>: TableSource<'h> {
    /// Finds the index of the entry equal to `key`, or `None` if absent.
    ///
    /// `hash` must be `key`'s hash; taking it as an argument rather than
    /// computing it lets set algebra pass the hash cached beside an element,
    /// as CPython's `set_contains_entry` does, so nothing already in a table
    /// gets re-hashed.
    ///
    /// Candidates are snapshotted so `py_eq` can run without the bucket borrow
    /// held, and revalidated either side of it: only one that moved restarts
    /// the probe, as in CPython's `lookdict`. Restarts poll the limits, since
    /// each callback restarts the VM's dispatch countdown.
    ///
    /// The returned slot is valid only until the next call into the VM.
    fn find_index(&self, key: &Value, hash: u64, vm: &mut VM<'h>) -> RunResult<Option<Slot>>;
}

impl<'h, S: TableSource<'h>> TableProbe<'h> for S {
    fn find_index(&self, key: &Value, hash: u64, vm: &mut VM<'h>) -> RunResult<Option<Slot>> {
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
            let found = table.probe(hash, |index, entry| {
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
                return Ok(Some(Slot(index)));
            }
            if candidate_indices.is_empty() {
                return Ok(None);
            }

            for (&candidate_index, candidate_key) in candidate_indices.iter().zip(candidate_keys.iter()) {
                if !all_native && !probe_valid(self, candidate_index, hash, candidate_key, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                // CPython compares the stored key on the left.
                let eq = candidate_key.py_eq(key, vm)?;
                if !all_native && !probe_valid(self, candidate_index, hash, candidate_key, vm) {
                    vm.heap.tracker.check_memory_time()?;
                    continue 'restart;
                }
                if eq {
                    return Ok(Some(Slot(candidate_index)));
                }
            }

            // Nothing matched. Comparisons can themselves add colliding keys
            // the snapshot never saw, so a pass that ran any hands over to the
            // mutation-aware continuation — unless none could run user code.
            if all_native {
                return Ok(None);
            }
            match probe_after_compare(self, hash, key, candidate_keys, vm)? {
                ProbeOutcome::Found(slot) => return Ok(Some(slot)),
                ProbeOutcome::Missing => return Ok(None),
                // a candidate moved: fall through to the next probe from scratch
                ProbeOutcome::Restart => (),
            }
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
fn probe_after_compare<'h>(
    source: &impl TableSource<'h>,
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
        let (candidate_indices, candidate_keys) = probe_candidates(source, hash, vm);
        defer_drop!(candidate_keys, vm);
        let mut compared_any = false;

        for (&candidate_index, candidate_key) in candidate_indices.iter().zip(candidate_keys.iter()) {
            if !compared_ids.insert(candidate_key.id()) {
                continue;
            }
            if !probe_valid(source, candidate_index, hash, candidate_key, vm) {
                vm.heap.tracker.check_memory_time()?;
                return Ok(ProbeOutcome::Restart);
            }
            compared.push(candidate_key.clone_with_heap(vm.heap));
            compared_any = true;
            // CPython compares the stored key on the left.
            let eq = candidate_key.py_eq(key, vm)?;
            if !probe_valid(source, candidate_index, hash, candidate_key, vm) {
                vm.heap.tracker.check_memory_time()?;
                return Ok(ProbeOutcome::Restart);
            }
            if eq {
                return Ok(ProbeOutcome::Found(Slot(candidate_index)));
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
/// mutation-aware continuation calls this — [`find_index`](TableProbe::find_index)
/// inlines the same collection.
fn probe_candidates<'h>(
    source: &impl TableSource<'h>,
    hash: u64,
    vm: &VM<'h>,
) -> (SmallVec<[usize; 2]>, SmallVec<[Value; 2]>) {
    let mut indices: SmallVec<[usize; 2]> = SmallVec::new();
    let mut keys: SmallVec<[Value; 2]> = SmallVec::new();
    let table = source.table(vm);
    table.probe(hash, |index, entry| {
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
fn probe_valid<'h>(source: &impl TableSource<'h>, index: usize, hash: u64, key: &Value, vm: &VM<'h>) -> bool {
    source
        .table(vm)
        .get_live(index)
        .is_some_and(|entry| entry.hash() == hash && entry.probe_key().is(key))
}

/// The resize-checked step shared by the `dict` and `set` iterators.
///
/// Walking either container yields values into `py_eq` / `py_hash` /
/// membership tests, any of which can run guest code that mutates what is
/// being walked. The position is therefore re-validated against the live
/// length at every step rather than trusted from construction, and a change
/// ends the walk with the container's `RuntimeError` instead of reading past
/// the entries.
///
/// This is only the position; the yielded values, the recursion token and the
/// lending slots belong to the iterator built on top, because `dict` lends a
/// pair and `set` lends an element and its hash.
///
/// The position is a *slot*, so it steps over the tombstones a deletion leaves
/// and the live count it is owed is tracked separately — see
/// [`TableCursor`], which applies the same two guards to the heap-resident
/// Python iterators.
pub(crate) struct TableWalk<'a, 'h, S: TableSource<'h>> {
    source: &'a S,
    cursor: TableCursor,
    /// `'h` is the heap brand the source is read under; the walk never stores
    /// anything carrying it, but must not outlive it either.
    heap: PhantomData<fn(&'h ())>,
}

impl<'a, 'h, S: TableSource<'h>> TableWalk<'a, 'h, S> {
    /// Starts a walk over `source`, pinning the length it must keep.
    pub(crate) fn new(source: &'a S, vm: &VM<'h>) -> Self {
        Self {
            source,
            cursor: TableCursor::new(source.table(vm).len()),
            heap: PhantomData,
        }
    }

    /// The next live entry, or `None` once the walk is exhausted.
    ///
    /// Polls the limits (amortized) and re-checks the live length first, so a
    /// mutation from guest code between steps raises the container's
    /// `RuntimeError` here. The entry borrows both the source and `vm`, so it
    /// cannot outlive the source or cross anything that runs guest code.
    pub(crate) fn next<'v>(&mut self, vm: &'v VM<'h>) -> RunResult<Option<&'v S::Entry>>
    where
        'a: 'v,
    {
        // Keyed on entries yielded, not the slot: tombstones make the slot
        // skip values, and a walk over odd slots alone would never poll.
        vm.heap.tracker.check_time_every(self.cursor.yielded())?;
        self.source.table(vm).step(&mut self.cursor)
    }
}

/// A position that tolerates mutation, as CPython's `PyDict_Next` does.
///
/// Each step re-reads the table and yields the next live entry after the last
/// one, with no size check: an entry appended mid-scan is reached, a deleted
/// one is skipped. Repr uses this, since CPython's `dict_repr` walks the same
/// way and raises nothing when a `__repr__` mutates the dict.
#[derive(Debug, Default)]
pub(crate) struct TableScan {
    /// The slot to resume scanning from.
    slot: usize,
}

impl TableScan {
    /// The next live entry of `table` after the last one yielded.
    pub(crate) fn next<'t, E: TableEntry>(&mut self, table: &'t PyHashTable<E>) -> Option<&'t E> {
        let slot = table.next_live_slot(self.slot)?;
        self.slot = slot + 1;
        Some(&table.entries[slot])
    }
}

/// The mutation guards CPython's dict and set iterators apply, in its order.
///
/// Shared by [`TableWalk`] and by the heap-resident `dict`/`set` iterator
/// objects, which need the same rules but persist across suspensions. Those
/// flatten it into their own dumped fields, which keeps the keys a pre-tombstone
/// iterator was dumped with.
///
/// Two guards, because tombstones make one insufficient. The size check alone
/// is defeated by a paired delete and insert; the live count is what catches a
/// compaction renumbering the slots under the cursor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(from = "DumpedCursor")]
pub(crate) struct TableCursor {
    /// The slot to resume scanning from. Not an entry ordinal: tombstones make
    /// it run ahead of how many entries have been yielded.
    #[serde(rename = "index")]
    slot: usize,
    /// The live count pinned at construction, compared on every step.
    expected_len: usize,
    /// Live entries still owed, counted down per yield. CPython's `di->len`:
    /// finding another once this hits zero means the table was renumbered.
    remaining: usize,
    /// Set once a step has reached the end. Mirrors CPython clearing
    /// `di_dict`: an exhausted iterator returns `StopIteration` on every
    /// further call and never re-checks the size, even if the container was
    /// mutated after exhaustion.
    exhausted: bool,
    /// Set by a size-change error. CPython makes that error sticky
    /// (`di_used = -1`), so it repeats even once the size is restored.
    size_changed: bool,
}

/// A [`TableCursor`] as any supported dump stores it.
///
/// Dumps from before tombstones have no `remaining`, and a set iterator's no
/// `exhausted`. Their tables were dense, so the slot was an ordinal and the
/// count still owed follows from it.
#[derive(serde::Deserialize)]
struct DumpedCursor {
    index: usize,
    expected_len: usize,
    #[serde(default)]
    remaining: Option<usize>,
    #[serde(default)]
    exhausted: bool,
    #[serde(default)]
    size_changed: bool,
}

impl From<DumpedCursor> for TableCursor {
    fn from(dumped: DumpedCursor) -> Self {
        Self {
            slot: dumped.index,
            expected_len: dumped.expected_len,
            remaining: dumped
                .remaining
                .unwrap_or_else(|| dumped.expected_len.saturating_sub(dumped.index)),
            exhausted: dumped.exhausted,
            size_changed: dumped.size_changed,
        }
    }
}

impl TableCursor {
    /// Starts a cursor owed `expected_len` entries.
    pub(crate) fn new(expected_len: usize) -> Self {
        Self {
            slot: 0,
            expected_len,
            remaining: expected_len,
            exhausted: false,
            size_changed: false,
        }
    }

    /// Entries not yet yielded, for `__length_hint__`.
    pub(crate) fn size_hint(&self) -> usize {
        self.remaining
    }

    /// Entries yielded so far: a counter that rises by one per step, unlike
    /// the slot, so it is what an amortized time poll keys on.
    fn yielded(&self) -> usize {
        self.expected_len.saturating_sub(self.remaining)
    }

    /// Validates the two guards and takes `next_live` as the next slot.
    ///
    /// `next_live` finds the container's first live slot at or after the one
    /// given. It runs only once the guards pass, so an exhausted or failed
    /// cursor does not rescan a run of tombstones on every call. The size check fires only while the walk is live
    /// and *before* exhaustion, so a mutation on the final entry still raises
    /// on the terminating step. Then, having skipped tombstones to a live
    /// slot, finding one when nothing is owed means a compaction renumbered
    /// the table mid-walk — CPython's "keys changed" case, for the containers
    /// that guard it.
    ///
    /// Both errors leave the cursor as CPython leaves its iterator: a size
    /// error repeats on every later step, and "keys changed" exhausts it.
    fn advance<E: TableEntry>(
        &mut self,
        current_len: usize,
        next_live: impl FnOnce(usize) -> Option<usize>,
    ) -> RunResult<Option<usize>> {
        if self.exhausted {
            Ok(None)
        } else if self.size_changed || current_len != self.expected_len {
            self.size_changed = true;
            Err(E::changed_size_error())
        } else {
            match next_live(self.slot) {
                None => {
                    self.exhausted = true;
                    Ok(None)
                }
                Some(_) if self.remaining == 0 => {
                    // With no such guard for this container, stop rather than
                    // yield an entry the walk was never owed.
                    self.exhausted = true;
                    E::changed_keys_error().map_or(Ok(None), Err)
                }
                Some(slot) => {
                    self.slot = slot + 1;
                    self.remaining -= 1;
                    Ok(Some(slot))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use monty_types::{PrintWriter, ResourceLimits, ResourceTracker};

    use super::{CloneTableEntry, PyHashTable, TableCursor, TableEntry, TableSource};
    use crate::{
        bytecode::VM,
        exception_private::{RunError, RunResult},
        heap::{ContainsHeap, DropWithContext, Heap, HeapReader},
        run::{Program, SessionTables},
        types::set::SetEntry,
        value::Value,
    };

    // Raw tables in these tests are owned outside the VM and cannot be mutated by callbacks.
    impl<'h, E: TableEntry> TableSource<'h> for &PyHashTable<E> {
        type Entry = E;

        fn table<'r>(&'r self, _vm: &'r VM<'h>) -> &'r PyHashTable<E> {
            self
        }
    }

    fn copy_in_vm<E>(source: &PyHashTable<E>, heap: &mut Heap) -> RunResult<PyHashTable<E>>
    where
        E: CloneTableEntry,
        for<'h> E: DropWithContext<VM<'h>>,
    {
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        HeapReader::with(heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            source.copy_table(&mut vm)
        })
    }

    /// Compact output must not renumber the source or a cursor restored alongside it.
    #[test]
    fn copying_discards_holes_but_dumps_preserve_cursor_slots() {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut source = table_with_entries();
        for hash in (0..16).step_by(2) {
            remove(&mut source, hash, &mut heap);
        }
        let mut cursor = TableCursor::new(source.len());
        assert_eq!(source.step(&mut cursor).unwrap().unwrap().hash, 1);

        let copy = copy_in_vm(&source, &mut heap).unwrap();
        assert_eq!(copy.entries.len(), 8);
        assert_eq!(copy.entries.capacity(), 8);
        assert_eq!(source.entries.len(), 16);
        assert_eq!(source.entries.capacity(), 64);
        assert_eq!(
            copy.live().map(|entry| entry.hash).collect::<Vec<_>>(),
            (1..16).step_by(2).collect::<Vec<_>>()
        );

        let dumped = serde_json::to_value((&source, &cursor)).unwrap();
        let (restored, mut restored_cursor): (PyHashTable<SetEntry>, TableCursor) =
            serde_json::from_value(dumped).unwrap();
        assert_eq!(restored.entries.len(), 16);
        assert_eq!(source.step(&mut cursor).unwrap().unwrap().hash, 3);
        assert_eq!(restored.step(&mut restored_cursor).unwrap().unwrap().hash, 3);

        for hash in (1..16).step_by(2) {
            remove(&mut source, hash, &mut heap);
        }
        let empty_copy = copy_in_vm(&source, &mut heap).unwrap();
        assert_eq!(source.entries.len(), 16);
        assert_eq!(source.copy_slots().len(), 0);
        assert_eq!(empty_copy.entries.capacity(), 0);
        assert_eq!(empty_copy.indices.capacity(), 0);

        source.drop_with(&mut heap);
        copy.drop_with(&mut heap);
        empty_copy.drop_with(&mut heap);
        restored.drop_with(&mut heap);
    }

    /// Expiring after the first clone must interrupt a tombstone run before the next live entry.
    #[test]
    fn sparse_copy_polls_while_skipping_tombstones() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
        let mut heap = Heap::new(16, tracker);
        let slots = 2 * ResourceTracker::LOOP_CHECK_INTERVAL;
        let mut source = PyHashTable::with_capacity(slots);
        for value in 0..slots {
            source.push(StartsClockOnClone(SetEntry {
                value: Value::Int(i64::try_from(value).unwrap()),
                hash: u64::try_from(value).unwrap(),
            }));
        }
        for hash in 1..slots - 1 {
            let slot = source.find(u64::try_from(hash).unwrap(), |_| true).unwrap();
            source.remove(slot).drop_with(&mut heap);
        }
        let result = copy_in_vm(&source, &mut heap);
        heap.tracker.on_execution_stop();
        assert!(matches!(result, Err(RunError::UncatchableExc(_))));
        assert_eq!(source.entries.len(), slots);
        assert_eq!(source.len(), 2);
        source.drop_with(&mut heap);
    }

    /// A drained suffix needs no scan once the snapshot owns every live entry.
    #[test]
    fn snapshot_skips_trailing_tombstones() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
        let mut heap = Heap::new(16, tracker);
        let mut source = table_with_entries();
        for hash in 1..16 {
            remove(&mut source, hash, &mut heap);
        }
        source
            .entries
            .resize_with(2 * ResourceTracker::LOOP_CHECK_INTERVAL, SetEntry::tombstone);
        let result = source.for_each_copy_entry(&heap.tracker, |_| {
            heap.tracker.on_execution_start();
            thread::sleep(Duration::from_millis(1));
        });
        heap.tracker.on_execution_stop();
        assert!(result.is_ok());
        source.drop_with(&mut heap);
    }

    /// Snapshot polling must count the tombstones between live entries.
    #[test]
    fn sparse_snapshot_polls_while_skipping_tombstones() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
        let mut heap = Heap::new(16, tracker);
        let slots = 2 * ResourceTracker::LOOP_CHECK_INTERVAL;
        let mut source = PyHashTable::with_capacity(slots);
        for value in 0..slots {
            source.push(SetEntry {
                value: Value::Int(i64::try_from(value).unwrap()),
                hash: u64::try_from(value).unwrap(),
            });
        }
        for hash in 1..slots - 1 {
            remove(&mut source, u64::try_from(hash).unwrap(), &mut heap);
        }
        let mut retained = 0;
        let result = source.for_each_copy_entry(&heap.tracker, |_| {
            retained += 1;
            heap.tracker.on_execution_start();
            thread::sleep(Duration::from_millis(1));
        });
        heap.tracker.on_execution_stop();
        assert!(matches!(result, Err(RunError::UncatchableExc(_))));
        assert_eq!(retained, 1);
        assert_eq!(source.len(), 2);
        source.drop_with(&mut heap);
    }

    /// Expires the clock on the first clone, without latency assertions.
    struct StartsClockOnClone(SetEntry);

    impl TableEntry for StartsClockOnClone {
        fn probe_key(&self) -> &Value {
            self.0.probe_key()
        }

        fn hash(&self) -> u64 {
            self.0.hash()
        }

        fn tombstone() -> Self {
            Self(SetEntry::tombstone())
        }

        fn is_live(&self) -> bool {
            self.0.is_live()
        }

        fn changed_size_error() -> RunError {
            SetEntry::changed_size_error()
        }
    }

    impl CloneTableEntry for StartsClockOnClone {
        fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
            let cloned = Self(self.0.clone_with_heap(heap));
            heap.heap().tracker.on_execution_start();
            thread::sleep(Duration::from_millis(1));
            cloned
        }
    }

    impl<C: ContainsHeap> DropWithContext<C> for StartsClockOnClone {
        fn drop_with(self, heap: &mut C) {
            self.0.drop_with(heap);
        }
    }

    /// Deletion retains positions; only a later insertion crossing the threshold compacts them.
    #[test]
    fn insertion_compacts_at_the_dead_slot_threshold_without_shrinking() {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut table = table_with_entries();
        for hash in 0..7 {
            remove(&mut table, hash, &mut heap);
        }
        table.push(SetEntry {
            value: Value::Int(16),
            hash: 16,
        });
        assert_eq!(table.entries.len(), 17);

        remove(&mut table, 7, &mut heap);
        table.push(SetEntry {
            value: Value::Int(17),
            hash: 17,
        });
        assert_eq!(table.entries.len(), 10);
        assert_eq!(table.entries.capacity(), 64);
        assert_eq!(
            table.live().map(|entry| entry.hash).collect::<Vec<_>>(),
            (8..18).collect::<Vec<_>>()
        );
        table.drop_with(&mut heap);
    }

    /// Builds a table with spare capacity so tests distinguish compaction from shrinking.
    fn table_with_entries() -> PyHashTable<SetEntry> {
        let mut table = PyHashTable::with_capacity(64);
        for value in 0..16 {
            table.push(SetEntry {
                value: Value::Int(value),
                hash: u64::try_from(value).unwrap(),
            });
        }
        table
    }

    /// Removes a known entry while releasing its owned member.
    fn remove(table: &mut PyHashTable<SetEntry>, hash: u64, heap: &mut Heap) {
        let slot = table.find(hash, |_| true).unwrap();
        table.remove(slot).drop_with(heap);
    }
}
