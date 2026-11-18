use std::{
    fs,
    future::pending,
    path::PathBuf,
    process::{self, Child, Command, Stdio},
    thread,
    time::Duration,
};

use super::{Clock, parse_start_time};
use crate::cpu::{self, Failure, Monitor};

/// A stand-in child that idles until the test kills it.
fn sleeper() -> Child {
    Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap()
}

#[test]
fn start_time_is_field_22_even_when_the_command_name_has_spaces_and_parens() {
    let stat = "4242 (a) b (c)) S 1 4242 4242 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 1 0 987654 1000 200";
    assert_eq!(parse_start_time(stat).unwrap(), 987_654);
}

#[test]
fn malformed_stat_is_rejected() {
    assert_eq!(
        parse_start_time("4242 no-parens S 1").unwrap_err().to_string(),
        "malformed /proc stat: no command name"
    );
    assert_eq!(
        parse_start_time("4242 (short) S 1 2").unwrap_err().to_string(),
        "malformed /proc stat: no field 22"
    );
}

/// A `/proc` from another PID namespace would name some other process at the child's PID.
#[test]
fn a_process_that_is_not_our_child_is_refused() {
    let own = process::id();
    assert_eq!(
        Clock::for_child(own, own).err().unwrap().to_string(),
        format!("/proc/{own} is not this process's child; /proc may belong to another PID namespace")
    );
}

#[test]
fn live_child_samples_and_confirms() {
    let mut child = sleeper();
    let clock = Clock::for_child(child.id(), process::id()).unwrap();
    assert!(clock.sample().is_ok());
    assert!(clock.confirm().is_ok());
    child.kill().unwrap();
    child.wait().unwrap();
}

/// Stands in for PID reuse: the PID now names a process whose start time differs from the recorded one.
#[test]
fn a_different_start_time_fails_confirmation() {
    let mut child = sleeper();
    let mut clock = Clock::for_child(child.id(), process::id()).unwrap();
    clock.start_time += 1;
    assert!(clock.sample().is_ok(), "sampling alone does not check identity");
    assert_eq!(clock.confirm().unwrap_err().to_string(), "child has already exited");
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_reaped_child_fails_confirmation() {
    let mut child = sleeper();
    let clock = Clock::for_child(child.id(), process::id()).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(clock.confirm().unwrap_err().to_string(), "child has already exited");
}

/// Only a vanished `/proc` entry means the child exited; other read failures report themselves.
#[test]
fn an_unrelated_proc_read_failure_is_reported_as_itself() {
    let mut child = sleeper();
    let mut clock = Clock::for_child(child.id(), process::id()).unwrap();
    clock.stat_path = PathBuf::from("/proc");
    assert_eq!(clock.confirm().unwrap_err().to_string(), "Is a directory (os error 21)");
    child.kill().unwrap();
    child.wait().unwrap();
}

/// A sample over the limit would kill the worker, so it is confirmed first.
#[tokio::test]
async fn a_crossed_limit_is_confirmed_before_the_worker_is_failed() {
    let mut child = sleeper();
    let clock = Clock::for_child(child.id(), process::id()).unwrap();
    let mut impostor = clock.clone();
    impostor.start_time += 1;

    let genuine = Monitor::new(clock, Duration::ZERO).unwrap();
    let outcome = cpu::run(Some(&genuine), None, pending::<()>()).await;
    assert!(matches!(outcome, Err(Failure::Limit { .. })));

    let reused = Monitor::new(impostor, Duration::ZERO).unwrap();
    let Err(Failure::Clock(error)) = cpu::run(Some(&reused), None, pending::<()>()).await else {
        panic!("an unconfirmed sample over the limit must not fail the worker as a limit");
    };
    assert_eq!(error.to_string(), "child has already exited");
    child.kill().unwrap();
    child.wait().unwrap();
}

/// The final charge to the feed budget is always confirmed, even under the limit.
#[test]
fn the_final_charge_is_confirmed() {
    let mut child = sleeper();
    let mut clock = Clock::for_child(child.id(), process::id()).unwrap();
    clock.start_time += 1;
    let monitor = Monitor::new(clock, Duration::MAX).unwrap();
    assert_eq!(monitor.elapsed().unwrap_err().to_string(), "child has already exited");
    child.kill().unwrap();
    child.wait().unwrap();
}

/// An exited but unreaped child still holds its PID, so its final CPU reading stays valid.
#[test]
fn an_unreaped_zombie_is_still_read() {
    let mut child = sleeper();
    let clock = Clock::for_child(child.id(), process::id()).unwrap();
    child.kill().unwrap();
    // wait for the exit without reaping: the zombie's /proc entry reports state `Z`
    let stat = format!("/proc/{}/stat", child.id());
    while !fs::read_to_string(&stat).unwrap().contains(") Z ") {
        thread::yield_now();
    }
    assert!(clock.sample().is_ok());
    assert!(clock.confirm().is_ok());
    child.wait().unwrap();
}
