//! OS CPU counter for one local child, independent of worker telemetry.
//!
//! CPU supervision is Linux-only: `nix` reads another process's CPU clock without `unsafe`.
//! Other platforms report the clock as unsupported, and `Pool::new` rejects CPU mode there.

#[cfg(not(target_os = "linux"))]
use std::convert::Infallible;
use std::{io, time::Duration};

#[cfg(target_os = "linux")]
use nix::{time::ClockId, unistd::Pid};
use tokio::process::Child;

/// The child's process CPU clock: user and system time summed over all its threads.
pub(crate) struct Clock {
    #[cfg(target_os = "linux")]
    id: ClockId,
    /// Off Linux no clock can be built, so `read` is unreachable.
    #[cfg(not(target_os = "linux"))]
    never: Infallible,
}

impl Clock {
    // The checkout retains the child; final sampling is skipped after teardown removes it.
    pub(crate) fn new(child: &Child) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let pid = i32::try_from(child.id().ok_or_else(child_exited)?).map_err(io::Error::other)?;
            let id = ClockId::pid_cpu_clock_id(Pid::from_raw(pid)).map_err(io::Error::from)?;
            Ok(Self { id })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = child;
            Err(unsupported())
        }
    }

    pub(crate) fn read(&self) -> io::Result<Duration> {
        #[cfg(target_os = "linux")]
        {
            let time = self.id.now().map_err(io::Error::from)?;
            Ok(Duration::new(
                u64::try_from(time.tv_sec()).map_err(io::Error::other)?,
                u32::try_from(time.tv_nsec()).map_err(io::Error::other)?,
            ))
        }
        #[cfg(not(target_os = "linux"))]
        match self.never {}
    }
}

#[cfg(target_os = "linux")]
fn child_exited() -> io::Error {
    io::Error::other("child has already exited")
}

#[cfg(not(target_os = "linux"))]
fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "child CPU clock is only available on Linux")
}
