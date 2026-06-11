//! `ResourceLimits` ↔ `pb::ResourceLimits` conversions.
//!
//! Wire fields are `u64`; the Rust struct uses `usize`, so proto → Rust can
//! fail on 32-bit hosts. Absent wire fields mean "unlimited", except
//! recursion depth which falls back to monty's standard default — matching
//! `ResourceLimits::new()` semantics so an empty message is a safe default.

use std::time::Duration;

use monty::{DEFAULT_MAX_RECURSION_DEPTH, ResourceLimits};

use crate::{convert::ProtoConvertError, pb};

impl From<&ResourceLimits> for pb::ResourceLimits {
    fn from(limits: &ResourceLimits) -> Self {
        Self {
            max_allocations: limits.max_allocations.map(|v| v as u64),
            max_duration_micros: limits
                .max_duration
                .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX)),
            max_memory_bytes: limits.max_memory.map(|v| v as u64),
            gc_interval: limits.gc_interval.map(|v| v as u64),
            max_recursion_depth: limits.max_recursion_depth.map(|v| v as u64),
        }
    }
}

impl TryFrom<pb::ResourceLimits> for ResourceLimits {
    type Error = ProtoConvertError;

    fn try_from(limits: pb::ResourceLimits) -> Result<Self, ProtoConvertError> {
        Ok(Self {
            max_allocations: usize_field(limits.max_allocations, "ResourceLimits.max_allocations")?,
            max_duration: limits.max_duration_micros.map(Duration::from_micros),
            max_memory: usize_field(limits.max_memory_bytes, "ResourceLimits.max_memory_bytes")?,
            gc_interval: usize_field(limits.gc_interval, "ResourceLimits.gc_interval")?,
            max_recursion_depth: usize_field(limits.max_recursion_depth, "ResourceLimits.max_recursion_depth")?
                .or(Some(DEFAULT_MAX_RECURSION_DEPTH)),
        })
    }
}

/// Narrows an optional wire `u64` to `usize` (fallible on 32-bit hosts only).
fn usize_field(value: Option<u64>, field: &'static str) -> Result<Option<usize>, ProtoConvertError> {
    value
        .map(|v| {
            usize::try_from(v).map_err(|_| ProtoConvertError::InvalidValue {
                field,
                reason: format!("{v} does not fit in usize"),
            })
        })
        .transpose()
}
