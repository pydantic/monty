//! The work clock at the tracker: charged units become execution time without a clock read.

use std::{thread, time::Duration};

use monty_types::{ExecutionClock, NANOS_PER_UNIT, ResourceError, ResourceLimits, ResourceTracker, TimeLimitScope};

/// A tracker on the work clock with an optional feed budget in units.
fn work_tracker(feed_units: Option<u64>) -> ResourceTracker {
    let limits = match feed_units {
        Some(units) => ResourceLimits::default().max_feed_duration(units_time(units)),
        None => ResourceLimits::default(),
    };
    ResourceTracker::new(limits).with_execution_clock(ExecutionClock::Work)
}

fn units_time(units: u64) -> Duration {
    Duration::from_nanos(units * NANOS_PER_UNIT)
}

#[test]
fn charged_units_convert_at_the_fixed_rate() {
    let tracker = work_tracker(None);
    tracker.on_execution_start();
    tracker.charge_work(1000);
    assert_eq!(tracker.elapsed(), units_time(1000));
    tracker.charge_work(500);
    tracker.on_execution_stop();
    assert_eq!(tracker.elapsed(), units_time(1500));
    assert_eq!(tracker.feed_elapsed(), units_time(1500));
    assert_eq!(tracker.turn_elapsed(), units_time(1500));
}

#[test]
fn elapsed_time_is_not_charged() {
    let tracker = work_tracker(Some(1));
    tracker.on_execution_start();
    thread::sleep(Duration::from_millis(5));
    tracker.checkpoint(0).unwrap();
    tracker.on_execution_stop();
    assert_eq!(tracker.elapsed(), Duration::ZERO);
}

#[test]
fn work_between_windows_is_dropped() {
    let tracker = work_tracker(None);
    tracker.charge_work(1000);
    tracker.on_execution_start();
    tracker.charge_work(1);
    tracker.on_execution_stop();
    tracker.charge_work(1000);
    assert_eq!(tracker.elapsed(), units_time(1));
}

#[test]
fn feed_and_turn_budgets_reset_independently() {
    let tracker = work_tracker(None);
    tracker.on_feed_start();
    tracker.on_execution_start();
    tracker.charge_work(10);
    tracker.on_execution_stop();
    tracker.on_turn_start();
    tracker.on_execution_start();
    tracker.charge_work(5);
    tracker.on_execution_stop();
    assert_eq!(tracker.feed_elapsed(), units_time(15));
    assert_eq!(tracker.turn_elapsed(), units_time(5));
    tracker.on_feed_start();
    assert_eq!(tracker.feed_elapsed(), Duration::ZERO);
    assert_eq!(tracker.elapsed(), units_time(15));
}

#[test]
fn checkpoint_raises_once_the_budget_is_spent() {
    let tracker = work_tracker(Some(1000));
    tracker.on_execution_start();
    tracker.checkpoint(1000).unwrap();
    let error = tracker.checkpoint(1).unwrap_err();
    let ResourceError::Time { scope, limit, elapsed } = error else {
        panic!("expected a time error, got {error}");
    };
    assert_eq!(scope, TimeLimitScope::Feed);
    assert_eq!(limit, units_time(1000));
    assert_eq!(elapsed, units_time(1001));
}

#[test]
fn native_polls_charge_a_loop_interval() {
    let interval = ResourceTracker::LOOP_CHECK_INTERVAL as u64;
    let tracker = work_tracker(None);
    tracker.on_execution_start();
    for i in 0..ResourceTracker::LOOP_CHECK_INTERVAL * 3 {
        tracker.check_time_every(i).unwrap();
    }
    assert_eq!(tracker.elapsed(), units_time(interval * 3));
    tracker.check_time().unwrap();
    tracker.check_memory_time().unwrap();
    assert_eq!(tracker.elapsed(), units_time(interval * 5));
}

#[test]
fn other_clocks_ignore_charges() {
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_secs(60));
    let tracker = ResourceTracker::new(limits);
    tracker.on_execution_start();
    tracker.charge_work(u64::MAX);
    tracker.checkpoint(u64::MAX).unwrap();
    tracker.on_execution_stop();
    assert!(tracker.elapsed() < Duration::from_secs(60));
}

#[test]
fn clock_is_available_and_serialized_by_name() {
    assert!(ExecutionClock::Work.is_available());
    let tracker = work_tracker(None);
    assert_eq!(tracker.execution_clock(), ExecutionClock::Work);
    assert_eq!(serde_json::to_string(&ExecutionClock::Work).unwrap(), "\"work\"");
}
