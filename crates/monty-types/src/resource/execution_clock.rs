//! Execution-window accounting with sampled thread CPU time and an elapsed-time fallback.

#[cfg(any(unix, windows))]
use std::thread::{ThreadId, current};
use std::time::Duration;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

#[cfg(all(feature = "cpu-clock", unix))]
use nix::{libc::CLOCK_THREAD_CPUTIME_ID, time::ClockId};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;
#[cfg(all(feature = "cpu-clock", windows))]
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::Threading::{GetCurrentThread, GetThreadTimes},
};

use super::ExecutionClock;

/// Maximum interval between cooperative CPU samples. Boundaries always read directly.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(1);

/// One synchronous VM window. Only accumulated durations survive serialization.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExecutionInstant {
    wall: Instant,
    cpu: Option<Duration>,
    sampled_at: Instant,
    sample_interval: Duration,
    last_elapsed: Duration,
    #[cfg(any(unix, windows))]
    thread: Option<ThreadId>,
}

impl ExecutionInstant {
    /// Capture a fresh baseline on the executing thread, including after a resume or restore.
    pub(super) fn now(clock: ExecutionClock, remaining: Option<Duration>) -> Self {
        let cpu = (clock == ExecutionClock::Cpu).then(thread_cpu).flatten();
        let wall = Instant::now();
        Self {
            wall,
            cpu,
            sampled_at: wall,
            sample_interval: remaining.map_or(SAMPLE_INTERVAL, |budget| budget.min(SAMPLE_INTERVAL)),
            last_elapsed: Duration::ZERO,
            #[cfg(any(unix, windows))]
            thread: cpu.map(|_| current().id()),
        }
    }

    /// Reuse this window's sample until its polling interval expires.
    pub(super) fn elapsed(&mut self) -> Duration {
        self.measure(false)
    }

    /// Include work since the last cooperative sample when the VM returns to its host.
    pub(super) fn elapsed_at_stop(&mut self) -> Duration {
        self.measure(true)
    }

    /// A failed, reversed, or cross-thread clock uses elapsed time for the rest of this window.
    fn measure(&mut self, exact: bool) -> Duration {
        #[cfg(any(unix, windows))]
        if self.cpu.is_some() && Some(current().id()) != self.thread {
            self.cpu = None;
        }
        if let Some(started) = self.cpu {
            if !exact && self.sampled_at.elapsed() < self.sample_interval {
                return self.last_elapsed;
            }
            if let Some(elapsed) = thread_cpu().and_then(|now| now.checked_sub(started))
                && elapsed >= self.last_elapsed
            {
                self.sampled_at = Instant::now();
                self.last_elapsed = elapsed;
                return elapsed;
            }
            self.cpu = None;
        }
        self.last_elapsed = self.last_elapsed.max(self.wall.elapsed());
        self.last_elapsed
    }
}

/// Read an absolute counter without retaining a thread-bound handle in movable VM state.
#[cfg_attr(
    all(feature = "cpu-clock", windows),
    expect(
        unsafe_code,
        reason = "the current-thread pseudo-handle and separate initialized FILETIME outputs stay live"
    )
)]
pub(super) fn thread_cpu() -> Option<Duration> {
    #[cfg(all(feature = "cpu-clock", unix))]
    {
        let time = ClockId::from_raw(CLOCK_THREAD_CPUTIME_ID).now().ok()?;
        Some(Duration::new(
            time.tv_sec().try_into().ok()?,
            time.tv_nsec().try_into().ok()?,
        ))
    }
    #[cfg(all(feature = "cpu-clock", windows))]
    {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the pseudo-handle names this thread; each output is a separate live FILETIME.
        let status = unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        };
        if status == 0 {
            return None;
        }
        let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        ticks(kernel)
            .checked_add(ticks(user))?
            .checked_mul(100)
            .map(Duration::from_nanos)
    }
    #[cfg(not(all(feature = "cpu-clock", any(unix, windows))))]
    {
        None
    }
}

#[cfg(all(test, any(unix, windows)))]
#[path = "../../tests/unit/execution_clock.rs"]
mod tests;
