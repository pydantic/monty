//! Extraction of the `auto_os_calls` checkout argument — the `AutoOSCalls`
//! TypedDict — into the session's [`AutoOsCalls`].
//!
//! Every key is optional and defaults as the Rust struct does; an unknown
//! key is a `ValueError`, since a misspelt one would silently leave a call
//! answered differently from what the caller meant. Values are rejected at
//! argument-extraction time with a message naming the accepted forms.

use std::time::Duration;

use chrono::NaiveDate;
use monty_types::{AutoOsCalls, DateTimeSource, RandomSeed, RandomStart, SandboxTimeZone, SleepMode};
use num_bigint::BigInt;
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{
        PyBool, PyBytes, PyDateAccess, PyDateTime, PyDelta, PyDeltaAccess, PyDict, PyFloat, PyInt, PyString,
        PyTimeAccess,
    },
};

use crate::pool::duration_from_secs;

/// The `auto_os_calls` checkout argument; `None` is every default.
#[derive(Clone, Default)]
pub(crate) struct AutoOsCallsArg(pub AutoOsCalls);

impl<'a, 'py> FromPyObject<'a, 'py> for AutoOsCallsArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let Ok(dict) = ob.cast::<PyDict>() else {
            return Err(PyTypeError::new_err(format!(
                "auto_os_calls must be a dict, not {}",
                ob.get_type().name()?
            )));
        };
        let mut calls = AutoOsCalls::default();
        // A fixed `datetime` implies a zone (see `fixed_datetime`) unless
        // `timezone` says otherwise, so the zone key is applied last.
        let mut timezone = None;
        let mut max = None;
        for (key, value) in dict.iter() {
            let key = key
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("auto_os_calls keys must be str"))?
                .to_cow()?;
            match &*key {
                "datetime" => {
                    let (source, implied_zone) = datetime_source(&value)?;
                    calls.datetime = source;
                    if let Some(zone) = implied_zone {
                        calls.timezone = zone;
                    }
                }
                "timezone" => timezone = Some(time_zone(&value)?),
                "sleep" => calls.sleep = sleep_mode(&value)?,
                "sleep_system_max" => max = Some(sleep_system_max(&value)?),
                "random_start" => calls.random_start = random_start(&value)?,
                other => {
                    return Err(PyValueError::new_err(format!(
                        "unknown auto_os_calls key '{other}', expected one of: \
                         datetime, timezone, sleep, sleep_system_max, random_start"
                    )));
                }
            }
        }
        if let Some(timezone) = timezone {
            calls.timezone = timezone;
        }
        // The maximum only applies to a system sleep; the other modes ignore it.
        if let (Some(max), SleepMode::System(_)) = (max, calls.sleep) {
            calls.sleep = SleepMode::System(max);
        }
        Ok(Self(calls))
    }
}

/// `datetime`: `'system'`, `'call_host'`, or a `datetime.datetime` to freeze
/// the clock at. A `datetime` also implies the zone naive calls read in — its
/// `utcoffset()`, or UTC when naive — so `datetime.now()` returns it exactly.
fn datetime_source(value: &Bound<'_, PyAny>) -> PyResult<(DateTimeSource, Option<SandboxTimeZone>)> {
    if let Ok(name) = value.cast::<PyString>() {
        match &*name.to_cow()? {
            "system" => Ok((DateTimeSource::System, None)),
            "call_host" => Ok((DateTimeSource::CallHost, None)),
            other => Err(PyValueError::new_err(format!(
                "datetime must be 'system', 'call_host' or a datetime.datetime, got '{other}'"
            ))),
        }
    } else if let Ok(datetime) = value.cast::<PyDateTime>() {
        fixed_datetime(datetime).map(|(source, zone)| (source, Some(zone)))
    } else {
        Err(PyTypeError::new_err(format!(
            "datetime must be 'system', 'call_host' or a datetime.datetime, not {}",
            value.get_type().name()?
        )))
    }
}

/// A `datetime.datetime` as a frozen instant plus the zone it implies.
fn fixed_datetime(datetime: &Bound<'_, PyDateTime>) -> PyResult<(DateTimeSource, SandboxTimeZone)> {
    let py = datetime.py();
    let offset_seconds = match datetime
        .call_method0(intern!(py, "utcoffset"))?
        .extract::<Option<Bound<'_, PyDelta>>>()?
    {
        None => 0,
        Some(offset) => offset_seconds(&offset, "datetime utcoffset")?,
    };
    let name = datetime
        .call_method0(intern!(py, "tzname"))?
        .extract::<Option<String>>()?;
    let wall = NaiveDate::from_ymd_opt(
        datetime.get_year(),
        u32::from(datetime.get_month()),
        u32::from(datetime.get_day()),
    )
    .and_then(|date| {
        date.and_hms_opt(
            u32::from(datetime.get_hour()),
            u32::from(datetime.get_minute()),
            u32::from(datetime.get_second()),
        )
    })
    .ok_or_else(|| PyValueError::new_err("datetime is out of range"))?;
    let source = DateTimeSource::Fixed {
        unix_seconds: wall.and_utc().timestamp() - i64::from(offset_seconds),
        microsecond: datetime.get_microsecond(),
    };
    Ok((source, SandboxTimeZone::Fixed { offset_seconds, name }))
}

/// `timezone`: `'system'`, `'call_host'`, or a `{'offset_seconds': int,
/// 'name': str}` mapping (`name` optional).
fn time_zone(value: &Bound<'_, PyAny>) -> PyResult<SandboxTimeZone> {
    const SHAPE: &str = "timezone must be 'system', 'call_host' or {'offset_seconds': int, 'name': str}";
    if let Ok(name) = value.cast::<PyString>() {
        match &*name.to_cow()? {
            "system" => Ok(SandboxTimeZone::System),
            "call_host" => Ok(SandboxTimeZone::CallHost),
            other => Err(PyValueError::new_err(format!("{SHAPE}, got '{other}'"))),
        }
    } else if let Ok(mapping) = value.cast::<PyDict>() {
        let mut offset_seconds = None;
        let mut name = None;
        for (key, item) in mapping.iter() {
            let key = key
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("timezone keys must be str"))?
                .to_cow()?;
            match &*key {
                "offset_seconds" if item.cast::<PyBool>().is_err() && item.cast::<PyInt>().is_ok() => {
                    offset_seconds = Some(item.extract::<i32>()?);
                }
                "offset_seconds" => return Err(PyTypeError::new_err("timezone offset_seconds must be an int")),
                "name" if item.cast::<PyString>().is_ok() => name = Some(item.extract::<String>()?),
                "name" => return Err(PyTypeError::new_err("timezone name must be a str")),
                _ => return Err(PyValueError::new_err(format!("{SHAPE}, got {}", mapping.repr()?))),
            }
        }
        let Some(offset_seconds) = offset_seconds else {
            return Err(PyValueError::new_err(format!("{SHAPE}, got {}", mapping.repr()?)));
        };
        Ok(SandboxTimeZone::Fixed { offset_seconds, name })
    } else {
        Err(PyTypeError::new_err(format!(
            "{SHAPE}, not {}",
            value.get_type().name()?
        )))
    }
}

/// A `timedelta` as whole seconds, for a zone offset.
fn offset_seconds(offset: &Bound<'_, PyDelta>, what: &str) -> PyResult<i32> {
    if offset.get_microseconds() != 0 {
        return Err(PyValueError::new_err(format!(
            "{what} must be a whole number of seconds"
        )));
    }
    i32::try_from(i64::from(offset.get_days()) * 86_400 + i64::from(offset.get_seconds()))
        .map_err(|_| PyValueError::new_err(format!("{what} is out of range")))
}

/// `sleep`: `'system'` (with the default maximum until `sleep_system_max`
/// replaces it), `'call_host'` or `'zero'`.
fn sleep_mode(value: &Bound<'_, PyAny>) -> PyResult<SleepMode> {
    let name = value
        .cast::<PyString>()
        .map_err(|_| PyTypeError::new_err("sleep must be a str"))?;
    match &*name.to_cow()? {
        "system" => Ok(SleepMode::default()),
        "call_host" => Ok(SleepMode::CallHost),
        "zero" => Ok(SleepMode::Zero),
        other => Err(PyValueError::new_err(format!(
            "sleep must be 'system', 'call_host' or 'zero', got '{other}'"
        ))),
    }
}

/// `sleep_system_max`: seconds, `inf` for no cap. `bool` is refused
/// rather than read as 0/1.
fn sleep_system_max(value: &Bound<'_, PyAny>) -> PyResult<Duration> {
    if value.cast::<PyBool>().is_ok() || (value.cast::<PyInt>().is_err() && value.cast::<PyFloat>().is_err()) {
        return Err(PyTypeError::new_err(format!(
            "sleep_system_max must be a number of seconds, not {}",
            value.get_type().name()?
        )));
    }
    let seconds: f64 = value.extract()?;
    if seconds == f64::INFINITY {
        Ok(Duration::MAX)
    } else {
        duration_from_secs("sleep_system_max", seconds)
    }
}

/// `random_start`: `'system'`, `'call_host'`, or a `{'seed': ...}` mapping
/// whose seed is what `random.seed()` accepts.
fn random_start(value: &Bound<'_, PyAny>) -> PyResult<RandomStart> {
    const SHAPE: &str = "random_start must be 'system', 'call_host' or {'seed': int | float | str | bytes}";
    if let Ok(name) = value.cast::<PyString>() {
        match &*name.to_cow()? {
            "system" => Ok(RandomStart::System),
            "call_host" => Ok(RandomStart::CallHost),
            other => Err(PyValueError::new_err(format!("{SHAPE}, got '{other}'"))),
        }
    } else if let Ok(mapping) = value.cast::<PyDict>() {
        let py = value.py();
        let (1, Some(seed)) = (mapping.len(), mapping.get_item(intern!(py, "seed"))?) else {
            return Err(PyValueError::new_err(format!("{SHAPE}, got {}", mapping.repr()?)));
        };
        random_seed(&seed).map(RandomStart::Seed)
    } else {
        Err(PyTypeError::new_err(format!(
            "{SHAPE}, not {}",
            value.get_type().name()?
        )))
    }
}

/// A seed of any type `random.seed()` accepts; `bool` is refused since it
/// is never what a caller meant.
fn random_seed(seed: &Bound<'_, PyAny>) -> PyResult<RandomSeed> {
    if seed.cast::<PyBool>().is_ok() {
        Err(PyTypeError::new_err(
            "random_start seed must be an int, float, str or bytes, not bool",
        ))
    } else if let Ok(n) = seed.cast::<PyInt>() {
        Ok(RandomSeed::Int(n.extract::<BigInt>()?))
    } else if let Ok(f) = seed.cast::<PyFloat>() {
        Ok(RandomSeed::Float(f.value()))
    } else if let Ok(s) = seed.cast::<PyString>() {
        Ok(RandomSeed::Str(s.to_cow()?.into_owned()))
    } else if let Ok(b) = seed.cast::<PyBytes>() {
        Ok(RandomSeed::Bytes(b.as_bytes().to_vec()))
    } else {
        Err(PyTypeError::new_err(format!(
            "random_start seed must be an int, float, str or bytes, not {}",
            seed.get_type().name()?
        )))
    }
}
