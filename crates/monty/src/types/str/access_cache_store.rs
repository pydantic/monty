//! Per-session access caches for interned Unicode strings.
//!
//! Pages are allocated only for accessed intern slots, so a late ID does not reserve the preceding gap.
//! Append-only storage keeps cache borrows valid while runtime compilation adds strings.

use std::{
    array,
    cell::{Cell, OnceCell, RefCell},
};

use ahash::AHashMap;

use super::AccessCache;
use crate::heap::{HeapId, StableHeap};

const PAGE_SIZE: usize = 256;

/// Lazily populated cache slots for one contiguous range of intern IDs.
type AccessCachePage = [OnceCell<Box<AccessCache>>; PAGE_SIZE];

/// Lazy caches keyed by intern slot; ASCII-only sessions allocate no cache pages.
/// Cache addresses remain stable while runtime compilation adds strings; cloning creates an empty store.
#[derive(Debug, Default)]
pub(crate) struct AccessCacheStore(OnceCell<Box<Directory>>);

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
    pub(crate) fn is_initialized(&self) -> bool {
        self.0.get().is_some()
    }

    /// Returns the cache for an intern slot, allocating its page and cache shell on first access.
    /// The caller filters ASCII and reserved strings; returned borrows survive later insertions.
    pub(crate) fn get(&self, index: usize) -> &AccessCache {
        let table = self.0.get_or_init(Box::default);
        let page = index / PAGE_SIZE;
        let slot = if let Some((last_page, slot)) = table.last.get()
            && last_page == page
        {
            slot
        } else {
            let slot = *table
                .slots
                .borrow_mut()
                .entry(page)
                .or_insert_with(|| table.pages.allocate(Box::new(array::from_fn(|_| OnceCell::new()))));
            table.last.set(Some((page, slot)));
            slot
        };
        // Cache borrows come from stable pages, not the directory's temporary RefCell borrow.
        table.pages.get(slot)[index % PAGE_SIZE].get_or_init(Box::default)
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use monty_types::ResourceTracker;

    use super::*;

    #[test]
    fn late_unicode_cache_allocates_only_its_page() {
        let store = AccessCacheStore::default();
        store.get(1_000_000).char_len("é中");
        let table = store.0.get().unwrap();
        assert_eq!(table.pages.len(), 1);
        assert_eq!(
            table
                .pages
                .get(HeapId::from_index(0))
                .iter()
                .filter(|entry| entry.get().is_some())
                .count(),
            1
        );
        assert_eq!(table.slots.borrow().len(), 1);
        assert_eq!(store.get(1_000_001).char_len("é中🐍"), 3);
        assert_eq!(store.get(1_000_000).char_len("é中"), 2);
        assert_eq!(table.pages.len(), 1);
        assert!(!ptr::eq(store.get(1_000_000), store.get(1_000_001)));
    }

    #[test]
    fn unicode_cache_survives_table_growth_and_resets_on_clone() {
        let store = AccessCacheStore::default();
        let text = "aé中🐍".repeat(4096);
        let original = store.get(3);
        let tracker = ResourceTracker::default();
        for i in 0..1000 {
            original.character(&text, (i * 7919) % 16384, &tracker).unwrap();
        }
        assert!(original.has_checkpoints());
        for index in 1024..2048 {
            store.get(index * PAGE_SIZE).char_len("é中");
        }
        assert!(ptr::eq(original, store.get(3)));
        assert!(original.has_checkpoints());
        assert!(store.clone().0.get().is_none());
    }
}
