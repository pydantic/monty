//! Resource limits handling for the Monty TypeScript/JavaScript bindings.
//!
//! Provides utilities to extract and apply resource limits from JavaScript objects,
//! including time limits, memory limits, and recursion depth.

use std::time::Duration;

use monty::{ResourceLimits, DEFAULT_MAX_RECURSION_DEPTH};
use napi::{Error, Result, Status};
use napi_derive::napi;

/// Resource limits configuration from JavaScript.
///
/// All limits are optional. Omit a key to disable that limit.
/// Numeric limits are received as JS `number`s, so the boundary uses `f64`
/// and validates them before converting into Rust `usize` values.
#[napi(object, js_name = "ResourceLimits")]
#[derive(Debug, Clone, Copy, Default)]
pub struct JsResourceLimits {
    /// Maximum number of heap allocations allowed.
    pub max_allocations: Option<f64>,
    /// Maximum execution time in seconds.
    pub max_duration_secs: Option<f64>,
    /// Maximum heap memory in bytes.
    pub max_memory: Option<f64>,
    /// Run garbage collection every N allocations.
    pub gc_interval: Option<f64>,
    /// Maximum function call stack depth (default: 1000).
    pub max_recursion_depth: Option<f64>,
}

/// Extracts a Rust resource-limit configuration from a JS resource-limit object.
///
/// This mirrors the Python binding convention: validate host-provided limits at
/// the boundary, then convert them into Monty's internal `ResourceLimits`.
///
/// Returns `Err` for invalid JS number inputs that cannot be represented as
/// `usize` or for invalid duration values rejected by
/// `std::time::Duration::try_from_secs_f64`.
pub fn extract_limits(js_limits: JsResourceLimits) -> Result<ResourceLimits> {
    let max_recursion_depth = js_limits
        .max_recursion_depth
        .map(|v| js_number_to_usize(v, "maxRecursionDepth"))
        .transpose()?
        .or(Some(DEFAULT_MAX_RECURSION_DEPTH));

    let mut limits = ResourceLimits::new().max_recursion_depth(max_recursion_depth);

    if let Some(max) = js_limits.max_allocations {
        limits = limits.max_allocations(js_number_to_usize(max, "maxAllocations")?);
    }
    if let Some(secs) = js_limits.max_duration_secs {
        limits = limits.max_duration(
            Duration::try_from_secs_f64(secs).map_err(|err| Error::new(Status::InvalidArg, err.to_string()))?,
        );
    }
    if let Some(max) = js_limits.max_memory {
        limits = limits.max_memory(js_number_to_usize(max, "maxMemory")?);
    }
    if let Some(interval) = js_limits.gc_interval {
        limits = limits.gc_interval(js_number_to_usize(interval, "gcInterval")?);
    }

    Ok(limits)
}

impl TryFrom<JsResourceLimits> for ResourceLimits {
    type Error = Error;

    fn try_from(js_limits: JsResourceLimits) -> Result<Self> {
        extract_limits(js_limits)
    }
}

/// Converts a JavaScript `number` used for a size/count limit into `usize`.
///
/// JavaScript numbers are IEEE-754 doubles, so integers above `2^53 - 1`
/// cannot be represented exactly. Rejecting values outside the safe integer
/// range avoids silently rounding resource limits at the napi boundary.
///
/// Returns `Err` for non-finite, negative, fractional, or out-of-range inputs.
/// This helper does not panic.
fn js_number_to_usize(value: f64, name: &str) -> Result<usize> {
    const JS_MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

    match value {
        v if !v.is_finite() => Err(Error::new(
            Status::InvalidArg,
            format!("{name} must be a finite number"),
        )),
        v if v < 0.0 => Err(Error::new(Status::InvalidArg, format!("{name} must be non-negative"))),
        v if v.fract() != 0.0 => Err(Error::new(Status::InvalidArg, format!("{name} must be an integer"))),
        v if v > JS_MAX_SAFE_INTEGER as f64 => Err(Error::new(
            Status::InvalidArg,
            format!("{name} must be a safe integer (<= {JS_MAX_SAFE_INTEGER})"),
        )),
        v => {
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let value = v as u64;
            usize::try_from(value).map_err(|_| {
                Error::new(
                    Status::InvalidArg,
                    format!("{name} must fit in Rust usize on this platform"),
                )
            })
        }
    }
}
