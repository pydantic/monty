//! OS CPU counter for one local child, independent of worker telemetry.
//!
//! CPU supervision is Linux-only: `nix` reads another process's CPU clock without `unsafe`.
//! Other platforms report the clock as unsupported, and `Pool::new` rejects CPU mode there.

#[cfg(not(target_os = "linux"))]
use std::convert::Infallible;
#[cfg(target_os = "linux")]
use std::{fs, path::PathBuf, process};
use std::{io, time::Duration};

#[cfg(target_os = "linux")]
use nix::{libc, time::ClockId, unistd::Pid};
use tokio::process::Child;

/// The child's process CPU clock: user and system time summed over all its threads.
///
/// The clock is addressed by PID, which the kernel can reuse once the child is reaped, possibly by
/// another reaper in the host process. [`Self::sample`] is cheap but unconfirmed; [`Self::confirm`]
/// checks the child's `/proc` start time, which vouches for every sample taken before it.
/// Confirm before acting on a sample (killing for it, or charging it to a budget).
#[derive(Clone)]
pub(crate) struct Clock {
    #[cfg(target_os = "linux")]
    id: ClockId,
    /// `/proc/<pid>/stat`, re-read by every [`Self::confirm`].
    #[cfg(target_os = "linux")]
    stat_path: PathBuf,
    /// The child's `starttime` (field 22 of `stat`), recorded while it was certainly unreaped.
    #[cfg(target_os = "linux")]
    start_time: u64,
    /// Off Linux no clock can be built, so its methods are unreachable.
    #[cfg(not(target_os = "linux"))]
    never: Infallible,
}

impl Clock {
    /// Records the child's identity. Call right after spawn, before anything can have reaped it.
    ///
    /// Fails unless `/proc` reports this process as the child's parent: a `/proc` mounted for another
    /// PID namespace would otherwise record an unrelated process and confirm against it forever.
    pub(crate) fn new(child: &Child) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Self::for_child(child.id().ok_or_else(child_exited)?, process::id())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = child;
            Err(unsupported())
        }
    }

    /// Reads the clock without checking the PID still names our child; see [`Self::confirm`].
    pub(crate) fn sample(&self) -> io::Result<Duration> {
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

    /// Confirms the PID has named our child continuously since spawn, vouching for every earlier sample.
    ///
    /// An unchanged start time means the child is still alive or unreaped. A PID is only reused after
    /// a reap, so it cannot have named another process at any point in between.
    pub(crate) fn confirm(&self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        match fs::read_to_string(&self.stat_path).and_then(|stat| parse_start_time(&stat)) {
            Ok(start_time) if start_time == self.start_time => Ok(()),
            Ok(_) => Err(child_exited()),
            Err(error) if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH) => {
                Err(child_exited())
            }
            Err(error) => Err(error),
        }
        #[cfg(not(target_os = "linux"))]
        match self.never {}
    }

    /// Records `pid`'s clock and start time after checking `/proc` names `parent` as its parent.
    #[cfg(target_os = "linux")]
    fn for_child(pid: u32, parent: u32) -> io::Result<Self> {
        let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
        let stat = fs::read_to_string(&stat_path)?;
        if stat_field(&stat, PARENT_PID_FIELD)? != parent.to_string() {
            return Err(io::Error::other(format!(
                "/proc/{pid} is not this process's child; /proc may belong to another PID namespace"
            )));
        }
        let start_time = parse_start_time(&stat)?;
        let id = ClockId::pid_cpu_clock_id(Pid::from_raw(i32::try_from(pid).map_err(io::Error::other)?))
            .map_err(io::Error::from)?;
        Ok(Self {
            id,
            stat_path,
            start_time,
        })
    }
}

/// `ppid` in `/proc/<pid>/stat`, numbered from 1 as in proc(5).
#[cfg(target_os = "linux")]
const PARENT_PID_FIELD: usize = 4;
/// `starttime`, in clock ticks since boot.
#[cfg(target_os = "linux")]
const START_TIME_FIELD: usize = 22;

#[cfg(target_os = "linux")]
fn parse_start_time(stat: &str) -> io::Result<u64> {
    stat_field(stat, START_TIME_FIELD)?
        .parse()
        .map_err(|_| io::Error::other("malformed /proc stat: start time is not a number"))
}

/// Field `number` (from 3 on) of a `stat` line. Field 2 is `(comm)`, which may itself contain
/// spaces and `)`, so fields are counted from the last `)`.
#[cfg(target_os = "linux")]
fn stat_field(stat: &str, number: usize) -> io::Result<&str> {
    const FIRST_FIELD_AFTER_COMM: usize = 3;
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("malformed /proc stat: no command name"))?;
    fields
        .split_ascii_whitespace()
        .nth(number - FIRST_FIELD_AFTER_COMM)
        .ok_or_else(|| io::Error::other(format!("malformed /proc stat: no field {number}")))
}

#[cfg(target_os = "linux")]
fn child_exited() -> io::Error {
    io::Error::other("child has already exited")
}

#[cfg(not(target_os = "linux"))]
fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "child CPU clock is only available on Linux")
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/unit/cpu_clock.rs"]
mod tests;
