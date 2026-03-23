use std::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
};

use crate::heap::{HeapId, HeapValue, heap_entries::iter::HeapEntriesIter};
#[cfg(feature = "ref-count-panic")]
use crate::heap_traits::HeapItem;

/// Number of entries per page. Chosen to balance between wasted memory (from
/// partially-filled last pages) and the frequency of page allocations.
const PAGE_SIZE: usize = 256;

/// A single page of heap entries. Each page is a fixed-size boxed slice of
/// `MaybeUninit` slots — only slots at indices below `HeapEntries::len` are
/// initialized.
type Page = Box<[MaybeUninit<Option<HeapValue>>]>;

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

#[expect(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for HeapEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
            pages.push(Box::new_uninit_slice(PAGE_SIZE));
        }
        Self {
            pages: UnsafeCell::new(pages),
            len: Cell::new(0),
            free_list: UnsafeCell::new(Vec::new()),
        }
    }

    /// Returns a shared reference to the pages vec.
    ///
    /// # Safety
    ///
    /// Safe to call anytime — we only need exclusive access when mutating
    /// through `pages_mut`, and the borrow checker on `&self`/`&mut self`
    /// methods prevents those from overlapping.
    #[inline]
    unsafe fn pages(&self) -> &Vec<Page> {
        // SAFETY: no &mut reference to pages exists. Callers that mutate
        // (allocate, push, get_mut, iter_mut, free) either take &mut self
        // (preventing any concurrent &self call) or are allocate(&self)
        // which never holds a reference across the mutation point.
        unsafe { &*self.pages.get() }
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

    /// Returns a shared reference to the entry at `index`, or `None` if the slot is freed.
    ///
    /// # Panics
    /// Panics if `index >= len`.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&HeapValue> {
        self.get_inner(index).as_ref()
    }

    fn get_inner(&self, index: usize) -> &Option<HeapValue> {
        let len = self.len.get();
        assert!(index < len, "HeapEntries::get: index {index} out of bounds (len={len})",);
        // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
        // The slot cannot be mutably borrowed because `get_mut` requires `&mut self`.
        unsafe { self.pages()[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_ref() }
    }

    /// Returns a mutable reference to the `Option<HeapValue>` at `index`.
    ///
    /// # Panics
    /// Panics if `index >= len`.
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> &mut Option<HeapValue> {
        let len = self.len.get();
        assert!(
            index < len,
            "HeapEntries::get_mut: index {index} out of bounds (len={len})",
        );
        // SAFETY: (DH) all slots at indices < self.len have been initialized via `allocate`.
        unsafe { self.pages_mut()[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_mut() }
    }

    /// Retain only values satisfying the predicate, freeing the rest.
    pub fn retain(&mut self, mut predicate: impl FnMut(usize, &mut HeapValue) -> bool) {
        let len = self.len.get();
        for i in 0..len {
            // SAFETY: all slots at indices < self.len have been initialized via `push`
            // or `allocate`.
            let slot = unsafe { self.pages_mut()[i / PAGE_SIZE][i % PAGE_SIZE].assume_init_mut() };
            if let Some(value) = slot.as_mut() {
                if !predicate(i, value) {
                    *slot = None; // Free the slot by setting it to None
                    self.free(HeapId::from_index(i)); // Add the slot ID to the free list
                }
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
    pub fn allocate(&self, value: HeapValue) -> HeapId {
        // SAFETY: (DH) only `&mut` methods will touch the free list, except for this one
        // call site. `HeapEntries` is also not thread-safe, so calls to allocate cannot race.
        // This guarantees this `.pop()` cannot overlap with other operations on the free list.
        let free_id = unsafe { &mut *self.free_list.get() }.pop();
        if let Some(id) = free_id {
            // Reuse a freed slot — the slot was .take()n during dec_ref,
            // so no active borrows can exist on it.
            let index = id.index();
            // SAFETY: no &mut reference to pages exists (same argument as free_list above).
            // index < len (it was a valid slot before being freed) so the slot is initialized.
            // No active borrows exist on this slot since it was freed (.take()n in dec_ref).
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

            // SAFETY: no &mut reference to pages exists (same argument as free_list above).
            let pages = unsafe { &mut *self.pages.get() };
            if page_idx >= pages.len() {
                pages.push(Box::new_uninit_slice(PAGE_SIZE));
            }

            // Write to the new slot. This slot has never been initialized and
            // index == len, so no reader can access it (get() requires index < len).
            pages[page_idx][slot_idx].write(Some(value));
            self.len.set(index + 1);
            HeapId::from_index(index)
        }
    }

    /// Iterates the live values
    #[cfg(feature = "ref-count-return")]
    pub fn iter(&self) -> impl Iterator<Item = &HeapValue> {
        // SAFETY: (DH) iterating only the live entries ensures that caller
        // can never observe `None` entries which could be invalidated by
        // calls to `allocate()`
        unsafe { HeapEntriesIter::new(self) }.filter_map(|(_idx, slot)| slot.as_ref())
    }

    /// Returns a freed slot to the free list for reuse.
    ///
    /// Takes `&mut self` because freeing happens during `dec_ref` and GC,
    /// which genuinely need exclusive access.
    pub fn free(&mut self, id: HeapId) {
        self.free_list.get_mut().push(id)
    }
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
        let entries: Vec<Option<HeapValue>> = Vec::deserialize(deserializer)?;
        let mut this = Self::with_capacity(entries.len());

        // Re-initialize the freelist from none entries
        *this.free_list.get_mut() = entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| entry.is_none().then(|| HeapId::from_index(idx)))
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
            // SAFETY: all slots at indices < self.len have been initialized via `push`
            // or `allocate`.
            unsafe {
                // Mark all contained Objects as Dereferenced before dropping.
                // We use py_dec_ref_ids for this since it handles the marking
                // (we ignore the collected IDs since we're dropping everything anyway).
                #[cfg(feature = "ref-count-panic")]
                if let Some(value) = slot.assume_init_mut() {
                    if let Some(data) = &mut value.data {
                        data.py_dec_ref_ids(&mut Vec::new())
                    }
                }
                slot.assume_init_drop();
            }
        }
    }
}

/// Place iterator inside a submodule to create a safety boundary on `new` constructor
mod iter {
    use super::*;

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
        type Item = (usize, &'a Option<HeapValue>);

        fn next(&mut self) -> Option<Self::Item> {
            let current_index = self.index;
            if current_index >= self.entries.len() {
                return None;
            }
            self.index += 1;
            Some((current_index, self.entries.get_inner(current_index)))
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            let remaining = self.entries.len().saturating_sub(self.index);
            (remaining, Some(remaining))
        }
    }
}

#[cfg(test)]
mod tests {}
