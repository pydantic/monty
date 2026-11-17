//! Work accounting for [`ExecutionClock::Work`](super::ExecutionClock::Work): counted units, not clock reads.

use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

/// Bytes ever requested through the global allocator, wrapping at `usize::MAX`.
/// `monty-alloc` advances it; it stays zero where that allocator is not installed.
pub static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Execution time one unit of work stands for on the work clock.
///
/// A unit is one bytecode instruction. The value is the median measured for the wasm build, so
/// native builds report more time than they took. Fixed so durations compare across builds and dumps.
pub const NANOS_PER_UNIT: u64 = 20;

/// Bytes allocated per unit charged, so work that builds a result without its own charge still costs.
const BYTES_PER_UNIT: usize = 32;

/// Counts the work of one execution window and converts it to execution time.
///
/// Reads no clock: the VM and native operations charge units, and allocation is read from
/// [`ALLOCATED_BYTES`] as a floor under both. Only the converted durations outlive a window.
#[derive(Debug, Default)]
pub(super) struct WorkMeter {
    /// Whether a window is open; charges outside one are dropped at the next start.
    running: Cell<bool>,
    /// Units charged to the open window.
    units: Cell<u64>,
    /// [`ALLOCATED_BYTES`] up to which allocation has been charged.
    allocated: Cell<usize>,
}

impl WorkMeter {
    /// Opens a window, discarding allocation made while idle.
    pub(super) fn start(&self) {
        self.running.set(true);
        self.units.set(0);
        self.allocated.set(ALLOCATED_BYTES.load(Ordering::Relaxed));
    }

    /// Closes the window and returns the time it stands for.
    pub(super) fn stop(&self) -> Duration {
        let window = self.window();
        self.running.set(false);
        self.units.set(0);
        window
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.get()
    }

    /// Adds `units` to the open window.
    #[inline]
    pub(super) fn charge(&self, units: u64) {
        self.units.set(self.units.get().saturating_add(units));
    }

    /// Time the open window stands for so far, or zero when idle.
    pub(super) fn window(&self) -> Duration {
        if self.running.get() {
            self.charge_allocation();
            Duration::from_nanos(self.units.get().saturating_mul(NANOS_PER_UNIT))
        } else {
            Duration::ZERO
        }
    }

    /// Charges whole units for bytes allocated since the last call, carrying the remainder.
    fn charge_allocation(&self) {
        let charged = self.allocated.get();
        let units = ALLOCATED_BYTES.load(Ordering::Relaxed).wrapping_sub(charged) / BYTES_PER_UNIT;
        self.allocated.set(charged.wrapping_add(units * BYTES_PER_UNIT));
        self.charge(units as u64);
    }
}
