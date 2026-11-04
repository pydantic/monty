use std::{
    array,
    cell::{Cell, OnceCell, RefCell},
};

use ahash::AHashMap;

use super::AccessCache;
use crate::heap::{HeapId, StableHeap};

const PAGE_SIZE: usize = 256;
type AccessCachePage = [OnceCell<Box<AccessCache>>; PAGE_SIZE];

/// Lazy caches keyed by intern slot; ASCII-only sessions allocate no cache pages.
/// Directory remain at stable addresses while runtime compilation adds strings.
#[derive(Debug, Default)]
pub(crate) struct AccessCacheStore(OnceCell<Box<Directory>>);

/// Sparse directory entries avoid reserving the gap before a late Unicode intern slot.
#[derive(Debug)]
struct Directory {
    slots: RefCell<AHashMap<usize, HeapId>>,
    // Directory are append-only: no arena slot is freed while a cache may be borrowed.
    pages: StableHeap<Box<AccessCachePage>>,
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
#[path = "../../../tests/unit/str_access_cache_store.rs"]
mod tests;
