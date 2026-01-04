//! Python wrapper for Monty's `ResourceLimits`.
//!
//! Provides a TypedDict interface to configure resource limits for code execution,
//! including time limits, memory limits, and recursion depth.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::time::Duration;

/// Default maximum recursion depth if not specified.
const DEFAULT_MAX_RECURSION_DEPTH: usize = 1000;

/// Extracts resource limits from a Python dict.
///
/// The dict should have the following optional keys:
/// - `max_allocations`: Maximum number of heap allocations allowed (int)
/// - `max_duration_secs`: Maximum execution time in seconds (float)
/// - `max_memory`: Maximum heap memory in bytes (int)
/// - `gc_interval`: Run garbage collection every N allocations (int)
/// - `max_recursion_depth`: Maximum function call stack depth (int, default: 1000)
///
/// If a key is missing or set to `None`, that limit is not applied
/// (except `max_recursion_depth` which defaults to 1000).
pub fn extract_limits(dict: &Bound<'_, PyDict>) -> PyResult<monty::ResourceLimits> {
    let max_allocations: Option<usize> = dict.get_item("max_allocations")?.and_then(|v| v.extract().ok());
    let max_duration_secs: Option<f64> = dict.get_item("max_duration_secs")?.and_then(|v| v.extract().ok());
    let max_memory: Option<usize> = dict.get_item("max_memory")?.and_then(|v| v.extract().ok());
    let gc_interval: Option<usize> = dict.get_item("gc_interval")?.and_then(|v| v.extract().ok());
    let max_recursion_depth: Option<usize> = dict
        .get_item("max_recursion_depth")?
        .and_then(|v| v.extract().ok())
        .or(Some(DEFAULT_MAX_RECURSION_DEPTH));

    let mut limits = monty::ResourceLimits::new().max_recursion_depth(max_recursion_depth);

    if let Some(max) = max_allocations {
        limits = limits.max_allocations(max);
    }
    if let Some(secs) = max_duration_secs {
        limits = limits.max_duration(Duration::from_secs_f64(secs));
    }
    if let Some(max) = max_memory {
        limits = limits.max_memory(max);
    }
    if let Some(interval) = gc_interval {
        limits = limits.gc_interval(interval);
    }

    Ok(limits)
}
