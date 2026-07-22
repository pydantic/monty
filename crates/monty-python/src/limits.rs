//! Extraction of Monty's `ResourceLimits` from the Python `limits` dict.

use std::time::Duration;

use monty_types::DEFAULT_MAX_RECURSION_DEPTH;
use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

/// Extracts resource limits from a Python dict.
///
/// The dict should have the following optional keys:
/// - `max_duration_secs`: Maximum execution time in seconds (float)
/// - `max_memory`: Maximum heap memory in bytes (int)
/// - `gc_interval`: Run garbage collection every N allocations (int)
/// - `max_recursion_depth`: Maximum function call stack depth (int, default: 1000)
///
/// If a key is missing or set to `None`, that limit is not applied
/// (except `max_recursion_depth` which defaults to 1000).
///
/// Raises `TypeError` if a value is present but has the wrong type.
/// Raises `ValueError` if `max_duration_secs` is not a valid duration value.
pub fn extract_limits(dict: &Bound<'_, PyDict>) -> PyResult<monty_types::ResourceLimits> {
    let max_duration_secs = extract_optional_f64(dict, "max_duration_secs")?;
    let max_memory = extract_optional_usize(dict, "max_memory")?;
    let gc_interval = extract_optional_usize(dict, "gc_interval")?;
    let max_recursion_depth =
        extract_optional_usize(dict, "max_recursion_depth")?.or(Some(DEFAULT_MAX_RECURSION_DEPTH));

    let mut limits = monty_types::ResourceLimits::new().max_recursion_depth(max_recursion_depth);

    if let Some(secs) = max_duration_secs {
        limits = limits
            .max_duration(Duration::try_from_secs_f64(secs).map_err(|err| PyValueError::new_err(err.to_string()))?);
    }
    if let Some(max) = max_memory {
        limits = limits.max_memory(max);
    }
    if let Some(interval) = gc_interval {
        limits = limits.gc_interval(interval);
    }

    Ok(limits)
}

/// Extracts an optional usize from a dict, raising `TypeError` if the value has the wrong type.
fn extract_optional_usize(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<usize>> {
    match dict.get_item(key)? {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(value.extract()?)),
    }
}

/// Extracts an optional f64 from a dict, raising `TypeError` if the value has the wrong type.
fn extract_optional_f64(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<f64>> {
    match dict.get_item(key)? {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(value.extract()?)),
    }
}
