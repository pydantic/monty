use std::{
    cell::{Cell, UnsafeCell},
    fmt,
    mem::MaybeUninit,
};

#[cfg(feature = "memory-model-checks")]
use super::py_dec_ref_ids_for_data;
use crate::heap::{HeapEntry, HeapId, heap_entries::iter::HeapEntriesIter};

/// Number of entries per page. Chosen to balance between wasted memory (from
/// partially-filled last pages) and the frequency of page allocations.
const PAGE_SIZE: usize = 256;

/// A single page of heap entries. Each page is a fixed-size boxed slice of
/// `MaybeUninit` slots — only slots at indices below `HeapEntries::len` are
/// initialized.
type Page = Box<[Slot; PAGE_SIZE]>;
type Slot = MaybeUninit<Option<HeapEntry>>;

/// Paged storage for heap entries that guarantees address stability.
///
/// Entries are stored in fixed-size pages of `MaybeUninit<Option<HeapValue>>`.
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
///   but not the page contents. Any existing `&HeapValue` reference points into a
///   `Box`'s heap allocation, not into the `Vec`'s buffer.
/// - **`free_list`** is only accessed during `allocate` (pop, via `&self`) and
///   `free` (push, via `&mut self`). The borrow checker prevents overlap since
///   `free` requires `&mut self`.
///
/// Index `i` maps to `pages[i / PAGE_SIZE][i % PAGE_SIZE]`.
pub(crate) struct HeapEntries {
    /// Fixed-size pages of heap entries. Each page is heap-allocated once and
    /// never moved, providing address stability for all contained entries.
    /// Wrapped in `UnsafeCell` to allow `allocate(&self)` to append new pages.
    pages: UnsafeCell<Vec<Page>>,
    /// Total number of initialized slots (including freed ones).
    /// Uses `Cell` for interior mutability so `allocate(&self)` can increment.
    len: Cell<usize>,
    /// IDs of freed slots available for reuse. Populated by `free`, consumed by `allocate`.
    /// Wrapped in `UnsafeCell` to allow `allocate(&self)` to pop from the free list.
    free_list: UnsafeCell<Vec<HeapId>>,
}

impl fmt::Debug for HeapEntries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            // SAFETY: (DH) debug formatting never calls `.allocate()`
            .entries(unsafe { HeapEntriesIter::new(self) })
            .finish()
    }
}

impl HeapEntries {
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
            free_list: UnsafeCell::new(Vec::new()),
        }
    }

    /// Returns an exclusive reference to the pages vec.
    #[inline]
    fn pages_mut(&mut self) -> &mut Vec<Page> {
        self.pages.get_mut()
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
    pub fn get(&self, index: usize) -> &HeapEntry {
        // SAFETY: (DH) this call does not expose free slots which could be invalidated
        // by calls to `.allocate()`.
        unsafe { self.get_inner(index) }.expect("HeapEntries::get - data already freed")
    }

    /// Returns a shared reference to the entry at `index`, or `None` if empty
    ///
    /// # Safety
    ///
    /// Callers must not alias borrows from this method to calls to `allocate()`, as `None`
    /// slots can be invalidated by reuse from the freelist.
    #[track_caller]
    unsafe fn get_inner(&self, index: usize) -> Option<&HeapEntry> {
        let len = self.len.get();
        assert!(index < len, "HeapEntries::get: index {index} out of bounds (len={len})");
        // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
        // The slot cannot be mutably borrowed because `get_mut` requires `&mut self`.
        unsafe { (&*self.pages.get())[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_ref() }.as_ref()
    }

    /// Returns a mutable reference to the `Option<HeapValue>` at `index`.
    ///
    /// # Panics
    /// Panics if `index >= len`.
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> &mut Option<HeapEntry> {
        let len = self.len.get();
        assert!(
            index < len,
            "HeapEntries::get_mut: index {index} out of bounds (len={len})",
        );
        // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
        unsafe { self.pages_mut()[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_mut() }
    }

    /// Retain only values satisfying the predicate, freeing the rest.
    pub fn retain(&mut self, mut predicate: impl FnMut(usize, &mut HeapEntry) -> bool) {
        let len = self.len.get();
        for i in 0..len {
            // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
            let slot = unsafe { self.pages_mut()[i / PAGE_SIZE][i % PAGE_SIZE].assume_init_mut() };
            if let Some(value) = slot.as_mut()
                && !predicate(i, value)
            {
                debug_assert_eq!(value.readers.get(), 0);
                *slot = None; // Free the slot by setting it to None
                self.free(HeapId::from_index(i)); // Add the slot ID to the free list
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
    ///   not the page contents. Any existing `&HeapValue` reference points into a
    ///   `Box`'s heap allocation, not into the `Vec`'s buffer.
    pub fn allocate(&self, value: HeapEntry) -> HeapId {
        // SAFETY: (DH) only `&mut` methods will touch the free list, except for this one
        // call site. `HeapEntries` is also not thread-safe, so calls to allocate cannot race.
        // This guarantees this `.pop()` cannot overlap with other operations on the free list.
        let free_id = unsafe { &mut *self.free_list.get() }.pop();
        if let Some(id) = free_id {
            // Reuse a freed slot — the slot was .take()n during dec_ref,
            // so no active borrows can exist on it.
            let index = id.index();
            // SAFETY: (DH) no &mut reference to pages exists (same argument as free_list above).
            // index < len (it was a valid slot before being freed) so the slot is initialized.
            // No active borrows exist on this slot since it was freed.
            let pages = unsafe { &mut *self.pages.get() };
            // SAFETY: see above — freed slot is initialized and has no active borrows.
            unsafe {
                *pages[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_mut() = Some(value);
            }
            id
        } else {
            // No free slots — append a new entry.
            let index = self.len.get();
            let page_idx = index / PAGE_SIZE;
            let slot_idx = index % PAGE_SIZE;

            // SAFETY: (DH) no &mut reference to pages exists (same argument as free_list above).
            let pages = unsafe { &mut *self.pages.get() };
            if page_idx >= pages.len() {
                pages.push(create_page());
            }

            // Write to the new slot. This slot has never been initialized and
            // index == len, so no reader can access it (get() requires index < len).
            pages[page_idx][slot_idx].write(Some(value));
            self.len.set(index + 1);
            HeapId::from_index(index)
        }
    }

    /// Iterates the live values
    pub fn iter(&self) -> impl Iterator<Item = (HeapId, &HeapEntry)> {
        // SAFETY: (DH) iterating only the live entries ensures that caller
        // can never observe `None` entries which could be invalidated by
        // calls to `allocate()`
        unsafe { HeapEntriesIter::new(self) }.filter_map(|(idx, slot)| slot.map(|s| (HeapId::from_index(idx), s)))
    }

    /// Returns a freed slot to the free list for reuse.
    ///
    /// Takes `&mut self` because freeing happens during `dec_ref` and GC,
    /// which genuinely need exclusive access.
    pub fn free(&mut self, id: HeapId) {
        self.free_list.get_mut().push(id);
    }

    /// Tests whether the value at index i is allocated. Panics if `i >= self.len()`
    #[cfg(test)]
    fn is_allocated(&self, index: usize) -> bool {
        // SAFETY: (DH) - call does not expose borrowed data outside of this call
        unsafe { self.get_inner(index) }.is_some()
    }
}

fn create_page() -> Box<[Slot; PAGE_SIZE]> {
    let raw = Box::into_raw(Box::<[Slot]>::new_uninit_slice(PAGE_SIZE));
    // SAFETY: (DH) - allocation is known to be exactly PAGE_SIZE slots
    unsafe { Box::from_raw(raw.cast()) }
}

/// Serializes as a struct with two fields: `entries` (flat vec of all initialized
/// slots) and `free_list` (vec of freed slot IDs). This avoids exposing the
/// internal paged layout in the wire format.
impl serde::Serialize for HeapEntries {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // SAFETY: (DH) serializing the data does not cause allocation
        serializer.collect_seq(unsafe { HeapEntriesIter::new(self) }.map(|(_idx, slot)| slot))
    }
}

impl<'de> serde::Deserialize<'de> for HeapEntries {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries: Vec<Option<HeapEntry>> = Vec::deserialize(deserializer)?;
        let mut this = Self::with_capacity(entries.len());

        // Re-initialize the freelist from none entries
        *this.free_list.get_mut() = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.is_none())
            .map(|(idx, _)| HeapId::from_index(idx))
            .collect();

        // Set the initialized region
        this.len.set(entries.len());

        // Write all pages from the entries vec
        let pages = this.pages_mut();
        for (index, entry) in entries.into_iter().enumerate() {
            let page_idx = index / PAGE_SIZE;
            let slot_idx = index % PAGE_SIZE;
            pages[page_idx][slot_idx].write(entry);
        }
        Ok(this)
    }
}

impl Drop for HeapEntries {
    fn drop(&mut self) {
        let len = self.len.get();
        let pages = self.pages_mut();
        for i in 0..len {
            let slot = &mut pages[i / PAGE_SIZE][i % PAGE_SIZE];
            // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
            unsafe {
                // Mark all contained Objects as Dereferenced before dropping.
                // We use py_dec_ref_ids for this since it handles the marking
                // (we ignore the collected IDs since we're dropping everything anyway).
                #[cfg(feature = "memory-model-checks")]
                if let Some(value) = slot.assume_init_mut() {
                    py_dec_ref_ids_for_data(value.data.0.get_mut(), &mut Vec::new());
                }
                slot.assume_init_drop();
            }
        }
    }
}

/// Place iterator inside a submodule to create a safety boundary on `new` constructor
mod iter {
    use super::{HeapEntries, HeapEntry};

    pub(super) struct HeapEntriesIter<'a> {
        entries: &'a HeapEntries,
        index: usize,
    }

    impl<'a> HeapEntriesIter<'a> {
        /// Safety: (DH) - the caller must ensure that `HeapEntries::allocate()`
        /// is never called for the lifetime `'a` for which this iterator and its
        /// yielded elements exist.
        ///
        /// Allocation may write to `None` entries, which would cause unsafe
        /// aliasing.
        pub unsafe fn new(entries: &'a HeapEntries) -> Self {
            Self { entries, index: 0 }
        }
    }

    impl<'a> Iterator for HeapEntriesIter<'a> {
        type Item = (usize, Option<&'a HeapEntry>);

        fn next(&mut self) -> Option<Self::Item> {
            let current_index = self.index;
            if current_index >= self.entries.len() {
                return None;
            }
            self.index += 1;
            // SAFETY: (DH) - caller guaranteed no aliasing when calling `HeapEntriesIter::new`
            let slot = unsafe { self.entries.get_inner(current_index) };
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
    use crate::heap::{HashState, HeapData, UnsafeHeapData};

    fn dummy(label: &str) -> HeapEntry {
        use crate::types::Str;
        HeapEntry {
            refcount: Cell::new(1),
            data: UnsafeHeapData(UnsafeCell::new(HeapData::Str(Str::new(label.to_owned())))),
            readers: Cell::new(0),
            hash_state: HashState::Unknown,
        }
    }

    #[test]
    fn allocate_while_reference_alive() {
        // Allocate a value, hold a shared reference to it, then allocate
        // another value. The first reference must remain valid.
        let entries = HeapEntries::with_capacity(16);
        let id_a = entries.allocate(dummy("a"));
        let ref_a = entries.get(id_a.index());

        // Allocate while ref_a is live
        let id_b = entries.allocate(dummy("b"));

        // Both references must be readable
        assert!(format!("{ref_a:?}").contains("Str"));
        assert!(format!("{:?}", entries.get(id_b.index())).contains("Str"));
    }

    #[test]
    fn allocate_triggers_new_page_while_reference_alive() {
        // Fill the first page, hold a reference into it, then allocate into a
        // second page. The reference must survive the Vec<Page>::push.
        let entries = HeapEntries::with_capacity(PAGE_SIZE);

        // Fill the first page.
        for i in 0..PAGE_SIZE {
            entries.allocate(dummy(&format!("fill-{i}")));
        }
        assert_eq!(entries.len(), PAGE_SIZE);

        // Hold a reference into the first page.
        let first_ref = entries.get(0);

        // This allocation creates a second page — the pages Vec reallocates
        // its pointer buffer, but Box<Page> contents must not move.
        let overflow_id = entries.allocate(dummy("overflow"));

        // The reference into the first page must still be valid.
        assert!(format!("{first_ref:?}").contains("Str"));
        assert!(format!("{:?}", entries.get(overflow_id.index())).contains("Str"));
    }

    #[test]
    fn free_list_reuse_while_reference_alive() {
        // Allocate three values, free the middle one, then reallocate while
        // holding a reference to a different live slot.
        let mut entries = HeapEntries::with_capacity(16);
        let id_a = entries.allocate(dummy("a"));
        let id_b = entries.allocate(dummy("b"));
        let _id_c = entries.allocate(dummy("c"));

        // Free slot b (simulates dec_ref taking the value and calling free).
        *entries.get_mut(id_b.index()) = None;
        entries.free(id_b);

        // Hold a reference to slot a.
        let ref_a = entries.get(id_a.index());

        // Reallocate into the freed slot while ref_a is live.
        let id_reused = entries.allocate(dummy("reused"));
        assert_eq!(id_reused, id_b); // should reuse the freed slot

        // ref_a must still be valid.
        assert!(format!("{ref_a:?}").contains("Str"));
        // Reused slot has new data.
        assert!(format!("{:?}", entries.get(id_reused.index())).contains("reused"));
    }

    #[test]
    fn multiple_live_references_during_allocation() {
        // Hold references to multiple slots across different pages, then
        // allocate. All references must survive.
        let entries = HeapEntries::with_capacity(PAGE_SIZE * 2);

        // Fill two pages.
        for i in 0..PAGE_SIZE * 2 {
            entries.allocate(dummy(&format!("v-{i}")));
        }

        // Hold references in both pages.
        let ref_first_page = entries.get(0);
        let ref_second_page = entries.get(PAGE_SIZE);

        // Allocate a third page.
        let new_id = entries.allocate(dummy("new"));

        // All references must be readable — verify via Debug output since
        // UnsafeHeapData wraps an UnsafeCell and can't be destructured in patterns.
        assert!(format!("{ref_first_page:?}").contains("Str"));
        assert!(format!("{ref_second_page:?}").contains("Str"));
        assert!(format!("{:?}", entries.get(new_id.index())).contains("Str"));
    }

    #[test]
    fn allocate_into_freed_slot_does_not_alias_other_slots() {
        // Free several slots, then reallocate into them one by one while
        // reading other live slots. Tests that free-list reuse doesn't
        // accidentally alias.
        let mut entries = HeapEntries::with_capacity(16);

        let ids: Vec<_> = (0..8).map(|i| entries.allocate(dummy(&format!("v-{i}")))).collect();

        // Free even-indexed slots.
        for &id in ids.iter().step_by(2) {
            *entries.get_mut(id.index()) = None;
            entries.free(id);
        }

        // Hold references to odd-indexed (live) slots.
        let live_refs: Vec<_> = ids
            .iter()
            .skip(1)
            .step_by(2)
            .map(|id| entries.get(id.index()))
            .collect();

        // Reallocate into freed slots.
        for i in 0..4 {
            entries.allocate(dummy(&format!("realloc-{i}")));
        }

        // All live references must still be valid and unchanged.
        for r in &live_refs {
            assert!(format!("{r:?}").contains("Str"));
        }
    }

    #[test]
    fn retain_then_allocate_reuses_freed_slots() {
        // Use retain to free slots, then allocate into the freed slots.
        let mut entries = HeapEntries::with_capacity(16);

        for i in 0..6 {
            entries.allocate(dummy(&format!("v-{i}")));
        }

        // Retain only even-indexed entries.
        entries.retain(|i, _| i % 2 == 0);

        // Odd slots should now be None.
        for i in [1, 3, 5] {
            assert!(!entries.is_allocated(i));
        }

        // Allocate should reuse freed slots.
        let r1 = entries.allocate(dummy("new-1"));
        let r2 = entries.allocate(dummy("new-2"));
        let r3 = entries.allocate(dummy("new-3"));

        // The reused IDs should be the ones that were freed.
        let reused: HashSet<usize> = [r1, r2, r3].iter().map(|id| id.index()).collect();
        assert!(reused.contains(&1) || reused.contains(&3) || reused.contains(&5));
        assert_eq!(reused.len(), 3);

        // All slots should now be occupied.
        for i in 0..6 {
            // SAFETY: (DH) - borrow only held for `is_none()` check, no overlap with allocation
            assert!(entries.is_allocated(i), "slot {i} should be occupied");
        }
    }
}
