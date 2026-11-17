//! Counts of the work a run did, for fitting the work clock's weights.
//!
//! Compiled only with the `work-profile` feature, which no worker enables: the counters are
//! process-wide and cost an atomic add per instruction.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use strum::{EnumCount, IntoEnumIterator};

use super::work_event::WorkEvent;

/// Instructions executed, by opcode byte.
static OPCODES: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Occurrences charged, by [`WorkEvent`].
static EVENTS: [AtomicU64; WorkEvent::COUNT] = [const { AtomicU64::new(0) }; WorkEvent::COUNT];

/// Allocations ever made through the global allocator; `monty-alloc` advances it.
pub static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Records one executed instruction.
#[inline]
pub fn record_opcode(opcode: u8) {
    OPCODES[usize::from(opcode)].fetch_add(1, Ordering::Relaxed);
}

/// Records `quantity` occurrences of `event`.
#[inline]
pub fn record_event(event: WorkEvent, quantity: u64) {
    EVENTS[event as usize].fetch_add(quantity, Ordering::Relaxed);
}

/// The counts since the last call, which resets them.
#[must_use]
pub fn take() -> WorkProfile {
    WorkProfile {
        opcodes: (0..=u8::MAX)
            .map(|byte| (byte, OPCODES[usize::from(byte)].swap(0, Ordering::Relaxed)))
            .filter(|(_, count)| *count > 0)
            .collect(),
        events: WorkEvent::iter()
            .map(|event| (event, EVENTS[event as usize].swap(0, Ordering::Relaxed)))
            .filter(|(_, count)| *count > 0)
            .collect(),
    }
}

/// What one run executed and was charged for.
#[derive(Debug, Default)]
pub struct WorkProfile {
    /// Instructions executed per opcode byte, omitting those never run.
    pub opcodes: Vec<(u8, u64)>,
    /// Occurrences per event, omitting those never charged.
    pub events: Vec<(WorkEvent, u64)>,
}
