//! The work clock in the interpreter: bytecode and native operations charge work, waits do not.
//!
//! These tests run without `monty-alloc`, so allocation charges nothing here; the subprocess
//! tests cover that floor.

use std::{thread, time::Duration};

use monty::MontyRepl;
use monty_types::{
    CompileOptions, ExcType, ExecutionClock, MontyException, MontyObject, NANOS_PER_UNIT, PrintWriter, ResourceLimits,
    ResourceTracker,
};

const LOOP: &str = "x = 0\nfor i in range(10_000):\n    x = x + i\nx";

/// A session on the work clock, with an optional feed budget.
fn work_repl(max_feed_duration: Option<Duration>) -> MontyRepl {
    let limits = match max_feed_duration {
        Some(limit) => ResourceLimits::default().max_feed_duration(limit),
        None => ResourceLimits::default(),
    };
    let tracker = ResourceTracker::new(limits).with_execution_clock(ExecutionClock::Work);
    MontyRepl::new("test.py", tracker, CompileOptions::default())
}

fn feed(repl: &mut MontyRepl, code: &str) -> Result<MontyObject, MontyException> {
    repl.feed_run(code, vec![], PrintWriter::Stdout)
}

/// Units the session has been charged since it started.
fn units(repl: &MontyRepl) -> u64 {
    u64::try_from(repl.tracker().elapsed().as_nanos()).unwrap() / NANOS_PER_UNIT
}

#[test]
fn the_same_code_costs_the_same() {
    let costs: Vec<u64> = (0..3)
        .map(|_| {
            let mut repl = work_repl(None);
            assert_eq!(feed(&mut repl, LOOP).unwrap(), MontyObject::int(49_995_000));
            units(&repl)
        })
        .collect();
    assert_eq!(costs[0], costs[1]);
    assert_eq!(costs[0], costs[2]);
    // a handful of instructions per iteration
    assert!((30_000..200_000).contains(&costs[0]), "{} units", costs[0]);
}

#[test]
fn a_run_shorter_than_the_checkpoint_interval_is_charged() {
    let mut repl = work_repl(None);
    assert_eq!(feed(&mut repl, "1 + 1").unwrap(), MontyObject::int(2));
    let first = units(&repl);
    assert!(first > 0);
    assert_eq!(repl.tracker().feed_elapsed(), repl.tracker().elapsed());
    feed(&mut repl, "2 + 2").unwrap();
    assert_eq!(units(&repl), first * 2);
}

#[test]
fn feed_limit_raises_the_usual_timeout_and_the_session_continues() {
    let mut repl = work_repl(Some(Duration::from_millis(1)));
    let error = feed(&mut repl, "x = 0\nfor i in range(10**12):\n    x = x + 1").unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    let message = error.message().unwrap();
    assert!(
        message.starts_with("feed time limit exceeded: 1.00") && message.ends_with("ms > 1ms"),
        "{message}"
    );
    // detected at the first checkpoint past the budget
    let overshoot = repl
        .tracker()
        .feed_elapsed()
        .checked_sub(Duration::from_millis(1))
        .unwrap();
    assert!(overshoot < Duration::from_nanos(600 * NANOS_PER_UNIT), "{overshoot:?}");
    assert_eq!(feed(&mut repl, "x > 0").unwrap(), MontyObject::bool(true));
}

#[test]
fn a_budget_above_the_cost_does_not_fire() {
    let mut repl = work_repl(None);
    feed(&mut repl, LOOP).unwrap();
    let cost = repl.tracker().elapsed();

    let mut repl = work_repl(Some(cost));
    feed(&mut repl, LOOP).unwrap();
    let mut repl = work_repl(Some(cost / 2));
    assert_eq!(feed(&mut repl, LOOP).unwrap_err().exc_type(), ExcType::TimeoutError);
}

#[test]
fn waiting_on_the_host_charges_nothing() {
    let mut waited = Vec::new();
    for wait in [Duration::ZERO, Duration::from_millis(20)] {
        let repl = work_repl(Some(Duration::from_millis(1)));
        let call = repl
            .feed_start("host() + 1", vec![], PrintWriter::Stdout)
            .map_err(|error| error.error)
            .unwrap()
            .into_function_call()
            .expect("a host call");
        thread::sleep(wait);
        let (repl, value) = call
            .resume(MontyObject::int(41), PrintWriter::Stdout)
            .map_err(|error| error.error)
            .unwrap()
            .into_complete()
            .expect("completion");
        assert_eq!(value, MontyObject::int(42));
        waited.push(repl.tracker().elapsed());
    }
    assert_eq!(waited[0], waited[1]);
    assert!(waited[0] > Duration::ZERO);
}

#[test]
fn native_operations_charge_work() {
    let mut repl = work_repl(None);
    feed(&mut repl, "values = list(range(200_000, 0, -1))").unwrap();
    repl.tracker_mut().set_max_feed_duration(Duration::from_millis(1));
    for code in [
        "sorted(values)",
        "sum(values)",
        "values.index(-1)",
        "[1] * 100_000_000 == []",
    ] {
        let error = feed(&mut repl, code).unwrap_err();
        assert_eq!(error.exc_type(), ExcType::TimeoutError, "{code}");
        repl.tracker_mut().set_max_feed_duration(Duration::from_millis(1));
    }
}
