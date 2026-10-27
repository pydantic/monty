//! Execution-window accounting with sampled thread CPU time and an elapsed-time fallback.

#[cfg(any(unix, windows))]
use std::thread::{ThreadId, current};
use std::time::Duration;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

#[cfg(any(unix, windows))]
use cpu_time::ThreadTime;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

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
    thread: ThreadId,
}

impl ExecutionInstant {
    /// Capture a fresh baseline on the executing thread, including after a resume or restore.
    pub(super) fn now(remaining: Option<Duration>) -> Self {
        let cpu = thread_cpu();
        let wall = Instant::now();
        Self {
            wall,
            cpu,
            sampled_at: wall,
            sample_interval: remaining.map_or(SAMPLE_INTERVAL, |budget| budget.min(SAMPLE_INTERVAL)),
            last_elapsed: Duration::ZERO,
            #[cfg(any(unix, windows))]
            thread: current().id(),
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
        if current().id() != self.thread {
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
fn thread_cpu() -> Option<Duration> {
    #[cfg(any(unix, windows))]
    {
        ThreadTime::try_now().ok().map(|time| time.as_duration())
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[cfg(all(test, any(unix, windows)))]
#[path = "../../tests/unit/execution_clock.rs"]
mod tests;
