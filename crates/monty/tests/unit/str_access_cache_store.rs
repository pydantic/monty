use std::ptr;

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
    for i in 0..1000 {
        assert!(original.character(&text, (i * 7919) % 16384).is_some());
    }
    assert!(original.has_cursor());
    for index in 1024..2048 {
        store.get(index * PAGE_SIZE).char_len("é中");
    }
    assert!(ptr::eq(original, store.get(3)));
    assert!(original.has_cursor());
    assert!(store.clone().0.get().is_none());
}
