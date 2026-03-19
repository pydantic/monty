use std::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
};

use crate::heap::{HeapId, HeapValue};

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
    /// Shows summary info (counts) rather than full page/free-list contents,
    /// which would be extremely verbose for any non-trivial heap.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeapEntries")
            .field("len", &self.len.get())
            .field("pages", &self.pages().len())
            .field("free_list_len", &self.free_list().len())
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
    /// # Safety contract
    /// Safe to call anytime — we only need exclusive access when mutating
    /// through `pages_mut`, and the borrow checker on `&self`/`&mut self`
    /// methods prevents those from overlapping.
    #[inline]
    fn pages(&self) -> &Vec<Page> {
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

    /// Returns a shared reference to the free list.
    #[inline]
    fn free_list(&self) -> &Vec<HeapId> {
        // SAFETY: same argument as pages() — no &mut reference exists.
        unsafe { &*self.free_list.get() }
    }

    /// Returns an exclusive reference to the free list.
    #[inline]
    fn free_list_mut(&mut self) -> &mut Vec<HeapId> {
        self.free_list.get_mut()
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
        let len = self.len.get();
        assert!(index < len, "HeapEntries::get: index {index} out of bounds (len={len})",);
        // SAFETY: all slots at indices < self.len have been initialized via `push`
        // or `allocate`. The pages vec is not mutated by this call.
        unsafe { self.pages()[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_ref() }.as_ref()
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
        // SAFETY: all slots at indices < self.len have been initialized via `push`
        // or `allocate`.
        unsafe { self.pages_mut()[index / PAGE_SIZE][index % PAGE_SIZE].assume_init_mut() }
    }

    /// Appends a new entry via `&mut self` and returns its index.
    ///
    /// Allocates a new page if the current one is full.
    fn push(&mut self, value: Option<HeapValue>) -> usize {
        let index = self.len.get();
        let page_idx = index / PAGE_SIZE;
        let slot_idx = index % PAGE_SIZE;

        let pages = self.pages_mut();
        if page_idx >= pages.len() {
            // Allocate a new page WITHOUT initializing — only slots written via
            // `push` will be initialized, avoiding the cost of zeroing the whole page.
            pages.push(Box::new_uninit_slice(PAGE_SIZE));
        }

        pages[page_idx][slot_idx].write(value);
        self.len.set(index + 1);
        index
    }

    /// Iterates over all initialized entries as shared references.
    pub fn iter(&self) -> impl Iterator<Item = &Option<HeapValue>> {
        let len = self.len.get();
        self.pages()
            .iter()
            .flat_map(|page| page.iter())
            .take(len)
            // SAFETY: all slots at indices < self.len have been initialized via `push`
            // or `allocate`.
            .map(|slot| unsafe { slot.assume_init_ref() })
    }

    /// Iterates over all initialized entries as mutable references.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Option<HeapValue>> {
        let len = self.len.get();
        self.pages_mut()
            .iter_mut()
            .flat_map(|page| page.iter_mut())
            .take(len)
            // SAFETY: all slots at indices < self.len have been initialized via `push`
            // or `allocate`.
            .map(|slot| unsafe { slot.assume_init_mut() })
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
        // SAFETY: no &mut reference to free_list exists (this method takes &self,
        // and free() takes &mut self — the borrow checker prevents overlap).
        let free_list = unsafe { &mut *self.free_list.get() };
        if let Some(id) = free_list.pop() {
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

    /// Returns a freed slot to the free list for reuse.
    ///
    /// Takes `&mut self` because freeing happens during `dec_ref` and GC,
    /// which genuinely need exclusive access.
    pub fn free(&mut self, id: HeapId) {
        self.free_list_mut().push(id);
    }

    /// Reconstructs `HeapEntries` from flat vectors (used during deserialization).
    ///
    /// Each entry from the vec is pushed into paged storage, preserving indices.
    pub fn from_vecs(entries: Vec<Option<HeapValue>>, free_list: Vec<HeapId>) -> Self {
        let mut this = Self::with_capacity(entries.len());
        for entry in entries {
            this.push(entry);
        }
        *this.free_list_mut() = free_list;
        this
    }

    /// Returns a slice of the free list for serialization.
    pub fn free_list_slice(&self) -> &[HeapId] {
        self.free_list()
    }
}

impl Drop for HeapEntries {
    fn drop(&mut self) {
        let len = self.len.get();
        let pages = self.pages_mut();
        for i in 0..len {
            // SAFETY: all slots at indices < self.len have been initialized via `push`
            // or `allocate`.
            unsafe {
                pages[i / PAGE_SIZE][i % PAGE_SIZE].assume_init_drop();
            }
        }
    }
}
