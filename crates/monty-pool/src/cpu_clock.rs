//! OS CPU counters for one local child, independent of worker telemetry.

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::{io, time::Duration};

#[cfg(target_os = "macos")]
use mach2::mach_time::{mach_timebase_info, mach_timebase_info_data_t};
#[cfg(target_os = "linux")]
use nix::{time::ClockId, unistd::Pid};
use tokio::process::Child;
#[cfg(windows)]
use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};

pub(crate) struct Clock {
    #[cfg(target_os = "linux")]
    id: ClockId,
    #[cfg(target_os = "macos")]
    pid: i32,
    #[cfg(target_os = "macos")]
    timebase: mach_timebase_info_data_t,
    #[cfg(windows)]
    handle: OwnedHandle,
}

impl Clock {
    // The checkout retains the child; final sampling is skipped after teardown removes it.
    #[cfg_attr(
        any(target_os = "macos", windows),
        expect(unsafe_code, reason = "checked OS timebase and borrowed child handle")
    )]
    pub(crate) fn new(child: &Child) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let pid = i32::try_from(child.id().ok_or_else(child_exited)?).map_err(io::Error::other)?;
            let id = ClockId::pid_cpu_clock_id(Pid::from_raw(pid)).map_err(io::Error::from)?;
            Ok(Self { id })
        }
        #[cfg(target_os = "macos")]
        {
            let pid = i32::try_from(child.id().ok_or_else(child_exited)?).map_err(io::Error::other)?;
            let mut timebase = mach_timebase_info_data_t {
                numer: 0, // codespell:ignore numer
                denom: 0,
            };
            // SAFETY: the OS writes one initialized struct through a live, aligned pointer.
            let status = unsafe { mach_timebase_info(&raw mut timebase) };
            let numerator = timebase.numer; // codespell:ignore numer
            if status != 0 || numerator == 0 || timebase.denom == 0 {
                return Err(io::Error::other(format!("invalid Mach timebase: status {status}")));
            }
            Ok(Self { pid, timebase })
        }
        #[cfg(windows)]
        {
            let handle = child.raw_handle().ok_or_else(child_exited)?;
            // SAFETY: `child` owns this live handle throughout the borrow. The duplicate owns its lifetime.
            let handle = unsafe { BorrowedHandle::borrow_raw(handle) }.try_clone_to_owned()?;
            Ok(Self { handle })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = child;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "child CPU clock is unavailable",
            ))
        }
    }

    #[cfg_attr(
        any(target_os = "macos", windows),
        expect(unsafe_code, reason = "checked OS CPU counters and initialized output buffers")
    )]
    pub(crate) fn read(&self) -> io::Result<Duration> {
        #[cfg(target_os = "linux")]
        {
            let time = self.id.now().map_err(io::Error::from)?;
            Ok(Duration::new(
                u64::try_from(time.tv_sec()).map_err(io::Error::other)?,
                u32::try_from(time.tv_nsec()).map_err(io::Error::other)?,
            ))
        }
        #[cfg(target_os = "macos")]
        {
            let mut usage = libc::rusage_info_v0 {
                ri_uuid: [0; 16],
                ri_user_time: 0,
                ri_system_time: 0,
                ri_pkg_idle_wkups: 0,
                ri_interrupt_wkups: 0,
                ri_pageins: 0,
                ri_wired_size: 0,
                ri_resident_size: 0,
                ri_phys_footprint: 0,
                ri_proc_start_abstime: 0,
                ri_proc_exit_abstime: 0,
            };
            // SAFETY: flavor V0 writes exactly rusage_info_v0. The buffer is aligned and lives through the call.
            let status = unsafe { libc::proc_pid_rusage(self.pid, libc::RUSAGE_INFO_V0, (&raw mut usage).cast()) };
            if status != 0 {
                return Err(io::Error::last_os_error());
            }
            ticks_to_duration(
                u128::from(usage.ri_user_time) + u128::from(usage.ri_system_time),
                self.timebase.numer, // codespell:ignore numer
                self.timebase.denom,
            )
        }
        #[cfg(windows)]
        {
            let mut created = FILETIME::default();
            let mut exited = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: the owned handle stays live; each output points to a separate initialized FILETIME.
            let status = unsafe {
                GetProcessTimes(
                    self.handle.as_raw_handle(),
                    &raw mut created,
                    &raw mut exited,
                    &raw mut kernel,
                    &raw mut user,
                )
            };
            if status == 0 {
                return Err(io::Error::last_os_error());
            }
            let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
            ticks_to_duration(u128::from(ticks(kernel)) + u128::from(ticks(user)), 100, 1)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "child CPU clock is unavailable",
            ))
        }
    }
}

fn child_exited() -> io::Error {
    io::Error::other("child has already exited")
}

/// Mach absolute ticks use a rational nanosecond scale; Windows FILETIME uses 100 ns ticks.
#[cfg(any(target_os = "macos", windows, test))]
fn ticks_to_duration(ticks: u128, numerator: u32, denominator: u32) -> io::Result<Duration> {
    let nanos = ticks
        .checked_mul(u128::from(numerator))
        .and_then(|value| value.checked_div(u128::from(denominator)))
        .ok_or_else(|| io::Error::other("CPU clock conversion overflow or invalid scale"))?;
    Ok(Duration::new(
        u64::try_from(nanos / 1_000_000_000).map_err(io::Error::other)?,
        u32::try_from(nanos % 1_000_000_000).map_err(io::Error::other)?,
    ))
}

#[cfg(test)]
#[path = "../tests/unit/cpu_clock.rs"]
mod tests;
