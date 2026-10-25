use std::time::Duration;

use monty_types::{DEFAULT_GC_INTERVAL, ResourceLimits, ResourceTracker};

/// Checks every restored allowance against both stricter and looser destination policies.
#[test]
fn restored_limits_only_tighten() {
    let limits = ResourceLimits::default()
        .max_memory(1024)
        .max_feed_duration(Duration::from_secs(4))
        .max_turn_duration(Duration::from_secs(3))
        .max_total_sleep(Duration::from_secs(2))
        .max_recursion_depth(20)
        .max_suspensions(10)
        .gc_interval(100);
    let stricter = ResourceLimits::default()
        .max_memory(512)
        .max_feed_duration(Duration::from_secs(2))
        .max_turn_duration(Duration::from_secs(1))
        .max_total_sleep(Duration::from_secs(1))
        .max_recursion_depth(10)
        .max_suspensions(5)
        .gc_interval(50);
    let mut tracker = ResourceTracker::new(limits.clone());
    tracker.on_execution_start();
    tracker.on_execution_stop();
    let elapsed = (tracker.elapsed(), tracker.feed_elapsed(), tracker.turn_elapsed());
    tracker.tighten_limits(&stricter);
    tracker.tighten_limits(&limits);
    tracker.tighten_limits(&ResourceLimits::default());
    assert_eq!(tracker.max_memory(), Some(512));
    assert_eq!(tracker.max_feed_duration(), Some(Duration::from_secs(2)));
    assert_eq!(tracker.max_turn_duration(), Some(Duration::from_secs(1)));
    assert_eq!(tracker.max_total_sleep(), Some(Duration::from_secs(1)));
    assert_eq!(tracker.max_suspensions(), 5);
    assert_eq!(tracker.gc_interval(), Some(50));
    assert!(tracker.check_recursion_depth(9).is_ok());
    assert!(tracker.check_recursion_depth(10).is_err());
    assert_eq!(
        (tracker.elapsed(), tracker.feed_elapsed(), tracker.turn_elapsed()),
        elapsed
    );
}

/// An unlimited snapshot inherits destination limits instead of disabling them.
#[test]
fn unlimited_restored_limits_inherit_the_destination_policy() {
    let mut tracker = ResourceTracker::default();
    let ceiling = ResourceLimits::default()
        .max_memory(512)
        .max_feed_duration(Duration::from_secs(2))
        .max_turn_duration(Duration::from_secs(1))
        .max_total_sleep(Duration::from_secs(1))
        .gc_interval(50);
    tracker.tighten_limits(&ceiling);
    assert_eq!(tracker.max_memory(), Some(512));
    assert_eq!(tracker.max_feed_duration(), Some(Duration::from_secs(2)));
    assert_eq!(tracker.max_turn_duration(), Some(Duration::from_secs(1)));
    assert_eq!(tracker.max_total_sleep(), Some(Duration::from_secs(1)));
    assert_eq!(tracker.gc_interval(), Some(50));
}

/// An unset `gc_interval` means the default interval, so it caps a looser snapshot
/// interval, and a looser destination interval does not loosen an unset snapshot's.
#[test]
fn unset_gc_interval_is_the_default_not_unlimited() {
    let mut tracker = ResourceTracker::new(ResourceLimits::default().gc_interval(DEFAULT_GC_INTERVAL * 10));
    tracker.tighten_limits(&ResourceLimits::default());
    assert_eq!(tracker.gc_interval(), Some(DEFAULT_GC_INTERVAL));

    let mut tracker = ResourceTracker::default();
    tracker.tighten_limits(&ResourceLimits::default().gc_interval(DEFAULT_GC_INTERVAL * 10));
    assert_eq!(tracker.gc_interval(), Some(DEFAULT_GC_INTERVAL));

    let mut tracker = ResourceTracker::default();
    tracker.tighten_limits(&ResourceLimits::default());
    assert_eq!(tracker.gc_interval(), None);
}
