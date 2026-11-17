//! Work accounting for [`ExecutionClock::Work`](super::ExecutionClock::Work): counted units, not clock reads.

use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

#[cfg(feature = "work-profile")]
use super::work_profile;
use super::{work_event::WorkEvent, work_weights::EVENT_MILLI_UNITS};

/// Bytes ever requested through the global allocator, plus [`ALLOCATION_OVERHEAD_BYTES`] for each
/// request, wrapping at `usize::MAX`.
/// `monty-alloc` advances it; it stays zero where that allocator is not installed.
pub static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Bytes each allocation counts for beyond its size, so one counter carries both the
/// [`WorkEvent::Allocation`] and the [`WorkEvent::AllocatedByte`] charge.
pub const ALLOCATION_OVERHEAD_BYTES: usize =
    (EVENT_MILLI_UNITS[WorkEvent::Allocation as usize] / EVENT_MILLI_UNITS[WorkEvent::AllocatedByte as usize]) as usize;

/// Execution time one unit of work stands for on the work clock, in a WebAssembly build.
///
/// A unit is one cheap bytecode instruction, such as a local load; costlier ones are charged several.
/// The value is the median `make calibrate-work-clock` measures for the build under Node.
#[cfg(target_family = "wasm")]
pub const NANOS_PER_UNIT: u64 = 5;

/// Execution time one unit of work stands for on the work clock, in a native build.
///
/// The units are those of the WebAssembly build; only the rate differs, as native code runs them
/// faster. The value is the median `make calibrate-work-clock` measures natively.
#[cfg(not(target_family = "wasm"))]
pub const NANOS_PER_UNIT: u64 = 3;

/// Counts the work of one execution window and converts it to execution time.
///
/// Reads no clock: the VM and native operations charge work, and allocation is read from
/// [`ALLOCATED_BYTES`] as a floor under both. Only the converted durations outlive a window.
#[derive(Debug, Default)]
pub(super) struct WorkMeter {
    /// Whether a window is open; charges outside one are dropped at the next start.
    running: Cell<bool>,
    /// Thousandths of a unit charged to the open window.
    milli_units: Cell<u64>,
    /// [`ALLOCATED_BYTES`] up to which allocation has been charged.
    allocated: Cell<usize>,
    /// Allocations up to which the profile has recorded them.
    #[cfg(feature = "work-profile")]
    profiled_allocations: Cell<usize>,
}

impl WorkMeter {
    /// Opens a window, discarding allocation made while idle.
    pub(super) fn start(&self) {
        self.running.set(true);
        self.milli_units.set(0);
        self.allocated.set(ALLOCATED_BYTES.load(Ordering::Relaxed));
        #[cfg(feature = "work-profile")]
        self.profiled_allocations
            .set(work_profile::ALLOCATIONS.load(Ordering::Relaxed));
    }

    /// Closes the window and returns the time it stands for.
    pub(super) fn stop(&self) -> Duration {
        let window = self.window();
        self.running.set(false);
        self.milli_units.set(0);
        window
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.get()
    }

    /// Adds thousandths of a unit to the open window.
    #[inline]
    pub(super) fn charge(&self, milli_units: u64) {
        self.milli_units.set(self.milli_units.get().saturating_add(milli_units));
    }

    /// Time the open window stands for so far, or zero when idle.
    pub(super) fn window(&self) -> Duration {
        if self.running.get() {
            self.charge_allocation();
            Duration::from_nanos(self.milli_units.get().saturating_mul(NANOS_PER_UNIT) / 1000)
        } else {
            Duration::ZERO
        }
    }

    /// Charges the bytes allocated since the last call.
    fn charge_allocation(&self) {
        let allocated = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let bytes = allocated.wrapping_sub(self.allocated.replace(allocated));
        self.charge((bytes as u64).saturating_mul(WorkEvent::AllocatedByte.milli_units()));
        #[cfg(feature = "work-profile")]
        {
            let allocations = work_profile::ALLOCATIONS.load(Ordering::Relaxed);
            let count = allocations.wrapping_sub(self.profiled_allocations.replace(allocations));
            work_profile::record_event(WorkEvent::Allocation, count as u64);
            let own_bytes = bytes.saturating_sub(count.saturating_mul(ALLOCATION_OVERHEAD_BYTES));
            work_profile::record_event(WorkEvent::AllocatedByte, own_bytes as u64);
        }
    }
}
