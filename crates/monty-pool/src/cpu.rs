//! Native child-process CPU supervision, independent of worker telemetry.

use std::{future::Future, io, pin::pin, time::Duration};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use tokio::time::sleep;
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout_at};

use crate::cpu_clock::Clock;
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

pub(crate) struct Monitor {
    clock: Clock,
    started: Duration,
    pub(crate) limit: Duration,
}

pub(crate) enum Failure {
    Timeout,
    Limit { limit: Duration, consumed: Duration },
    Clock(io::Error),
}

impl Monitor {
    pub(crate) fn new(clock: Clock, limit: Duration) -> io::Result<Self> {
        let started = clock.read()?;
        Ok(Self { clock, started, limit })
    }

    pub(crate) fn elapsed(&self) -> io::Result<Duration> {
        self.clock
            .read()?
            .checked_sub(self.started)
            .ok_or_else(|| io::Error::other("child CPU clock moved backwards"))
    }

    fn check(&self) -> Result<(), Failure> {
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
    monitor: Option<&Monitor>,
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

async fn until_exhausted(monitor: &Monitor, driver: Option<&Driver>) -> Result<(), Failure> {
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
                .expired(monitor.clock.id, deadline)
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
    feed_grace: Option<Duration>,
    turn_grace: Option<Duration>,
) -> Option<Duration> {
    let feed = feed
        .zip(feed_grace)
        .map(|(limit, grace)| limit.saturating_add(grace).saturating_sub(consumed));
    let turn = turn.zip(turn_grace).map(|(limit, grace)| limit.saturating_add(grace));
    match (feed, turn) {
        (Some(feed), Some(turn)) => Some(feed.min(turn)),
        (feed, turn) => feed.or(turn),
    }
}

#[cfg(test)]
#[path = "../tests/unit/cpu.rs"]
mod tests;
