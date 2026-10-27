//! Linux child-process CPU supervision, independent of worker telemetry.

use std::{future::Future, io, pin::pin, time::Duration};

#[cfg(target_os = "linux")]
use nix::{time::ClockId, unistd::Pid};
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use tokio::time::sleep;
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout_at};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use crate::cpu_timer::Driver as KernelDriver;

#[derive(Clone)]
pub(crate) struct Driver {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    kernel: KernelDriver,
}

impl Driver {
    #[cfg_attr(
        not(all(target_os = "linux", target_env = "gnu")),
        expect(clippy::unnecessary_wraps, reason = "the GNU timer constructor can fail")
    )]
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self {
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            kernel: KernelDriver::new()?,
        })
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone, Copy)]
pub(crate) struct Monitor {
    #[cfg(target_os = "linux")]
    clock: ClockId,
    started: Duration,
    pub(crate) limit: Duration,
}

pub(crate) enum Failure {
    Timeout,
    Limit { limit: Duration, consumed: Duration },
    Clock(io::Error),
}

impl Monitor {
    pub(crate) fn new(pid: u32, limit: Duration) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let pid = i32::try_from(pid).map_err(io::Error::other)?;
            let clock = ClockId::pid_cpu_clock_id(Pid::from_raw(pid)).map_err(io::Error::from)?;
            let mut monitor = Self {
                clock,
                started: Duration::ZERO,
                limit,
            };
            monitor.started = monitor.read()?;
            Ok(monitor)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (pid, limit);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "child CPU supervision requires Linux",
            ))
        }
    }

    fn read(self) -> io::Result<Duration> {
        #[cfg(target_os = "linux")]
        {
            let time = self.clock.now().map_err(io::Error::from)?;
            let seconds = u64::try_from(time.tv_sec()).map_err(io::Error::other)?;
            let nanos = u32::try_from(time.tv_nsec()).map_err(io::Error::other)?;
            Ok(Duration::new(seconds, nanos))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = self;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "child CPU supervision requires Linux",
            ))
        }
    }

    pub(crate) fn elapsed(self) -> io::Result<Duration> {
        self.read()?
            .checked_sub(self.started)
            .ok_or_else(|| io::Error::other("child CPU clock moved backwards"))
    }

    fn check(self) -> Result<(), Failure> {
        let consumed = self.elapsed().map_err(Failure::Clock)?;
        if consumed >= self.limit {
            Err(Failure::Limit {
                limit: self.limit,
                consumed,
            })
        } else {
            Ok(())
        }
    }
}

/// Bounds either protocol path with independent CPU and wall limits. Cancellation drops the I/O future.
pub(crate) async fn run<F: Future>(
    monitor: Option<Monitor>,
    driver: Option<&Driver>,
    deadline: Option<Instant>,
    future: F,
) -> Result<F::Output, Failure> {
    let supervised = async {
        let Some(monitor) = monitor else {
            return Ok(future.await);
        };
        monitor.check()?;
        let mut future = pin!(future);
        let expiry = until_exhausted(monitor, driver);
        let mut expiry = pin!(expiry);
        tokio::select! {
            biased;
            result = &mut expiry => result.and_then(|()| Err(Failure::Clock(io::Error::other("CPU monitor ended without exhaustion")))),
            result = &mut future => Ok(result),
        }
    };
    match deadline {
        Some(deadline) => timeout_at(deadline, supervised).await.map_err(|_| Failure::Timeout)?,
        None => supervised.await,
    }
}

async fn until_exhausted(monitor: Monitor, driver: Option<&Driver>) -> Result<(), Failure> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if let Some(driver) = driver {
        // Short replies need only boundary samples, not cross-thread timer registration.
        sleep(POLL_INTERVAL).await;
        loop {
            monitor.check()?;
            let deadline = monitor
                .started
                .checked_add(monitor.limit)
                .ok_or_else(|| Failure::Clock(io::Error::new(io::ErrorKind::InvalidInput, "CPU deadline overflow")))?;
            driver
                .kernel
                .expired(monitor.clock, deadline)
                .await
                .map_err(Failure::Clock)?;
        }
    }
    let _ = driver;
    let mut ticks = interval_at(Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        monitor.check()?;
    }
}

/// The grace is one capped allowance per feed, not a fresh grant on every resume.
pub(crate) fn allowance(
    feed: Option<Duration>,
    turn: Option<Duration>,
    consumed: Duration,
    grace: Duration,
) -> Option<Duration> {
    let feed = feed.map(|limit| limit.saturating_add(grace).saturating_sub(consumed));
    let turn = turn.map(|limit| limit.saturating_add(grace));
    match (feed, turn) {
        (Some(feed), Some(turn)) => Some(feed.min(turn)),
        (feed, turn) => feed.or(turn),
    }
}

#[cfg(test)]
#[path = "../tests/unit/cpu.rs"]
mod tests;
