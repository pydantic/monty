//! Verifies that merely linking `monty-alloc` does not claim to meter memory.

/// Limits require `LimitedAllocator` to be selected by the final executable.
#[test]
fn set_limit_rejects_the_default_allocator() {
    let error = monty_alloc::set_limit(Some(1024), false).unwrap_err();
    assert_eq!(
        error.to_string(),
        "monty-alloc is not installed as the global allocator"
    );
}
