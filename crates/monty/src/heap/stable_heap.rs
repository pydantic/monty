use std::{
    cell::{Cell, UnsafeCell},
    fmt,
    mem::MaybeUninit,
};

use crate::heap::{HeapId, free_list::FreeList, stable_heap::iter::HeapEntriesIter};

/// Number of entries per page. Chosen to balance between wasted memory (from
/// partially-filled last pages) and the frequency of page allocations.
const PAGE_SIZE: usize = 256;

/// A single page of heap entries. Each page is a fixed-size boxed slice of
/// `MaybeUninit` slots — only slots at indices below `HeapEntries::len` are
/// initialized.
type Page<T> = Box<[Slot<T>; PAGE_SIZE]>;
type Slot<T> = MaybeUninit<Option<T>>;

/// Paged storage for heap entries that guarantees address stability. This also
/// has other conveniences like only needing `&self` to allocate, and a free list
/// for reuse of freed slots.
///
/// Entries are stored in fixed-size pages of `MaybeUninit<Option<T>>`.
/// Only slots that have been `push`ed are initialized — new pages are allocated
/// without touching the memory, avoiding the cost of writing `None` to every slot.
///
/// Once a page is allocated, it is never reallocated or moved in memory.
/// This is the key invariant that makes `&self` allocation sound: a reference
/// derived from an entry's data will remain valid for the entry's entire lifetime,
/// even as new pages are appended via `allocate(&self)`.
///
/// The free list tracks slot IDs freed by `dec_ref` for reuse by `allocate`,
/// keeping memory usage roughly constant for long-running loops that repeatedly
/// allocate and free values.
///
/// ## Interior mutability and safety
///
/// `pages`, `len`, and `free_list` use interior mutability (`UnsafeCell`/`Cell`)
/// so that `allocate` can take `&self` instead of `&mut self`. This is sound because:
///
/// - **`allocate(&self)`** only writes to the slot at index `len` (never readable
///   by anyone, since all reads require `index < len`) or to a freed slot from the
///   free list (no active borrows exist on freed slots).
/// - **`Vec::push` on `pages`** during allocation reallocates the page pointer array,
///   but not the page contents. Any existing `&T` reference points into a
///   `Box`'s heap allocation, not into the `Vec`'s buffer.
///
/// Index `i` maps to `pages[i / PAGE_SIZE][i % PAGE_SIZE]`.
pub(crate) struct StableHeap<T> {
    /// Fixed-size pages of heap entries. Each page is heap-allocated once and
    /// never moved, providing address stability for all contained entries.
    /// Wrapped in `UnsafeCell` to allow `allocate(&self)` to append new pages.
    pages: UnsafeCell<Vec<Page<T>>>,
    /// Total number of initialized slots (including freed ones).
    /// Uses `Cell` for interior mutability so `allocate(&self)` can increment.
    len: Cell<usize>,
    /// IDs of freed slots available for reuse. Populated by `free`, consumed by `allocate`.
    free_list: FreeList,
}

impl<T: fmt::Debug> fmt::Debug for StableHeap<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            // SAFETY: [DH] - debug formatting the `None` values never calls `.allocate()`, and `HeapEntries` is not `Sync`,
            // so there can be no re-entrancy which observes a `None` value at the same time which allocates.
            .entries(unsafe { HeapEntriesIter::new(self) })
            .finish()
    }
}

impl<T> StableHeap<T> {
    /// Creates a new paged storage pre-allocating enough pages for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        let num_pages = capacity.div_ceil(PAGE_SIZE);
        let mut pages = Vec::with_capacity(num_pages);
        for _ in 0..num_pages {
            pages.push(create_page());
        }
        Self {
            pages: UnsafeCell::new(pages),
            len: Cell::new(0),
            free_list: FreeList::new(),
        }
    }

    /// Returns the total number of initialized slots (including freed ones).
    #[inline]
    pub fn len(&self) -> usize {
        self.len.get()
    }

    /// Returns a shared reference to the entry at `index`.
    ///
    /// # Panics
    /// Panics if `index >= len`, or if the slot is freed.
    #[inline]
    #[track_caller]
    pub fn get(&self, id: HeapId) -> &T {
        // SAFETY: [DH] - this call panics rather than expose free slots which could be invalidated
        // by calls to `.allocate()`.
        unsafe { self.slot_at(id) }.expect("HeapEntries::get - data already freed")
    }

    /// Returns a mutable reference to the entry at `index`. Entries can also be
    /// freed via the returned `StableHeapEntry`'s `free` method.
    ///
    /// # Panics
    /// Panics if `index >= len`.
    #[inline]
    #[track_caller]
    pub fn entry(&mut self, id: HeapId) -> Option<StableHeapEntry<'_, T>> {
        assert!(id.index() < self.len.get(), "StableHeap::entry - {id:?} out of bounds");
        let (page_idx, slot_idx) = Self::page_slot_indices(id);
        let slot = &mut self.pages.get_mut()[page_idx][slot_idx];
        // SAFETY: [DH] - all slots at indices < self.len have been initialized via `allocate`.
        let value = unsafe { slot.assume_init_mut() };
        StableHeapEntry::new(id, value, &mut self.free_list)
    }

    /// Retain only values satisfying the predicate, freeing the rest.
    pub fn retain(&mut self, mut predicate: impl FnMut(usize, &mut T) -> bool) {
        let len = self.len.get();
        for i in 0..len {
            if let Some(mut entry) = self.entry(HeapId::from_index(i))
                && !predicate(i, entry.get())
            {
                entry.free();
            }
        }
    }

    /// Allocates a slot — reusing from the free list or appending — and returns its ID.
    ///
    /// Takes `&self` instead of `&mut self`, enabling allocation while holding shared
    /// borrows to other heap entries. This is the core operation that makes
    /// `Heap::allocate(&self)` possible.
    ///
    /// # Safety contract (enforced by caller structure, not runtime checks)
    ///
    /// - No `&mut` reference to `pages` or `free_list` exists. Guaranteed because
    ///   all `&mut self` methods on `HeapEntries` require exclusive access, and the
    ///   borrow checker prevents calling this `&self` method while any `&mut self`
    ///   method is active.
    /// - **New slots** (at index `len`) have never been initialized — no existing
    ///   reference can point to them, because `get()` requires `index < len`.
    /// - **Reused slots** (from free list) were freed via `dec_ref` and have no
    ///   active borrows — the slot was `.take()`n and its ID added to the free list.
    /// - **Vec growth** (`pages.push(new_page)`) reallocates the page pointer array,
    ///   not the page contents. Any existing `&T` reference points into a
    ///   `Box`'s heap allocation, not into the `Vec`'s buffer.
    pub fn allocate(&self, value: T) -> HeapId {
        // SAFETY: [DH]
        // - This is the only `&self` method which uses `pages.get()` to create a mutable reference to `pages`.
        // - This method is not re-entrant, and not `Sync`, so cannot have a global `StableHeap` where
        //   allocator re-entrancy on `pages.push` could cause a problem.
        // - No `&T` borrows derived from `pages` can be invalidated by `allocate`; only borrows to `None`
        //   entries can be invalidated. Other callsites (e.g. iteration) have safety contracts that
        //   forbid calling `allocate` while borrows which may be `None` are live.
        //
        // TODO: use `UnsafeCell::as_mut_unchecked` when stable.
        let pages = unsafe { self.pages.get().as_mut_unchecked() };
        let (id, slot) = if let Some(id) = self.free_list.pop() {
            let (page_idx, slot_idx) = Self::page_slot_indices(id);
            let slot = &mut pages[page_idx][slot_idx];
            debug_assert!(
                // SAFETY: [DH] - previously freed slots must have previously been initialized.
                unsafe { slot.assume_init_ref() }.is_none(),
                "allocate - popped free slot {id:?} is not actually free"
            );
            (id, slot)
        } else {
            let id = HeapId::from_index(self.len.get());
            let (page_idx, slot_idx) = Self::page_slot_indices(id);
            if page_idx >= pages.len() {
                pages.push(create_page());
            }
            self.len.set(id.index() + 1);
            (id, &mut pages[page_idx][slot_idx])
        };

        // Write to the new slot. No need to worry about initializedness / the destructor - either the slot was
        // free (in which case the slot is `None` and has no meaningful destructor), or the slot is not
        // initialized yet.
        slot.write(Some(value));

        id
    }

    /// Iterates the live values
    pub fn iter(&self) -> impl Iterator<Item = (HeapId, &T)> {
        // SAFETY: [DH] - iterating only the live entries ensures that caller
        // can never observe `None` entries which could be invalidated by
        // calls to `allocate()`. Entries can only be freed by `&mut self` methods.
        unsafe { HeapEntriesIter::new(self) }.filter_map(|(idx, slot)| slot.map(|s| (HeapId::from_index(idx), s)))
    }

    /// Accesses the slot at `id` with an immutable borrow on self.
    ///
    /// # Safety:
    /// - Caller must ensure that the `None` references returned do not overlap with any calls to `allocate()`,
    ///   as these will be invalidated by the reuse of freed slots.
    #[track_caller]
    unsafe fn slot_at(&self, id: HeapId) -> Option<&T> {
        assert!(id.index() < self.len.get(), "StableHeap::slot - {id:?} out of bounds");
        let (page_idx, slot_idx) = Self::page_slot_indices(id);
        // SAFETY: [DH] - modification to pages Vec only occur in allocate, which only writes to `None` entries
        // and doesn't cause reallocation of the boxed contents of existing pages, so temporary read of `pages` here is ok.
        //
        // TODO: use `UnsafeCell::as_ref_unchecked` when stable.
        let pages = unsafe { self.pages.get().as_ref_unchecked() };
        let slot = &pages[page_idx][slot_idx];
        // SAFETY: [DH] - all slots at indices < self.len have been initialized via `allocate`.
        unsafe { slot.assume_init_ref() }.as_ref()
    }

    fn page_slot_indices(id: HeapId) -> (usize, usize) {
        let index = id.index();
        (index / PAGE_SIZE, index % PAGE_SIZE)
    }

    /// Tests whether the value at index i is allocated. Panics if `i >= self.len()`
    #[cfg(test)]
    fn is_allocated(&self, id: HeapId) -> bool {
        // SAFETY: [DH] - call does not expose borrowed data
        unsafe { self.slot_at(id) }.is_some()
    }
}

/// Allocates a new page of uninitialized slots directly on the heap.
#[expect(clippy::unnecessary_box_returns, reason = "entire intent is to heap-allocate")]
fn create_page<T>() -> Box<[Slot<T>; PAGE_SIZE]> {
    let raw = Box::into_raw(Box::<[Slot<T>]>::new_uninit_slice(PAGE_SIZE)).cast();
    // SAFETY: [DH] - allocation is known to be exactly PAGE_SIZE slots, so
    // the cast-ed pointer can still be used with `Box::from_raw`
    unsafe { Box::from_raw(raw) }
}

/// Submodule for `StableHeapEntry` to help enforce a safety boundary around the `new` constructor.
mod stable_heap_entry {
    use crate::heap::{HeapId, free_list::FreeList};

    pub struct StableHeapEntry<'a, T> {
        id: HeapId,
        value: &'a mut Option<T>,
        free_list: &'a mut FreeList,
    }

    impl<'a, T> StableHeapEntry<'a, T> {
        // Only way to construct a `StableHeapEntry`. Returns `None` if the slot is already freed.
        pub fn new(id: HeapId, value: &'a mut Option<T>, free_list: &'a mut FreeList) -> Option<Self> {
            value.is_some().then_some(Self { id, value, free_list })
        }

        pub fn free(self) -> T {
            self.free_list.push(self.id);
            // SAFETY: [DH] - impossible to get a `StableHeapEntry` with `None` slot - `new`
            // is the only constructor
            unsafe { self.value.take().unwrap_unchecked() }
        }

        pub fn get(&mut self) -> &mut T {
            // SAFETY: [DH] - impossible to get a `StableHeapEntry` with `None` slot - `new`
            // is the only constructor
            unsafe { self.value.as_mut().unwrap_unchecked() }
        }
    }
}

use stable_heap_entry::StableHeapEntry;

/// Serializes as a struct with two fields: `entries` (flat vec of all initialized
/// slots) and `free_list` (vec of freed slot IDs). This avoids exposing the
/// internal paged layout in the wire format.
impl<T> serde::Serialize for StableHeap<T>
where
    T: serde::Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // SAFETY: [DH] - serializing `None` data cannot cause allocation, so `None` entries
        // cannot be observed by serialization, even if reentrant.
        serializer.collect_seq(unsafe { HeapEntriesIter::new(self) }.map(|(_idx, slot)| slot))
    }
}

impl<'de, T> serde::Deserialize<'de> for StableHeap<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries: Vec<Option<T>> = Vec::deserialize(deserializer)?;
        let mut this = Self::with_capacity(entries.len());

        // Re-initialize the freelist from none entries
        this.free_list = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.is_none())
            .map(|(idx, _)| HeapId::from_index(idx))
            .collect::<Vec<_>>()
            .into();

        // Set the initialized region
        this.len.set(entries.len());

        // Write all pages from the entries vec
        let pages = this.pages.get_mut();
        for (index, entry) in entries.into_iter().enumerate() {
            let (page_idx, slot_idx) = Self::page_slot_indices(HeapId::from_index(index));
            pages[page_idx][slot_idx].write(entry);
        }
        Ok(this)
    }
}

impl<T> Drop for StableHeap<T> {
    fn drop(&mut self) {
        let len = self.len.get();
        let pages = self.pages.get_mut();
        for i in 0..len {
            let slot = &mut pages[i / PAGE_SIZE][i % PAGE_SIZE];
            // SAFETY: [DH] - all slots at indices < self.len have been initialized via `allocate`.
            unsafe { slot.assume_init_drop() };
        }
    }
}

/// Place iterator inside a submodule to create a safety boundary on `new` constructor
mod iter {
    use super::StableHeap;
    use crate::heap::HeapId;

    pub(super) struct HeapEntriesIter<'a, T> {
        entries: &'a StableHeap<T>,
        index: usize,
    }

    impl<'a, T> HeapEntriesIter<'a, T> {
        /// # Safety:
        ///
        /// The caller must ensure that `HeapEntries::allocate()`
        /// is never called for the lifetime `'a` for which this iterator and its
        /// yielded elements exist.
        ///
        /// Allocation may write to `None` entries, which would cause unsafe
        /// aliasing.
        pub unsafe fn new(entries: &'a StableHeap<T>) -> Self {
            Self { entries, index: 0 }
        }
    }

    impl<'a, T> Iterator for HeapEntriesIter<'a, T> {
        type Item = (usize, Option<&'a T>);

        fn next(&mut self) -> Option<Self::Item> {
            let current_index = self.index;
            if current_index >= self.entries.len() {
                return None;
            }
            self.index += 1;
            // SAFETY: [DH] - caller guaranteed no aliasing when calling `HeapEntriesIter::new`
            let slot = unsafe { self.entries.slot_at(HeapId::from_index(current_index)) };
            Some((current_index, slot))
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            let remaining = self.entries.len().saturating_sub(self.index);
            (remaining, Some(remaining))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn allocate_while_reference_alive() {
        // Allocate a value, hold a shared reference to it, then allocate
        // another value. The first reference must remain valid.
        let entries = StableHeap::with_capacity(16);
        let id_a = entries.allocate("a");
        let ref_a = entries.get(id_a);

        // Allocate while ref_a is live
        let id_b = entries.allocate("b");

        // Both references must be readable
        assert_eq!(*ref_a, "a");
        assert_eq!(*entries.get(id_b), "b");
    }

    #[test]
    fn allocate_triggers_new_page_while_reference_alive() {
        // Fill the first page, hold a reference into it, then allocate into a
        // second page. The reference must survive the Vec<Page>::push.
        let entries = StableHeap::with_capacity(PAGE_SIZE);

        // Fill the first page.
        for _ in 0..PAGE_SIZE {
            entries.allocate("fill");
        }
        assert_eq!(entries.len(), PAGE_SIZE);

        // Hold a reference into the first page.
        let first_ref = entries.get(HeapId::from_index(0));

        // This allocation creates a second page — the pages Vec reallocates
        // its pointer buffer, but Box<Page> contents must not move.
        let overflow_id = entries.allocate("overflow");

        // The reference into the first page must still be valid.
        assert_eq!(*first_ref, "fill");
        assert_eq!(*entries.get(overflow_id), "overflow");
    }

    #[test]
    fn free_list_reuse_while_reference_alive() {
        // Allocate three values, free the middle one, then reallocate while
        // holding a reference to a different live slot.
        let mut entries = StableHeap::with_capacity(16);
        let id_a = entries.allocate("a");
        let id_b = entries.allocate("b");
        let _id_c = entries.allocate("c");

        // Free slot b (simulates dec_ref taking the value and calling free).
        entries.entry(id_b).unwrap().free();

        // Hold a reference to slot a.
        let ref_a = entries.get(id_a);

        // Reallocate into the freed slot while ref_a is live.
        let id_reused = entries.allocate("reused");
        assert_eq!(id_reused, id_b); // should reuse the freed slot

        // ref_a must still be valid.
        assert_eq!(*ref_a, "a");
        // Reused slot has new data.
        assert_eq!(*entries.get(id_reused), "reused");
    }

    #[test]
    fn multiple_live_references_during_allocation() {
        // Hold references to multiple slots across different pages, then
        // allocate. All references must survive.
        let entries = StableHeap::with_capacity(PAGE_SIZE * 2);

        // Fill two pages.
        for _ in 0..PAGE_SIZE * 2 {
            entries.allocate("filler");
        }

        // Hold references in both pages.
        let ref_first_page = entries.get(HeapId::from_index(0));
        let ref_second_page = entries.get(HeapId::from_index(PAGE_SIZE));

        // Allocate a third page.
        let new_id = entries.allocate("new");

        // All references must be readable.
        assert_eq!(*ref_first_page, "filler");
        assert_eq!(*ref_second_page, "filler");
        assert_eq!(*entries.get(new_id), "new");
    }

    #[test]
    fn allocate_into_freed_slot_does_not_alias_other_slots() {
        // Free several slots, then reallocate into them while holding live
        // references to other slots. Reallocation must not disturb those
        // references — using distinct values lets a stray alias surface as
        // a value mismatch.
        let mut entries = StableHeap::with_capacity(16);

        let ids: Vec<_> = (0..8).map(|_| entries.allocate("original")).collect();

        // Free even-indexed slots.
        for &id in ids.iter().step_by(2) {
            entries.entry(id).unwrap().free();
        }

        // Hold references to odd-indexed (live) slots.
        let live_refs: Vec<_> = ids.iter().skip(1).step_by(2).map(|id| entries.get(*id)).collect();

        // Reallocate into freed slots with a different value.
        for _ in 0..4 {
            entries.allocate("realloc");
        }

        // Live references must still see the original value.
        for r in &live_refs {
            assert_eq!(**r, "original");
        }
    }

    #[test]
    fn retain_then_allocate_reuses_freed_slots() {
        // Use retain to free slots, then allocate into the freed slots.
        let mut entries = StableHeap::with_capacity(16);

        for _ in 0..6 {
            entries.allocate("v");
        }

        // Retain only even-indexed entries.
        entries.retain(|i, _| i % 2 == 0);

        // Odd slots should now be None.
        for i in [1, 3, 5] {
            assert!(!entries.is_allocated(HeapId::from_index(i)));
        }

        // Allocate should reuse freed slots.
        let r1 = entries.allocate("new-1");
        let r2 = entries.allocate("new-2");
        let r3 = entries.allocate("new-3");

        // The reused IDs should be the ones that were freed.
        let reused: HashSet<usize> = [r1, r2, r3].iter().map(|id| id.index()).collect();
        assert!(reused.contains(&1) || reused.contains(&3) || reused.contains(&5));
        assert_eq!(reused.len(), 3);

        // All slots should now be occupied.
        for i in 0..6 {
            assert!(
                entries.is_allocated(HeapId::from_index(i)),
                "slot {i} should be occupied"
            );
        }
    }
}
