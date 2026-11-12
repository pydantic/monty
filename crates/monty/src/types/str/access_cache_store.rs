//! Per-session access caches for interned Unicode strings.
//!
//! Pages are allocated only for accessed intern slots, so a late ID does not reserve the preceding gap.
//! Append-only storage keeps cache borrows valid while runtime compilation adds strings.

use std::{
    array,
    cell::{Cell, OnceCell, RefCell},
};

use ahash::AHashMap;

use super::{AccessCache, access_cache::Budget};
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
    pub(crate) fn get<B: Budget>(&self, index: usize, budget: &B) -> Option<&AccessCache> {
        if self.0.get().is_none() && !budget.allow_allocation(size_of::<Directory>()) {
            return None;
        }
        let table = self.0.get_or_init(Box::default);
        let page = index / PAGE_SIZE;
        let slot = if let Some((last_page, slot)) = table.last.get()
            && last_page == page
        {
            slot
        } else {
            let mut slots = table.slots.borrow_mut();
            if !slots.contains_key(&page) {
                // Include the cache page, a stable-arena page and conservative directory growth.
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
                if !budget.allow_allocation(bytes) {
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
        if entry.get().is_none() && !budget.allow_allocation(size_of::<AccessCache>()) {
            return None;
        }
        Some(entry.get_or_init(Box::default))
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::{super::access_cache::test_budget::Unlimited, *};

    #[test]
    fn late_unicode_cache_allocates_only_its_page() {
        let store = AccessCacheStore::default();
        let cache = store.get(1_000_000, &Unlimited).unwrap();
        assert_eq!(cache.character("é中", 1, 2, &Unlimited), Ok(Some('中')));
        let table = store.0.get().unwrap();
        assert_eq!(table.pages.len(), 1);
        assert_eq!(table.slots.borrow().len(), 1);
        assert!(!ptr::eq(cache, store.get(1_000_001, &Unlimited).unwrap()));
        assert_eq!(table.pages.len(), 1);
    }

    #[test]
    fn cache_borrows_survive_growth_and_clone_starts_empty() {
        let store = AccessCacheStore::default();
        let original = store.get(3, &Unlimited).unwrap();
        original.byte_offset("aé中🐍", 2, 4, &Unlimited).unwrap();
        for i in 0..512 {
            store.get(i * PAGE_SIZE + 1024, &Unlimited).unwrap();
        }
        assert!(ptr::eq(original, store.get(3, &Unlimited).unwrap()));
        assert!(original.has_cursor());
        assert!(!store.clone().is_initialized());
    }

    struct NoMemory;
    impl Budget for NoMemory {
        type Error = ();
        fn allow_allocation(&self, _: usize) -> bool {
            false
        }
        fn check_time(&self) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn refused_directory_or_cache_allocation_returns_none() {
        let store = AccessCacheStore::default();
        assert!(store.get(0, &NoMemory).is_none());
        assert!(!store.is_initialized());
        store.get(0, &Unlimited).unwrap();
        assert!(store.get(1, &NoMemory).is_none());
        assert!(store.get(PAGE_SIZE, &NoMemory).is_none());
        assert!(store.get(0, &NoMemory).is_some());
    }
}
