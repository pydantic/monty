//! The high-water mark `LimitedAllocator` keeps for `monty_types::peak_memory`.

use std::hint::black_box;

use monty_types::{peak_memory, reset_peak_memory};

#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// Far above anything the test harness allocates on its own.
const BLOCK: usize = 8 * 1024 * 1024;

/// One test rather than several, since libtest's threads would share the
/// process-wide counters.
#[test]
fn the_peak_outlives_freed_memory_until_reset() {
    // no baseline until the allocator is armed, so nothing to measure against
    assert_eq!(peak_memory(), None);
    monty_alloc::set_hard_limit(None).unwrap();

    reset_peak_memory();
    let start = peak_memory().unwrap();
    drop(black_box(vec![1u8; BLOCK]));
    let peak = peak_memory().unwrap();
    assert!(peak >= start + BLOCK, "{peak} < {start} + {BLOCK}");

    // a reset forgets the freed block
    reset_peak_memory();
    let reset = peak_memory().unwrap();
    assert!(reset < BLOCK, "{reset}");

    // growth through `realloc` is charged too
    let mut grown = Vec::new();
    for byte in (0..=u8::MAX).cycle().take(BLOCK) {
        grown.push(byte);
    }
    drop(black_box(grown));
    let peak = peak_memory().unwrap();
    assert!(peak >= reset + BLOCK, "{peak} < {reset} + {BLOCK}");
}
