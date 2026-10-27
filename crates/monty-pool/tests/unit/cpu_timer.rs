use std::{fs, future::Future, io, sync::Arc, task::Poll, time::Duration};

use futures_util::future::{join_all, poll_fn};
use nix::{
    sys::signal::{SigSet, Signal},
    time::ClockId,
    unistd::Pid,
};
use tokio::{
    process::Command,
    time::{sleep, timeout},
};

use super::Driver;

fn clock(pid: u32) -> ClockId {
    ClockId::pid_cpu_clock_id(Pid::from_raw(pid.try_into().unwrap())).unwrap()
}

fn now(clock: ClockId) -> Duration {
    let value = clock.now().unwrap();
    Duration::new(value.tv_sec().try_into().unwrap(), value.tv_nsec().try_into().unwrap())
}

#[tokio::test]
async fn coalesced_signals_deliver_every_simultaneous_expiration() {
    let driver = Driver::new().unwrap();
    let mut child = Command::new("python3")
        .args(["-c", "while True: pass"])
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let clock = clock(child.id().unwrap());
    let deadline = now(clock) + Duration::from_millis(50);
    let results = timeout(
        Duration::from_secs(2),
        join_all((0..64).map(|_| driver.expired(clock, deadline))),
    )
    .await
    .unwrap();
    assert!(results.into_iter().all(|r| r.is_ok()));
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancelled_expirations_cannot_notify_a_different_registration() {
    let driver = Driver::new().unwrap();
    let mut busy = Command::new("python3")
        .args(["-c", "while True: pass"])
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut blocked = Command::new("sleep").arg("60").kill_on_drop(true).spawn().unwrap();
    let busy_clock = clock(busy.id().unwrap());
    let blocked_clock = clock(blocked.id().unwrap());
    for _ in 0..100 {
        let mut old = Box::pin(driver.expired(busy_clock, now(busy_clock)));
        poll_fn(|cx| {
            let _ = old.as_mut().poll(cx);
            Poll::Ready(())
        })
        .await;
        drop(old);
        let new_deadline = now(blocked_clock) + Duration::from_secs(10);
        assert!(
            timeout(Duration::from_millis(2), driver.expired(blocked_clock, new_deadline))
                .await
                .is_err()
        );
    }
    busy.kill().await.unwrap();
    blocked.kill().await.unwrap();
}

#[tokio::test]
async fn cancellation_deletes_the_kernel_timer() {
    match fs::read_to_string("/proc/self/timers") {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("kernel timer inspection is unavailable; cancellation behavior has separate tests");
            return;
        }
        Err(error) => panic!("cannot inspect kernel timers: {error}"),
    }
    let driver = Driver::new().unwrap();
    let mut child = Command::new("sleep").arg("60").kill_on_drop(true).spawn().unwrap();
    let clock = clock(child.id().unwrap());
    let exists = || {
        let raw = clock.as_raw().to_string();
        fs::read_to_string("/proc/self/timers").unwrap().lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some("ClockID:") && fields.next() == Some(raw.as_str())
        })
    };
    let mut pending = Box::pin(driver.expired(clock, now(clock) + Duration::from_secs(60)));
    poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    timeout(Duration::from_secs(2), async {
        while !exists() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    drop(pending);
    timeout(Duration::from_secs(2), async {
        while exists() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    child.kill().await.unwrap();
}

#[test]
fn oversized_deadline_is_rejected_before_kernel_registration() {
    let error = super::arm(ClockId::CLOCK_MONOTONIC, Duration::MAX).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn pool_driver_preserves_the_callers_signal_mask_and_shuts_down() {
    let before = SigSet::thread_get_mask().unwrap();
    let driver = Driver::new().unwrap();
    let after = SigSet::thread_get_mask().unwrap();
    assert_eq!(before.contains(Signal::SIGUSR1), after.contains(Signal::SIGUSR1));
    let endpoint = Arc::downgrade(&driver.0);
    drop(driver);
    assert!(endpoint.upgrade().is_none());
}
