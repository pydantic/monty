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
type Slot<T> = MaybeUninit<HeapSlot<T>>;

/// One initialized slot: its payload plus the generation that invalidates
/// pointers minted while an earlier payload occupied the slot.
///
/// `generation` is bumped by [`stable_heap_entry::StableHeapEntry::free`], the
/// only free path, so a [`HeapPtr`](super::HeapPtr) held across a free and a
/// later reuse is detectable even though the page memory stays valid. It is not
/// persisted: a restored heap starts every generation at zero, and no pointer
/// survives a dump. The counter wraps, so detection is best-effort after 2^32
/// frees of the same slot.
pub(crate) struct HeapSlot<T> {
    /// Bumped on every free; compared against a `HeapPtr`'s recorded value.
    pub(crate) generation: u32,
    /// The payload, `None` while the slot sits on the free list.
    pub(crate) entry: Option<T>,
}

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
        let slot = unsafe { self.slot_at(id) };
        let Some(slot) = slot else {
            panic!("StableHeap::get - {id:?} out of bounds");
        };
        slot.entry.as_ref().expect("HeapEntries::get - data already freed")
    }

    /// Returns a mutable handle to the entry at `id`, which can free it.
    ///
    /// Derives the handle through `pages.get_mut()`: the `&mut self` receiver
    /// yields `Unique` provenance, which is what lets
    /// [`StableHeapEntry::free`] take the payload and bump the slot generation.
    /// It also means an arena write pops the borrow tags `slot_at` handed out
    /// for that page, so a [`HeapPtr`](super::HeapPtr) must not be dereferenced
    /// after one — see its documentation.
    ///
    /// # Panics
    /// Panics if `index >= len`.
    #[inline]
    #[track_caller]
    pub fn entry(&mut self, id: HeapId) -> Option<StableHeapEntry<'_, T>> {
        assert!(id.index() < self.len.get(), "StableHeap::entry - {id:?} out of bounds");
        let (page_idx, slot_idx) = Self::page_slot_indices(id);
        let pages = self.pages.get_mut();
        let slot = &mut pages[page_idx][slot_idx];
        // SAFETY: [DH] - all slots at indices < self.len have been initialized via `allocate`.
        let slot = unsafe { slot.assume_init_mut() };
        StableHeapEntry::new(id, slot, &mut self.free_list)
    }

    /// Allocates a slot — reusing from the free list or appending — and returns its ID.
    ///
    /// Takes `&self` instead of `&mut self`, enabling allocation while holding shared
    /// borrows to other heap entries. This is the core operation that makes
    /// `Heap::allocate(&self)` possible.
    pub fn allocate(&self, value: T) -> HeapId {
        self.allocate_with_slot(value).0
    }

    /// Allocates a value and also returns its stable arena slot.
    ///
    /// The slot lets the heap immediately project a newly allocated payload
    /// without repeating the page and slot lookup.
    pub fn allocate_with_slot(&self, value: T) -> (HeapId, &HeapSlot<T>) {
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
        let (id, generation) = if let Some(id) = self.free_list.pop() {
            let (page_idx, slot_idx) = Self::page_slot_indices(id);
            // SAFETY: [DH] - previously freed slots must have previously been initialized.
            let slot = unsafe { pages[page_idx][slot_idx].assume_init_ref() };
            debug_assert!(
                slot.entry.is_none(),
                "allocate - popped free slot {id:?} is not actually free"
            );
            // The generation bumped by `free` stays: a pointer minted while the
            // previous payload occupied this slot must never resolve again.
            (id, slot.generation)
        } else {
            let id = HeapId::from_index(self.len.get());
            let page_idx = id.index() / PAGE_SIZE;
            if page_idx >= pages.len() {
                pages.push(create_page());
            }
            self.len.set(id.index() + 1);
            (id, 0)
        };

        // Write the whole slot. No need to worry about the previous value's
        // destructor - either the slot was free (its `entry` is `None` and has
        // nothing to drop), or the slot is not initialized yet.
        let (page_idx, slot_idx) = Self::page_slot_indices(id);
        let slot = pages[page_idx][slot_idx].write(HeapSlot {
            generation,
            entry: Some(value),
        });

        (id, slot)
    }

    /// Iterates the live values
    #[cfg(any(feature = "ref-count-return", test))]
    pub fn iter(&self) -> impl Iterator<Item = (HeapId, &T)> {
        // SAFETY: [DH] - iterating only the live entries ensures that caller
        // can never observe `None` entries which could be invalidated by
        // calls to `allocate()`. Entries can only be freed by `&mut self` methods.
        unsafe { HeapEntriesIter::new(self) }.filter_map(|(idx, slot)| slot.map(|s| (HeapId::from_index(idx), s)))
    }

    /// Accesses the slot at `id` with an immutable borrow on self. Returns `None` if the slot
    /// is out of bounds.
    ///
    /// # Safety:
    /// - Caller must ensure that a reference to a freed slot's `None` entry does
    ///   not overlap with any call to `allocate()`, which would reuse the slot.
    #[track_caller]
    pub unsafe fn slot_at(&self, id: HeapId) -> Option<&HeapSlot<T>> {
        if id.index() >= self.len.get() {
            return None;
        }
        let (page_idx, slot_idx) = Self::page_slot_indices(id);
        // SAFETY: [DH] - modification to pages Vec only occur in allocate, which only writes to `None` entries
        // and doesn't cause reallocation of the boxed contents of existing pages, so temporary read of `pages` here is ok.
        //
        // TODO: use `UnsafeCell::as_ref_unchecked` when stable.
        let pages = unsafe { self.pages.get().as_ref_unchecked() };
        let slot = &pages[page_idx][slot_idx];
        // SAFETY: [DH] - all slots at indices < self.len have been initialized via `allocate`.
        Some(unsafe { slot.assume_init_ref() })
    }

    fn page_slot_indices(id: HeapId) -> (usize, usize) {
        let index = id.index();
        (index / PAGE_SIZE, index % PAGE_SIZE)
    }
}

/// Allocates a new page of uninitialized slots directly on the heap.
fn create_page<T>() -> Box<[Slot<T>; PAGE_SIZE]> {
    let raw = Box::into_raw(Box::<[Slot<T>]>::new_uninit_slice(PAGE_SIZE)).cast();
    // SAFETY: [DH] - allocation is known to be exactly PAGE_SIZE slots, so
    // the cast-ed pointer can still be used with `Box::from_raw`
    unsafe { Box::from_raw(raw) }
}

/// Submodule for `StableHeapEntry` to help enforce a safety boundary around the `new` constructor.
mod stable_heap_entry {
    use std::ops::{Deref, DerefMut};

    use crate::heap::{HeapId, free_list::FreeList, stable_heap::HeapSlot};

    pub struct StableHeapEntry<'a, T> {
        id: HeapId,
        slot: &'a mut HeapSlot<T>,
        free_list: &'a mut FreeList,
    }

    impl<'a, T> StableHeapEntry<'a, T> {
        // Only way to construct a `StableHeapEntry`. Returns `None` if the slot is already freed.
        pub fn new(id: HeapId, slot: &'a mut HeapSlot<T>, free_list: &'a mut FreeList) -> Option<Self> {
            slot.entry.is_some().then_some(Self { id, slot, free_list })
        }

        pub fn free(self) -> T {
            self.free_list.push(self.id);
            // Bump before taking the payload: any `HeapPtr` minted while this slot
            // held the old payload must be detectable after `allocate` reuses it.
            self.slot.generation = self.slot.generation.wrapping_add(1);
            // SAFETY: [DH] - impossible to get a `StableHeapEntry` with `None` slot - `new`
            // is the only constructor
            unsafe { self.slot.entry.take().unwrap_unchecked() }
        }

        pub fn get(&self) -> &T {
            // SAFETY: [DH] - impossible to get a `StableHeapEntry` with `None` slot - `new`
            // is the only constructor
            unsafe { self.slot.entry.as_ref().unwrap_unchecked() }
        }

        pub fn get_mut(&mut self) -> &mut T {
            // SAFETY: [DH] - impossible to get a `StableHeapEntry` with `None` slot - `new`
            // is the only constructor
            unsafe { self.slot.entry.as_mut().unwrap_unchecked() }
        }
    }

    impl<T> Deref for StableHeapEntry<'_, T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            self.get()
        }
    }

    impl<T> DerefMut for StableHeapEntry<'_, T> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.get_mut()
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

        // Write all pages from the entries vec. Generations restart at zero: they
        // are diagnostic only, and no `HeapPtr` survives a dump.
        let pages = this.pages.get_mut();
        for (index, entry) in entries.into_iter().enumerate() {
            let (page_idx, slot_idx) = Self::page_slot_indices(HeapId::from_index(index));
            pages[page_idx][slot_idx].write(HeapSlot { generation: 0, entry });
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
            // SAFETY: [DH] - caller guaranteed no aliasing when calling `HeapEntriesIter::new`
            let slot = unsafe { self.entries.slot_at(HeapId::from_index(current_index)) }?;
            self.index += 1;
            Some((current_index, slot.entry.as_ref()))
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            let remaining = self.entries.len().saturating_sub(self.index);
            (remaining, Some(remaining))
        }
    }
}

#[cfg(test)]
mod tests {

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
    fn free_then_reuse_bumps_the_slot_generation() {
        let mut entries = StableHeap::with_capacity(4);
        let first = entries.allocate("a");
        // SAFETY: no allocation happens while this reference is live.
        let generation = unsafe { entries.slot_at(first) }.expect("allocated").generation;

        assert_eq!(entries.entry(first).expect("live").free(), "a");
        let second = entries.allocate("b");
        assert_eq!(second, first, "the free list reuses the freed slot");

        // SAFETY: no allocation happens while this reference is live.
        let slot = unsafe { entries.slot_at(second) }.expect("allocated");
        assert_eq!(slot.entry, Some("b"));
        assert_eq!(slot.generation, generation.wrapping_add(1));
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
}
