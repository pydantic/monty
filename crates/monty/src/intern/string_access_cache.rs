//! Lazy storage for the cursor caches of interned strings.

use std::{
    array,
    cell::{Cell, OnceCell, RefCell},
};

use ahash::AHashMap;
use monty_types::ResourceTracker;

use crate::{
    heap::{HeapId, StableHeap},
    types::str::AccessCache,
};

const PAGE_SIZE: usize = 256;

/// Lazily populated cache slots for one contiguous range of intern IDs.
/// Boxing individual caches avoids initializing their cursor state for untouched slots.
type AccessCachePage = [OnceCell<Box<AccessCache>>; PAGE_SIZE];

/// Allocates pages on demand for interned strings that use Unicode cursors.
/// Cache addresses remain stable while runtime compilation adds strings; cloning creates an empty store.
#[derive(Debug, Default)]
pub(super) struct AccessCacheStore(OnceCell<Box<Directory>>);

/// Sparse directory entries avoid reserving the gap before a late Unicode intern slot.
#[derive(Debug)]
struct Directory {
    /// Logical page number to its stable arena slot; absent pages allocate nothing.
    slots: RefCell<AHashMap<usize, HeapId>>,
    /// Append-only pages: no arena slot is freed while a cache may be borrowed.
    pages: StableHeap<Box<AccessCachePage>>,
    /// Last logical page and arena slot, avoiding a hash lookup for nearby intern IDs.
    last: Cell<Option<(usize, HeapId)>>,
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            slots: RefCell::default(),
            pages: StableHeap::with_capacity(0),
            last: Cell::new(None),
        }
    }
}

impl Clone for AccessCacheStore {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl AccessCacheStore {
    #[cfg(test)]
    pub(super) fn is_initialized(&self) -> bool {
        self.0.get().is_some()
    }

    /// Returns the cache for an intern slot, allocating its page and cache shell on first access.
    /// The caller filters ASCII and reserved strings; returned borrows survive later insertions.
    /// Refused allocations return `None` so the caller can scan directly without retaining state.
    pub(super) fn get(&self, index: usize, tracker: &ResourceTracker) -> Option<&AccessCache> {
        if self.0.get().is_none() && tracker.check_allocation(size_of::<Directory>()).is_err() {
            return None;
        }
        let table = self.0.get_or_init(Box::default);
        let page = index / PAGE_SIZE;
        // Most accesses reuse a logical page; avoid borrowing and probing the directory in that case.
        let slot = if let Some((last_page, slot)) = table.last.get()
            && last_page == page
        {
            slot
        } else {
            let mut slots = table.slots.borrow_mut();
            if !slots.contains_key(&page) {
                // Preflight the cache page, possible stable-arena page, and directory/pointer growth.
                // Conservative headroom can skip a cache early; lookup correctness never depends on it.
                let directory = slots
                    .capacity()
                    .saturating_add(1)
                    .saturating_mul(8 * size_of::<(usize, HeapId)>());
                let page_pointers = (table.pages.len() / PAGE_SIZE + 1).saturating_mul(4 * size_of::<usize>());
                let bytes = size_of::<AccessCachePage>()
                    .saturating_mul(2)
                    .saturating_add(directory)
                    .saturating_add(page_pointers)
                    .saturating_add(128);
                if tracker.check_allocation(bytes).is_err() {
                    return None;
                }
            }
            let slot = *slots
                .entry(page)
                .or_insert_with(|| table.pages.allocate(Box::new(array::from_fn(|_| OnceCell::new()))));
            table.last.set(Some((page, slot)));
            slot
        };
        // Cache borrows come from stable pages, not the directory's temporary RefCell borrow.
        let entry = &table.pages.get(slot)[index % PAGE_SIZE];
        if entry.get().is_none() && tracker.check_allocation(size_of::<AccessCache>()).is_err() {
            return None;
        }
        Some(entry.get_or_init(Box::default))
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use monty_types::ResourceLimits;

    use super::*;

    #[test]
    fn late_unicode_cache_allocates_only_its_page() {
        let store = AccessCacheStore::default();
        let tracker = ResourceTracker::default();
        let cache = store.get(1_000_000, &tracker).unwrap();
        let table = store.0.get().unwrap();
        assert_eq!(table.pages.len(), 1);
        assert_eq!(table.slots.borrow().len(), 1);
        assert!(!ptr::eq(cache, store.get(1_000_001, &tracker).unwrap()));
        assert_eq!(table.pages.len(), 1);
    }

    #[test]
    fn cache_borrows_survive_growth_and_clone_starts_empty() {
        let store = AccessCacheStore::default();
        let tracker = ResourceTracker::default();
        let original = store.get(3, &tracker).unwrap();
        for i in 0..512 {
            store.get(i * PAGE_SIZE + 1024, &tracker).unwrap();
        }
        assert!(ptr::eq(original, store.get(3, &tracker).unwrap()));
        assert!(!store.clone().is_initialized());
    }

    #[test]
    fn refused_directory_or_cache_allocation_returns_none() {
        let store = AccessCacheStore::default();
        let tracker = ResourceTracker::default();
        let no_memory = ResourceTracker::new(ResourceLimits::default().max_memory(0));
        assert!(store.get(0, &no_memory).is_none());
        assert!(!store.is_initialized());
        store.get(0, &tracker).unwrap();
        assert!(store.get(1, &no_memory).is_none());
        assert!(store.get(PAGE_SIZE, &no_memory).is_none());
        assert!(store.get(0, &no_memory).is_some());
    }
}
