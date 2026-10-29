use std::{thread, time::Duration};

use super::{ExecutionClock, ExecutionInstant};

#[test]
fn cached_sample_preserves_the_last_reported_duration() {
    let mut window = ExecutionInstant::now(ExecutionClock::Cpu, None);
    window.cpu = Some(Duration::ZERO);
    window.thread = Some(thread::current().id());
    window.sample_interval = Duration::from_mins(1);
    window.last_elapsed = Duration::from_nanos(42);
    assert_eq!(window.elapsed(), Duration::from_nanos(42));
}

#[test]
fn unavailable_counter_keeps_the_monotonic_elapsed_floor() {
    let mut window = ExecutionInstant::now(ExecutionClock::Cpu, None);
    window.cpu = None;
    window.last_elapsed = Duration::from_mins(1);
    assert_eq!(window.elapsed(), Duration::from_mins(1));
    assert_eq!(window.elapsed_at_stop(), Duration::from_mins(1));
    assert!(window.cpu.is_none());
}

#[test]
fn reversed_counter_latches_elapsed_fallback() {
    let mut window = ExecutionInstant::now(ExecutionClock::Cpu, None);
    window.cpu = Some(Duration::MAX);
    window.elapsed_at_stop();
    assert!(window.cpu.is_none());
    let before = window.last_elapsed;
    assert!(window.elapsed_at_stop() >= before);
    assert!(window.cpu.is_none());
}

#[test]
fn regressed_sample_cannot_refund_consumed_time() {
    let mut window = ExecutionInstant::now(ExecutionClock::Cpu, None);
    window.cpu = Some(Duration::ZERO);
    window.last_elapsed = Duration::MAX;
    assert_eq!(window.elapsed_at_stop(), Duration::MAX);
    assert!(window.cpu.is_none());
}

#[test]
fn wall_mode_never_arms_the_cpu_counter() {
    let mut window = ExecutionInstant::now(ExecutionClock::Wall, None);
    assert!(window.cpu.is_none());
    thread::sleep(Duration::from_millis(10));
    assert!(window.elapsed_at_stop() >= Duration::from_millis(10));
    assert!(window.cpu.is_none());
}

#[test]
fn clock_availability_matches_the_execution_feature() {
    assert!(ExecutionClock::Wall.is_available());
    assert_eq!(ExecutionClock::Cpu.is_available(), cfg!(feature = "cpu-clock"));
}
