//! Extraction of the `checkout()` arguments that build the session's
//! [`AutoOsCalls`]: `datetime`, `sleep`, `sandbox_sleep_clamp` and
//! `random_start`.
//!
//! Each argument is a newtype with a `FromPyObject` impl, so a bad value is
//! rejected at argument-extraction time with a message naming the accepted
//! forms, rather than surfacing later as a worker error.

use std::time::Duration;

use chrono::NaiveDate;
use monty_types::{AutoOsCalls, DateTimeSource, RandomSeed, RandomStart, SleepMode};
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

/// Builds the session's [`AutoOsCalls`] from the four checkout arguments.
/// The clamp only applies to a sandbox sleep; the other modes ignore it.
pub(crate) fn parse_auto_os_calls(
    datetime: DateTimeArg,
    sleep: SleepArg,
    sandbox_sleep_clamp: SleepClampArg,
    random_start: RandomStartArg,
) -> AutoOsCalls {
    let sleep = match sleep.0 {
        SleepMode::SandboxSleep(_) => SleepMode::SandboxSleep(sandbox_sleep_clamp.0),
        other => other,
    };
    AutoOsCalls {
        datetime: datetime.0,
        sleep,
        random_start: random_start.0,
    }
}

/// The `datetime` checkout argument: `'system'` (the default), `'call_host'`,
/// or a `datetime.datetime` to freeze the clock at.
#[derive(Clone, Copy, Default)]
pub(crate) struct DateTimeArg(pub DateTimeSource);

impl<'a, 'py> FromPyObject<'a, 'py> for DateTimeArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if let Ok(name) = ob.cast::<PyString>() {
            match &*name.to_cow()? {
                "system" => Ok(Self(DateTimeSource::System)),
                "call_host" => Ok(Self(DateTimeSource::CallHost)),
                other => Err(PyValueError::new_err(format!(
                    "datetime must be 'system', 'call_host' or a datetime.datetime, got '{other}'"
                ))),
            }
        } else if let Ok(datetime) = ob.cast::<PyDateTime>() {
            fixed_datetime(&datetime).map(Self)
        } else {
            Err(PyTypeError::new_err(format!(
                "datetime must be 'system', 'call_host' or a datetime.datetime, not {}",
                ob.get_type().name()?
            )))
        }
    }
}

/// A `datetime.datetime` as a frozen instant: an aware one is its instant with
/// the sandbox's local zone at its `utcoffset()`; a naive one is read as UTC,
/// so `datetime.now()` in the sandbox returns exactly the value given.
fn fixed_datetime(datetime: &Bound<'_, PyDateTime>) -> PyResult<DateTimeSource> {
    let py = datetime.py();
    let local_offset_seconds = match datetime
        .call_method0(intern!(py, "utcoffset"))?
        .extract::<Option<Bound<'_, PyDelta>>>()?
    {
        None => 0,
        Some(offset) => {
            if offset.get_microseconds() != 0 {
                return Err(PyValueError::new_err(
                    "datetime utcoffset must be a whole number of seconds",
                ));
            }
            i64::from(offset.get_days()) * 86_400 + i64::from(offset.get_seconds())
        }
    };
    let local_offset_seconds =
        i32::try_from(local_offset_seconds).map_err(|_| PyValueError::new_err("datetime utcoffset is out of range"))?;
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
    Ok(DateTimeSource::Fixed {
        unix_seconds: wall.and_utc().timestamp() - i64::from(local_offset_seconds),
        microsecond: datetime.get_microsecond(),
        local_offset_seconds,
    })
}

/// The `sleep` checkout argument: `'sandbox_sleep'` (the default), `'zero'`
/// or `'call_host'`. A sandbox sleep carries the default clamp here;
/// [`parse_auto_os_calls`] applies the `sandbox_sleep_clamp` argument.
#[derive(Clone, Copy, Default)]
pub(crate) struct SleepArg(pub SleepMode);

impl<'a, 'py> FromPyObject<'a, 'py> for SleepArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let name = ob
            .cast::<PyString>()
            .map_err(|_| PyTypeError::new_err("sleep must be a str"))?;
        match &*name.to_cow()? {
            "sandbox_sleep" => Ok(Self(SleepMode::default())),
            "zero" => Ok(Self(SleepMode::Zero)),
            "call_host" => Ok(Self(SleepMode::CallHost)),
            other => Err(PyValueError::new_err(format!(
                "sleep must be 'sandbox_sleep', 'zero' or 'call_host', got '{other}'"
            ))),
        }
    }
}

/// The `sandbox_sleep_clamp` checkout argument: seconds (default 10), `inf`
/// for no cap. `bool` is refused rather than read as 0/1.
#[derive(Clone, Copy)]
pub(crate) struct SleepClampArg(pub Duration);

impl Default for SleepClampArg {
    fn default() -> Self {
        Self(SleepMode::DEFAULT_CLAMP)
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for SleepClampArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if ob.cast::<PyBool>().is_ok() || (ob.cast::<PyInt>().is_err() && ob.cast::<PyFloat>().is_err()) {
            return Err(PyTypeError::new_err(format!(
                "sandbox_sleep_clamp must be a number of seconds, not {}",
                ob.get_type().name()?
            )));
        }
        let seconds: f64 = ob.extract()?;
        if seconds == f64::INFINITY {
            Ok(Self(Duration::MAX))
        } else {
            duration_from_secs("sandbox_sleep_clamp", seconds).map(Self)
        }
    }
}

/// The `random_start` checkout argument: `'random'` (the default) or a
/// `{'seed': ...}` mapping whose seed is what `random.seed()` accepts.
#[derive(Clone, Default)]
pub(crate) struct RandomStartArg(pub RandomStart);

impl<'a, 'py> FromPyObject<'a, 'py> for RandomStartArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        const SHAPE: &str = "random_start must be 'random' or {'seed': int | float | str | bytes}";
        if let Ok(name) = ob.cast::<PyString>() {
            match &*name.to_cow()? {
                "random" => Ok(Self(RandomStart::Random)),
                other => Err(PyValueError::new_err(format!("{SHAPE}, got '{other}'"))),
            }
        } else if let Ok(mapping) = ob.cast::<PyDict>() {
            let py = ob.py();
            let (1, Some(seed)) = (mapping.len(), mapping.get_item(intern!(py, "seed"))?) else {
                return Err(PyValueError::new_err(format!("{SHAPE}, got {}", mapping.repr()?)));
            };
            random_seed(&seed).map(|seed| Self(RandomStart::Seed(seed)))
        } else {
            Err(PyTypeError::new_err(format!("{SHAPE}, not {}", ob.get_type().name()?)))
        }
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
