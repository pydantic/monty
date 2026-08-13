//! Verifies the tracker's memory checks read a host-installed probe.

use monty_types::{ResourceError, ResourceLimits, ResourceTracker, set_memory_probe};

fn probed_usage(tracker: &ResourceTracker) -> usize {
    match tracker.check_memory_time() {
        Err(ResourceError::Memory { used, .. }) => used,
        other => panic!("expected a memory error from the probe, got {other:?}"),
    }
}

/// One test, since the probe is process-wide and installs exactly once.
#[test]
fn tracker_reads_the_installed_probe() {
    set_memory_probe(|| 2048).unwrap();

    let tracker = ResourceTracker::new(ResourceLimits::default().max_memory(1024));
    assert_eq!(probed_usage(&tracker), 2048);

    set_memory_probe(|| 0).expect_err("a second install is refused");
    assert_eq!(probed_usage(&tracker), 2048, "the first probe stays");
}
