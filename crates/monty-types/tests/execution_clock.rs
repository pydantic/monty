//! Native execution-clock regressions independent of interpreter dispatch speed.
#![cfg(all(feature = "cpu-clock", target_os = "linux"))]

use std::{
    hint::black_box,
    thread,
    time::{Duration, Instant},
};

use cpu_time::ThreadTime;
use monty_types::{ExecutionClock, ResourceError, ResourceLimits, ResourceTracker, TimeLimitScope};

/// Spend measured CPU rather than assuming a particular machine's instruction rate.
fn burn_cpu(duration: Duration) {
    let started = ThreadTime::now();
    while started.elapsed() < duration {
        for value in 0..1000 {
            black_box(value * value);
        }
    }
}

#[test]
fn parked_thread_excludes_elapsed_time_and_other_threads_cpu() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(50)))
        .with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    thread::spawn(|| burn_cpu(Duration::from_millis(100))).join().unwrap();
    tracker.on_execution_stop();
    assert!(
        tracker.elapsed() < Duration::from_millis(50),
        "charged a parked thread: {:?}",
        tracker.elapsed()
    );
    tracker.check_time().unwrap();
}

#[test]
fn execution_boundary_charges_cpu_without_polling() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(20)))
        .with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    burn_cpu(Duration::from_millis(40));
    tracker.on_execution_stop();
    assert!(tracker.elapsed() >= Duration::from_millis(40));
    assert!(matches!(
        tracker.check_time(),
        Err(ResourceError::Time {
            scope: TimeLimitScope::Feed,
            ..
        })
    ));
}

/// A restored feed keeps its consumption while a new turn gets a fresh thread baseline.
#[test]
fn restored_feed_accumulates_cpu_across_threads_and_host_work() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(50)))
        .with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    burn_cpu(Duration::from_millis(30));
    tracker.on_execution_stop();
    let consumed = tracker.elapsed();
    let serialized = serde_json::to_vec(&tracker).unwrap();
    thread::spawn(move || {
        burn_cpu(Duration::from_millis(80));
        let restored: ResourceTracker = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(restored.elapsed(), consumed);
        assert_eq!(restored.feed_elapsed(), consumed);
        assert_eq!(restored.turn_elapsed(), Duration::ZERO);
        restored.on_turn_start();
        let measured = ThreadTime::now();
        restored.on_execution_start();
        burn_cpu(Duration::from_millis(30));
        restored.on_execution_stop();
        assert!(restored.feed_elapsed() >= Duration::from_millis(60));
        assert!(restored.feed_elapsed() <= consumed + measured.elapsed());
        assert!(matches!(
            restored.check_time(),
            Err(ResourceError::Time {
                scope: TimeLimitScope::Feed,
                ..
            })
        ));
        restored.on_feed_start();
        assert_eq!(restored.feed_elapsed(), Duration::ZERO);
        assert_eq!(restored.turn_elapsed(), Duration::ZERO);
        assert!(restored.elapsed() >= consumed);
    })
    .join()
    .unwrap();
}

/// Moving an idle tracker cannot subtract another thread's absolute CPU counter.
#[test]
fn idle_tracker_resumes_on_a_different_thread() {
    let tracker = ResourceTracker::new(ResourceLimits::default()).with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    burn_cpu(Duration::from_millis(20));
    tracker.on_execution_stop();
    let before = tracker.elapsed();
    thread::spawn(move || {
        burn_cpu(Duration::from_millis(80));
        let measured = ThreadTime::now();
        tracker.on_execution_start();
        burn_cpu(Duration::from_millis(20));
        tracker.on_execution_stop();
        assert!(tracker.elapsed() >= before + Duration::from_millis(20));
        assert!(tracker.elapsed() <= before + measured.elapsed());
    })
    .join()
    .unwrap();
}

#[test]
fn cooperative_checks_enforce_cpu_limit() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_turn_duration(Duration::from_millis(20)))
        .with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    let deadline = Instant::now();
    loop {
        if tracker.check_time().is_err() {
            break;
        }
        assert!(deadline.elapsed() < Duration::from_secs(10), "CPU limit did not fire");
        black_box(black_box(13) * black_box(17));
    }
    tracker.on_execution_stop();
    assert!(tracker.turn_elapsed() > Duration::from_millis(20));
}

/// An active window moved to another thread must not subtract unrelated CPU counters.
#[test]
fn active_thread_change_uses_conservative_elapsed_time() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(20)))
        .with_execution_clock(ExecutionClock::Cpu);
    tracker.on_execution_start();
    let tracker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        assert!(matches!(tracker.check_time(), Err(ResourceError::Time { .. })));
        tracker
    })
    .join()
    .unwrap();
    assert!(matches!(tracker.check_time(), Err(ResourceError::Time { .. })));
    tracker.on_execution_stop();
    assert!(tracker.elapsed() >= Duration::from_millis(40));
}

/// Many short windows still accumulate CPU when every window ends before the polling interval.
#[test]
fn short_execution_windows_charge_their_final_cpu_sample() {
    let tracker = ResourceTracker::new(ResourceLimits::default()).with_execution_clock(ExecutionClock::Cpu);
    for _ in 0..20 {
        tracker.on_execution_start();
        burn_cpu(Duration::from_micros(50));
        tracker.on_execution_stop();
    }
    assert!(tracker.elapsed() >= Duration::from_millis(1));
}

#[test]
fn snapshot_clock_is_explicit_and_older_snapshots_default_to_wall() {
    let tracker = ResourceTracker::new(ResourceLimits::default()).with_execution_clock(ExecutionClock::Cpu);
    let mut saved = serde_json::to_value(tracker).unwrap();
    let restored: ResourceTracker = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(restored.execution_clock(), ExecutionClock::Cpu);
    saved.as_object_mut().unwrap().remove("execution_clock");
    let legacy: ResourceTracker = serde_json::from_value(saved).unwrap();
    assert_eq!(legacy.execution_clock(), ExecutionClock::Wall);
    assert_eq!(ResourceTracker::default().execution_clock(), ExecutionClock::Wall);
}
