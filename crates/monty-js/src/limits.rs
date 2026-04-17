//! Resource limits handling for the Monty TypeScript/JavaScript bindings.
//!
//! Provides utilities to extract and apply resource limits from JavaScript objects,
//! including time limits, memory limits, and recursion depth.

use std::time::Duration;

use monty::{ResourceLimits, DEFAULT_MAX_RECURSION_DEPTH};
use napi_derive::napi;

/// Resource limits configuration from JavaScript.
///
/// All limits are optional. Omit a key to disable that limit.
#[napi(object, js_name = "ResourceLimits")]
#[derive(Debug, Clone, Copy, Default)]
pub struct JsResourceLimits {
    /// Maximum number of heap allocations allowed.
    pub max_allocations: Option<u32>,
    /// Maximum execution time in seconds.
    pub max_duration_secs: Option<f64>,
    /// Maximum heap memory in bytes. Stored as `f64` to match JS `number`.
    pub max_memory: Option<f64>,
    /// Run garbage collection every N allocations.
    pub gc_interval: Option<u32>,
    /// Maximum function call stack depth (default: 1000).
    pub max_recursion_depth: Option<u32>,
}

impl From<JsResourceLimits> for ResourceLimits {
    fn from(js_limits: JsResourceLimits) -> Self {
        let max_recursion_depth = js_limits
            .max_recursion_depth
            .map(|v| v as usize)
            .or(Some(DEFAULT_MAX_RECURSION_DEPTH));

        let mut limits = Self::new().max_recursion_depth(max_recursion_depth);

        if let Some(max) = js_limits.max_allocations {
            limits = limits.max_allocations(max as usize);
        }
        if let Some(secs) = js_limits.max_duration_secs {
            limits = limits.max_duration(Duration::from_secs_f64(secs));
        }
        if let Some(max) = js_limits.max_memory {
            limits = limits.max_memory(js_number_to_usize(max, "maxMemory"));
        }
        if let Some(interval) = js_limits.gc_interval {
            limits = limits.gc_interval(interval as usize);
        }

        limits
    }
}

/// Converts a JavaScript `number` used for a size/count limit into `usize`.
///
/// JavaScript numbers are IEEE-754 doubles, so integers above `2^53 - 1`
/// cannot be represented exactly. Rejecting values outside the safe integer
/// range avoids silently rounding resource limits at the napi boundary.
fn js_number_to_usize(value: f64, name: &str) -> usize {
    const JS_MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

    match value {
        v if !v.is_finite() => panic!("{name} must be a finite number"),
        v if v < 0.0 => panic!("{name} must be non-negative"),
        v if v.fract() != 0.0 => panic!("{name} must be an integer"),
        v if v > JS_MAX_SAFE_INTEGER as f64 => {
            panic!("{name} must be a safe integer (<= {JS_MAX_SAFE_INTEGER})")
        }
        v => {
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let value = v as u64;
            usize::try_from(value).unwrap_or_else(|_| panic!("{name} must fit in Rust usize on this platform"))
        }
    }
}
