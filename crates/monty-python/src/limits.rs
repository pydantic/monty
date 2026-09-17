//! Extraction of Monty's `ResourceLimits` from the Python `limits` dict.

use std::time::Duration;

use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

/// Extracts resource limits from a Python dict.
///
/// The dict should have the following optional keys:
/// - `max_duration_secs`: Maximum execution time in seconds (float)
/// - `max_memory`: Maximum heap memory in bytes (int)
/// - `gc_interval`: Run garbage collection every N allocations (int)
/// - `max_recursion_depth`: Maximum function call stack depth (int, default: 1000)
/// - `max_suspensions`: Maximum host round trips the pool will service (int, default: 1000)
///
/// If a key is missing or set to `None`, that limit is not applied
/// (except `max_recursion_depth` and `max_suspensions`, which default to 1000).
///
/// Raises `TypeError` if a value is present but has the wrong type.
/// Raises `ValueError` if the dict contains an unknown key — limits are a
/// security surface, so a misspelled key (e.g. `max_memroy`) must not silently
/// run without the intended cap — or if `max_duration_secs` is not a valid
/// duration value.
pub fn extract_limits(dict: &Bound<'_, PyDict>) -> PyResult<monty_types::ResourceLimits> {
    let mut limits = monty_types::ResourceLimits::default();
    // Keys parse into `LimitKey` and values are read from the same entry, so
    // validation and extraction share one path — no re-lookup that a `str`
    // subclass with a custom `__hash__` could dodge.
    for (key, value) in dict.iter() {
        let key: LimitKey = key.extract()?;
        if value.is_none() {
            // An explicit `None` disables the limit, like an absent key.
            continue;
        }
        limits = match key {
            LimitKey::MaxDurationSecs => {
                let d = Duration::try_from_secs_f64(value.extract()?)
                    .map_err(|err| PyValueError::new_err(err.to_string()))?;
                limits.max_duration(d)
            }
            LimitKey::MaxMemory => limits.max_memory(value.extract()?),
            LimitKey::GcInterval => limits.gc_interval(value.extract()?),
            LimitKey::MaxRecursionDepth => limits.max_recursion_depth(value.extract()?),
            LimitKey::MaxSuspensions => limits.max_suspensions(value.extract()?),
        };
    }
    Ok(limits)
}

/// One recognized `limits` key. Anything else fails extraction with a
/// `ValueError`, so a typo can't silently run without the intended cap.
#[derive(Clone, Copy)]
enum LimitKey {
    MaxDurationSecs,
    MaxMemory,
    GcInterval,
    MaxRecursionDepth,
    MaxSuspensions,
}

impl<'a, 'py> FromPyObject<'a, 'py> for LimitKey {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        match ob.extract::<&str>().unwrap_or_default() {
            "max_duration_secs" => Ok(Self::MaxDurationSecs),
            "max_memory" => Ok(Self::MaxMemory),
            "gc_interval" => Ok(Self::GcInterval),
            "max_recursion_depth" => Ok(Self::MaxRecursionDepth),
            "max_suspensions" => Ok(Self::MaxSuspensions),
            _ => {
                // `repr()` runs user `__repr__`, which may itself raise — fall
                // back so the promised `ValueError` is raised for every unknown key.
                let key_repr = ob
                    .repr()
                    .map_or_else(|_| "<unprintable key>".to_owned(), |r| r.to_string());
                Err(PyValueError::new_err(format!(
                    "unknown limits key {key_repr}; accepted keys are 'max_duration_secs', \
                     'max_memory', 'gc_interval', 'max_recursion_depth', 'max_suspensions'"
                )))
            }
        }
    }
}
