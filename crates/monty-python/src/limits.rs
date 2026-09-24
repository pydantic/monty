//! Extraction of Monty's `ResourceLimits` from the Python `limits` dict.

use std::time::Duration;

use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

/// Extracts limits using `ResourceLimits` defaults for missing or `None` values.
/// Wrong types raise `TypeError`; unknown keys and invalid durations raise `ValueError`.
/// Rejecting unknown keys prevents typos from silently disabling a limit.
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
            LimitKey::MaxFeedDurationSecs => limits.max_feed_duration(extract_duration(&value)?),
            LimitKey::MaxTurnDurationSecs => limits.max_turn_duration(extract_duration(&value)?),
            LimitKey::MaxMemory => limits.max_memory(value.extract()?),
            LimitKey::GcInterval => limits.gc_interval(value.extract()?),
            LimitKey::MaxRecursionDepth => limits.max_recursion_depth(value.extract()?),
            LimitKey::MaxSuspensions => limits.max_suspensions(value.extract()?),
            LimitKey::MaxTotalSleepSecs => {
                let d = Duration::try_from_secs_f64(value.extract()?)
                    .map_err(|err| PyValueError::new_err(err.to_string()))?;
                limits.max_total_sleep(d)
            }
        };
    }
    Ok(limits)
}

/// Reads one `*_duration_secs` value as a `Duration`, rejecting negative, NaN
/// and out-of-range values.
fn extract_duration(value: &Bound<'_, PyAny>) -> PyResult<Duration> {
    Duration::try_from_secs_f64(value.extract()?).map_err(|err| PyValueError::new_err(err.to_string()))
}

/// One recognized `limits` key. Anything else fails extraction with a
/// `ValueError`, so a typo can't silently run without the intended cap.
#[derive(Clone, Copy)]
enum LimitKey {
    MaxFeedDurationSecs,
    MaxTurnDurationSecs,
    MaxMemory,
    GcInterval,
    MaxRecursionDepth,
    MaxSuspensions,
    MaxTotalSleepSecs,
}

impl<'a, 'py> FromPyObject<'a, 'py> for LimitKey {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        match ob.extract::<&str>().unwrap_or_default() {
            "max_feed_duration_secs" => Ok(Self::MaxFeedDurationSecs),
            "max_turn_duration_secs" => Ok(Self::MaxTurnDurationSecs),
            "max_memory" => Ok(Self::MaxMemory),
            "gc_interval" => Ok(Self::GcInterval),
            "max_recursion_depth" => Ok(Self::MaxRecursionDepth),
            "max_suspensions" => Ok(Self::MaxSuspensions),
            "max_total_sleep_secs" => Ok(Self::MaxTotalSleepSecs),
            _ => {
                // `repr()` runs user `__repr__`, which may itself raise — fall
                // back so the promised `ValueError` is raised for every unknown key.
                let key_repr = ob
                    .repr()
                    .map_or_else(|_| "<unprintable key>".to_owned(), |r| r.to_string());
                Err(PyValueError::new_err(format!(
                    "unknown limits key {key_repr}; accepted keys are \
                     'max_feed_duration_secs', 'max_turn_duration_secs', 'max_memory', \
                     'gc_interval', 'max_recursion_depth', 'max_suspensions', 'max_total_sleep_secs'"
                )))
            }
        }
    }
}
