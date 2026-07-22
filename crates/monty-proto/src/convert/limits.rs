//! `ResourceLimits` ↔ `pb::ResourceLimits` conversions.
//!
//! Wire fields are `u64`; the Rust struct uses `usize`, so proto → Rust
//! saturates to `usize::MAX` on 32-bit hosts. Absent wire fields mean
//! "unlimited", except recursion depth which falls back to monty's standard
//! default — matching `ResourceLimits::new()` so an empty message is safe.

use std::time::Duration;

use monty_types::{DEFAULT_MAX_RECURSION_DEPTH, ResourceLimits};

use crate::pb;

impl From<&ResourceLimits> for pb::ResourceLimits {
    fn from(limits: &ResourceLimits) -> Self {
        Self {
            max_duration_micros: limits
                .max_duration
                .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX)),
            max_memory_bytes: limits.max_memory.map(|v| v as u64),
            gc_interval: limits.gc_interval.map(|v| v as u64),
            max_recursion_depth: limits.max_recursion_depth.map(|v| v as u64),
        }
    }
}

impl From<pb::ResourceLimits> for ResourceLimits {
    fn from(limits: pb::ResourceLimits) -> Self {
        Self {
            max_duration: limits.max_duration_micros.map(Duration::from_micros),
            max_memory: usize_field(limits.max_memory_bytes),
            gc_interval: usize_field(limits.gc_interval),
            max_recursion_depth: usize_field(limits.max_recursion_depth).or(Some(DEFAULT_MAX_RECURSION_DEPTH)),
        }
    }
}

/// Narrows an optional wire `u64` to `usize`, saturating to `usize::MAX` on 32-bit hosts.
fn usize_field(value: Option<u64>) -> Option<usize> {
    value.map(|v| usize::try_from(v).unwrap_or(usize::MAX))
}
