//! Extraction of Monty's `ResourceLimits` from the Python `limits` dict.

use std::time::Duration;

use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::PyDict,
};

/// The keys `extract_limits` understands; anything else is a hard error so typos can't fail open.
const KNOWN_KEYS: [&str; 4] = ["max_duration_secs", "max_memory", "gc_interval", "max_recursion_depth"];

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
/// Raises `TypeError` if a value is present but has the wrong type, or if the dict
/// contains an unknown key — limits are a security surface, so a misspelled key
/// (e.g. `max_memroy`) must not silently run without the intended cap.
/// Raises `ValueError` if `max_duration_secs` is not a valid duration value.
pub fn extract_limits(dict: &Bound<'_, PyDict>) -> PyResult<monty_types::ResourceLimits> {
    check_unknown_keys(dict)?;
    let max_duration_secs = extract_optional_f64(dict, "max_duration_secs")?;
    let max_memory = extract_optional_usize(dict, "max_memory")?;
    let gc_interval = extract_optional_usize(dict, "gc_interval")?;
    let max_recursion_depth = extract_optional_usize(dict, "max_recursion_depth")?;

    let mut limits = monty_types::ResourceLimits::default();

    if let Some(max_recursion_depth) = max_recursion_depth {
        limits = limits.max_recursion_depth(max_recursion_depth);
    }
    if let Some(secs) = max_duration_secs {
        let d = Duration::try_from_secs_f64(secs).map_err(|err| PyValueError::new_err(err.to_string()))?;
        limits = limits.max_duration(d);
    }
    if let Some(max) = max_memory {
        limits = limits.max_memory(max);
    }
    if let Some(interval) = gc_interval {
        limits = limits.gc_interval(interval);
    }

    Ok(limits)
}

/// Rejects unrecognized (or non-string) keys with a `TypeError` naming the key and the accepted set.
fn check_unknown_keys(dict: &Bound<'_, PyDict>) -> PyResult<()> {
    for key in dict.keys() {
        let known = key.extract::<&str>().is_ok_and(|k| KNOWN_KEYS.contains(&k));
        if !known {
            let accepted = KNOWN_KEYS.map(|k| format!("'{k}'")).join(", ");
            // `repr()` runs user `__repr__`, which may itself raise — fall back
            // so the promised `TypeError` is raised for every unknown key.
            let key_repr = key
                .repr()
                .map_or_else(|_| "<unprintable key>".to_owned(), |r| r.to_string());
            return Err(PyTypeError::new_err(format!(
                "unknown limits key {key_repr}; accepted keys are {accepted}"
            )));
        }
    }
    Ok(())
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
