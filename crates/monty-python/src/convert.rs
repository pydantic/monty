//! Type conversion between Monty's `MontyObject` and PyO3 Python objects.
//!
//! This module provides bidirectional conversion:
//! - `py_to_monty`: Convert Python objects to Monty's `MontyObject` for input
//! - `monty_to_py`: Convert Monty's `MontyObject` back to Python objects for output

use ::monty::{MontyDate, MontyDateTime, MontyObject, MontyTimeDelta, MontyTimeZone};
use monty::MontyException;
use num_bigint::BigInt;
use pyo3::{
    exceptions::{PyBaseException, PyTypeError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{
        PyBool, PyBytes, PyDate, PyDateAccess, PyDateTime, PyDelta, PyDeltaAccess, PyDict, PyFloat, PyFrozenSet, PyInt,
        PyList, PyModule, PySet, PyString, PyTimeAccess, PyTuple, PyTzInfo, PyTzInfoAccess,
    },
};

use crate::{
    dataclass::{DcRegistry, dataclass_to_monty, dataclass_to_py, is_dataclass},
    exceptions::{exc_monty_to_py, exc_to_monty_object},
};

/// Converts a Python object to Monty's `MontyObject` representation.
///
/// Handles all standard Python types that Monty supports as inputs, including callable objects
/// which are converted to `MontyObject::Function`. Unsupported types will raise a `TypeError`.
///
/// When a dataclass is encountered, it is automatically registered in `dc_registry`
/// so that the original Python type can be reconstructed on output (enabling `isinstance()`).
/// This applies recursively to nested dataclasses in fields, lists, dicts, etc.
///
/// # Important
/// Checks `bool` before `int` since `bool` is a subclass of `int` in Python.
/// Callable check is last since many Python types (classes, etc.) are technically callable.
pub fn py_to_monty(obj: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> PyResult<MontyObject> {
    if obj.is_none() {
        Ok(MontyObject::None)
    } else if let Ok(bool) = obj.cast::<PyBool>() {
        // Check bool BEFORE int since bool is a subclass of int in Python
        Ok(MontyObject::Bool(bool.is_true()))
    } else if let Ok(int) = obj.cast::<PyInt>() {
        // Try i64 first (fast path), fall back to BigInt for large values
        if let Ok(i) = int.extract::<i64>() {
            Ok(MontyObject::Int(i))
        } else {
            // Extract as BigInt for values that don't fit in i64
            let bi: BigInt = int.extract()?;
            Ok(MontyObject::BigInt(bi))
        }
    } else if let Ok(float) = obj.cast::<PyFloat>() {
        Ok(MontyObject::Float(float.extract()?))
    } else if let Ok(string) = obj.cast::<PyString>() {
        Ok(MontyObject::String(string.extract()?))
    } else if let Ok(bytes) = obj.cast::<PyBytes>() {
        Ok(MontyObject::Bytes(bytes.extract()?))
    } else if let Ok(list) = obj.cast::<PyList>() {
        let items: PyResult<Vec<MontyObject>> = list.iter().map(|item| py_to_monty(&item, dc_registry)).collect();
        Ok(MontyObject::List(items?))
    } else if let Ok(tuple) = obj.cast::<PyTuple>() {
        // Check for namedtuple BEFORE treating as regular tuple
        // Namedtuples have a `_fields` attribute with field names
        if let Ok(fields) = obj.getattr("_fields")
            && let Ok(fields_tuple) = fields.cast::<PyTuple>()
        {
            let py_type = obj.get_type();
            // Get the simple class name (e.g., "stat_result")
            let simple_name = py_type.name()?.to_string();
            // Get the module (e.g., "os" or "__main__")
            let module: String = py_type.getattr("__module__")?.extract()?;
            // Construct full type name: "os.stat_result"
            // Skip module prefix if it's a Python built-in module
            let type_name = if module.starts_with('_') || module == "builtins" {
                simple_name
            } else {
                format!("{module}.{simple_name}")
            };
            // Extract field names as strings
            let field_names: PyResult<Vec<String>> = fields_tuple.iter().map(|f| f.extract::<String>()).collect();
            // Extract values
            let values: PyResult<Vec<MontyObject>> = tuple.iter().map(|item| py_to_monty(&item, dc_registry)).collect();
            return Ok(MontyObject::NamedTuple {
                type_name,
                field_names: field_names?,
                values: values?,
            });
        }
        // Regular tuple
        let items: PyResult<Vec<MontyObject>> = tuple.iter().map(|item| py_to_monty(&item, dc_registry)).collect();
        Ok(MontyObject::Tuple(items?))
    } else if let Ok(dict) = obj.cast::<PyDict>() {
        // in theory we could provide a way of passing the iterator direct to the internal MontyObject construct
        // it's probably not worth it right now
        Ok(MontyObject::dict(
            dict.iter()
                .map(|(k, v)| Ok((py_to_monty(&k, dc_registry)?, py_to_monty(&v, dc_registry)?)))
                .collect::<PyResult<Vec<(MontyObject, MontyObject)>>>()?,
        ))
    } else if let Ok(set) = obj.cast::<PySet>() {
        let items: PyResult<Vec<MontyObject>> = set.iter().map(|item| py_to_monty(&item, dc_registry)).collect();
        Ok(MontyObject::Set(items?))
    } else if let Ok(frozenset) = obj.cast::<PyFrozenSet>() {
        let items: PyResult<Vec<MontyObject>> = frozenset.iter().map(|item| py_to_monty(&item, dc_registry)).collect();
        Ok(MontyObject::FrozenSet(items?))
    } else if obj.is(obj.py().Ellipsis()) {
        Ok(MontyObject::Ellipsis)
    } else if let Ok(datetime) = obj.cast::<PyDateTime>() {
        py_datetime_to_monty(datetime)
    } else if let Ok(date) = obj.cast::<PyDate>() {
        Ok(MontyObject::Date(MontyDate {
            year: date.get_year(),
            month: date.get_month(),
            day: date.get_day(),
        }))
    } else if let Ok(delta) = obj.cast::<PyDelta>() {
        Ok(MontyObject::TimeDelta(py_timedelta_to_monty(delta)))
    } else if obj.is_instance(get_datetime_timezone_type(obj.py())?)? {
        py_timezone_to_monty(obj).map(MontyObject::TimeZone)
    } else if let Ok(exc) = obj.cast::<PyBaseException>() {
        Ok(exc_to_monty_object(exc))
    } else if is_dataclass(obj) {
        // Auto-register the dataclass type so it can be reconstructed on output
        dc_registry.insert(&obj.get_type())?;
        dataclass_to_monty(obj, dc_registry)
    } else if obj.is_instance(get_pure_posix_path(obj.py())?)? {
        // Handle pathlib.PurePosixPath and thereby pathlib.PosixPath objects
        let path_str: String = obj.str()?.extract()?;
        Ok(MontyObject::Path(path_str))
    } else if obj.is_callable() {
        // Callable check is last since many Python types (classes, etc.) are technically callable,
        // and we want to match more specific types first (e.g. dataclasses).
        let name = get_name(obj);
        let docstring = get_docstring(obj);
        Ok(MontyObject::Function { name, docstring })
    } else if let Ok(name) = obj.get_type().qualname() {
        let msg = match obj.get_type().module() {
            Ok(module) => format!("Cannot convert {module}.{name} to Monty value"),
            Err(_) => format!("Cannot convert {name} to Monty value"),
        };
        Err(PyTypeError::new_err(msg))
    } else {
        Err(PyTypeError::new_err("Cannot convert unknown type to Monty value"))
    }
}

/// Converts Monty's `MontyObject` to a native Python object, using the dataclass registry.
///
/// When a dataclass is converted and its class name is found in the registry,
/// an instance of the original Python type is created (so `isinstance()` works).
/// Otherwise, falls back to `PyMontyDataclass`.
pub fn monty_to_py(py: Python<'_>, obj: &MontyObject, dc_registry: &DcRegistry) -> PyResult<Py<PyAny>> {
    match obj {
        MontyObject::None => Ok(py.None()),
        MontyObject::Ellipsis => Ok(py.Ellipsis()),
        MontyObject::Bool(b) => Ok(PyBool::new(py, *b).to_owned().into_any().unbind()),
        MontyObject::Int(i) => Ok(i.into_pyobject(py)?.clone().into_any().unbind()),
        MontyObject::BigInt(bi) => Ok(bi.into_pyobject(py)?.clone().into_any().unbind()),
        MontyObject::Float(f) => Ok(f.into_pyobject(py)?.clone().into_any().unbind()),
        MontyObject::String(s) => Ok(PyString::new(py, s).into_any().unbind()),
        MontyObject::Bytes(b) => Ok(PyBytes::new(py, b).into_any().unbind()),
        MontyObject::List(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> =
                items.iter().map(|item| monty_to_py(py, item, dc_registry)).collect();
            Ok(PyList::new(py, py_items?)?.into_any().unbind())
        }
        MontyObject::Tuple(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> =
                items.iter().map(|item| monty_to_py(py, item, dc_registry)).collect();
            Ok(PyTuple::new(py, py_items?)?.into_any().unbind())
        }
        // NamedTuple - create a proper Python namedtuple using collections.namedtuple
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => {
            // Extract module and simple name from full type_name
            // e.g., "os.stat_result" -> module="os", simple_name="stat_result"
            let (module, simple_name) = if let Some(idx) = type_name.rfind('.') {
                (&type_name[..idx], &type_name[idx + 1..])
            } else {
                ("", type_name.as_str())
            };

            // Create a namedtuple type with the module set for round-trip support
            // collections.namedtuple(typename, field_names, module=module)
            let namedtuple_fn = get_namedtuple(py)?;
            let py_field_names = PyList::new(py, field_names)?;
            let nt_type = if module.is_empty() {
                namedtuple_fn.call1((simple_name, py_field_names))?
            } else {
                let kwargs = PyDict::new(py);
                kwargs.set_item("module", module)?;
                namedtuple_fn.call((simple_name, py_field_names), Some(&kwargs))?
            };

            // Convert values and instantiate using _make() which accepts an iterable
            // note `_make` might start with an underscore, but it's a public documented method
            // https://docs.python.org/3/library/collections.html#collections.somenamedtuple._make
            let py_values: PyResult<Vec<Py<PyAny>>> =
                values.iter().map(|item| monty_to_py(py, item, dc_registry)).collect();
            let instance = nt_type.call_method1("_make", (py_values?,))?;
            Ok(instance.into_any().unbind())
        }
        MontyObject::Dict(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(monty_to_py(py, k, dc_registry)?, monty_to_py(py, v, dc_registry)?)?;
            }
            Ok(dict.into_any().unbind())
        }
        MontyObject::Set(items) => {
            let set = PySet::empty(py)?;
            for item in items {
                set.add(monty_to_py(py, item, dc_registry)?)?;
            }
            Ok(set.into_any().unbind())
        }
        MontyObject::FrozenSet(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> =
                items.iter().map(|item| monty_to_py(py, item, dc_registry)).collect();
            Ok(PyFrozenSet::new(py, &py_items?)?.into_any().unbind())
        }
        // Return the exception instance as a value (not raised)
        MontyObject::Exception { exc_type, arg } => {
            let exc = exc_monty_to_py(py, MontyException::new(*exc_type, arg.clone()));
            Ok(exc.into_value(py).into_any())
        }
        MontyObject::Date(date) => PyDate::new(py, date.year, date.month, date.day)
            .map(Bound::into_any)
            .map(Bound::unbind),
        MontyObject::DateTime(datetime) => monty_datetime_to_py(py, datetime),
        MontyObject::TimeDelta(delta) => PyDelta::new(py, delta.days, delta.seconds, delta.microseconds, true)
            .map(Bound::into_any)
            .map(Bound::unbind),
        MontyObject::TimeZone(timezone) => monty_timezone_to_py(py, timezone),
        // Return Python's built-in type object
        MontyObject::Type(t) => import_builtins(py)?.getattr(py, t.to_string()),
        MontyObject::BuiltinFunction(f) => import_builtins(py)?.getattr(py, f.to_string()),
        // Dataclass - use registry to reconstruct original type if available
        MontyObject::Dataclass {
            name,
            type_id,
            field_names,
            attrs,
            frozen,
        } => dataclass_to_py(py, name, *type_id, field_names, attrs, *frozen, dc_registry),
        // Path - convert to Python pathlib.Path
        MontyObject::Path(p) => {
            let pure_posix_path = get_pure_posix_path(py)?;
            let path_obj = pure_posix_path.call1((p,))?;
            Ok(path_obj.into_any().unbind())
        }
        // Output-only types - convert to string representation
        MontyObject::Repr(s) => Ok(PyString::new(py, s).into_any().unbind()),
        MontyObject::Cycle(_, placeholder) => Ok(PyString::new(py, placeholder).into_any().unbind()),
        // Function objects are internal to the name lookup protocol and should not normally
        // appear as final output values. If they do, represent as a string with the function name.
        MontyObject::Function { name, .. } => Ok(PyString::new(py, name).into_any().unbind()),
    }
}

pub fn import_builtins(py: Python<'_>) -> PyResult<&Py<PyModule>> {
    static BUILTINS: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

    BUILTINS.get_or_try_init(py, || py.import("builtins").map(Bound::unbind))
}

/// Converts a native Python `datetime.timedelta` to Monty's carrier representation.
fn py_timedelta_to_monty(delta: &Bound<'_, PyDelta>) -> MontyTimeDelta {
    MontyTimeDelta {
        days: delta.get_days(),
        seconds: delta.get_seconds(),
        microseconds: delta.get_microseconds(),
    }
}

/// Converts a Monty timezone payload to a native Python `datetime.timezone`.
fn monty_timezone_to_py(py: Python<'_>, timezone: &MontyTimeZone) -> PyResult<Py<PyAny>> {
    if timezone.offset_seconds == 0 && timezone.name.is_none() {
        return Ok(PyTzInfo::utc(py)?.to_owned().into_any().unbind());
    }

    let offset = PyDelta::new(py, 0, timezone.offset_seconds, 0, true)?;
    match timezone.name.as_deref() {
        None => PyTzInfo::fixed_offset(py, offset)
            .map(Bound::into_any)
            .map(Bound::unbind),
        Some(name) => get_datetime_timezone_type(py)?.call1((offset, name)).map(Bound::unbind),
    }
}

/// Converts a native Python `datetime.timezone` to Monty's carrier representation.
///
/// `timezone.__getinitargs__()` preserves whether the original Python object was
/// created with just an offset or with an explicit custom name, which is
/// important for Monty's repr/equality behavior.
fn py_timezone_to_monty(obj: &Bound<'_, PyAny>) -> PyResult<MontyTimeZone> {
    if obj.is(get_datetime_timezone_utc(obj.py())?) {
        return Ok(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        });
    }

    let init_args = obj.call_method0(intern!(obj.py(), "__getinitargs__"))?;
    let init_args = init_args.cast::<PyTuple>()?;

    Ok(MontyTimeZone {
        offset_seconds: timezone_offset_seconds(&py_timedelta_to_monty(
            &init_args.get_item(0)?.cast_into::<PyDelta>()?,
        ))?,
        name: init_args.get_item(1).and_then(|n| n.extract::<String>()).ok(),
    })
}

/// Converts a Monty datetime payload to a native Python `datetime.datetime`.
fn monty_datetime_to_py(py: Python<'_>, datetime: &MontyDateTime) -> PyResult<Py<PyAny>> {
    match (datetime.offset_seconds, &datetime.timezone_name) {
        (None, None) => PyDateTime::new(
            py,
            datetime.year,
            datetime.month,
            datetime.day,
            datetime.hour,
            datetime.minute,
            datetime.second,
            datetime.microsecond,
            None,
        )
        .map(Bound::into_any)
        .map(Bound::unbind),
        (Some(offset_seconds), timezone_name) => {
            let tzinfo_obj = monty_timezone_to_py(
                py,
                &MontyTimeZone {
                    offset_seconds,
                    name: timezone_name.clone(),
                },
            )?;
            let tzinfo = tzinfo_obj.bind(py).cast::<PyTzInfo>()?;
            PyDateTime::new(
                py,
                datetime.year,
                datetime.month,
                datetime.day,
                datetime.hour,
                datetime.minute,
                datetime.second,
                datetime.microsecond,
                Some(tzinfo),
            )
            .map(Bound::into_any)
            .map(Bound::unbind)
        }
        (None, Some(_)) => Err(PyTypeError::new_err(
            "invalid Monty datetime: timezone name without offset",
        )),
    }
}

/// Converts a native Python `datetime.datetime` to Monty's carrier representation.
///
/// For `datetime.timezone` tzinfo objects, uses `__getinitargs__()` to preserve
/// the explicit-vs-auto-generated name distinction. For other tzinfo types
/// (e.g. `zoneinfo.ZoneInfo`), falls back to the standard `utcoffset()`/`tzname()`
/// protocol on the datetime itself.
fn py_datetime_to_monty(datetime: &Bound<'_, PyDateTime>) -> PyResult<MontyObject> {
    let (offset_seconds, timezone_name) = if let Some(tzinfo) = datetime.get_tzinfo() {
        if tzinfo.is_instance(get_datetime_timezone_type(tzinfo.py())?)? {
            // datetime.timezone — use __getinitargs__ for round-trip fidelity
            let timezone = py_timezone_to_monty(&tzinfo)?;
            (Some(timezone.offset_seconds), timezone.name)
        } else {
            // Other tzinfo (e.g. zoneinfo.ZoneInfo) — use standard protocol
            py_tzinfo_via_utcoffset(datetime, &tzinfo)?
        }
    } else {
        (None, None)
    };

    Ok(MontyObject::DateTime(MontyDateTime {
        year: datetime.get_year(),
        month: datetime.get_month(),
        day: datetime.get_day(),
        hour: datetime.get_hour(),
        minute: datetime.get_minute(),
        second: datetime.get_second(),
        microsecond: datetime.get_microsecond(),
        offset_seconds,
        timezone_name,
    }))
}

/// Extracts timezone offset and name from a non-`datetime.timezone` tzinfo
/// (e.g. `zoneinfo.ZoneInfo`) using the standard `utcoffset()`/`tzname()` protocol.
///
/// Unlike `__getinitargs__()`, this always produces a name (since IANA timezones
/// always have one), so the name is stored as `Some(...)`.
fn py_tzinfo_via_utcoffset(
    datetime: &Bound<'_, PyDateTime>,
    tzinfo: &Bound<'_, PyAny>,
) -> PyResult<(Option<i32>, Option<String>)> {
    let py = tzinfo.py();
    let utcoffset = tzinfo
        .call_method1(intern!(py, "utcoffset"), (datetime,))?
        .cast_into::<PyDelta>()?;
    let offset = py_timedelta_to_monty(&utcoffset);
    let offset_seconds = timezone_offset_seconds(&offset)?;

    let name = tzinfo
        .call_method1(intern!(py, "tzname"), (datetime,))?
        .extract::<Option<String>>()?;

    Ok((Some(offset_seconds), name))
}

/// Converts a MontyTimeDelta to exact whole seconds for timezone offsets.
fn timezone_offset_seconds(delta: &MontyTimeDelta) -> PyResult<i32> {
    if delta.microseconds != 0 {
        return Err(PyTypeError::new_err(
            "datetime.timezone offset must be an exact number of whole seconds",
        ));
    }
    let total_seconds = i64::from(delta.days)
        .checked_mul(86_400)
        .and_then(|days| days.checked_add(i64::from(delta.seconds)))
        .ok_or_else(|| PyTypeError::new_err("datetime.timezone offset is out of range"))?;
    i32::try_from(total_seconds).map_err(|_| PyTypeError::new_err("datetime.timezone offset is out of range"))
}

/// Returns the Python `datetime.timezone` type object.
fn get_datetime_timezone_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static TIMEZONE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    TIMEZONE.import(py, "datetime", "timezone")
}

/// Returns Python's `datetime.timezone.utc` singleton.
fn get_datetime_timezone_utc(py: Python<'_>) -> PyResult<&Py<PyAny>> {
    static TIMEZONE_UTC: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    TIMEZONE_UTC.get_or_try_init(py, || {
        get_datetime_timezone_type(py)?
            .getattr(intern!(py, "utc"))
            .map(Bound::unbind)
    })
}

/// Cached import of `collections.namedtuple` function.
fn get_namedtuple(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static NAMEDTUPLE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    NAMEDTUPLE.import(py, "collections", "namedtuple")
}

/// Cached import of `pathlib.PurePosixPath` class.
fn get_pure_posix_path(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static PUREPOSIX: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    PUREPOSIX.import(py, "pathlib", "PurePosixPath")
}

pub fn get_name(f: &Bound<'_, PyAny>) -> String {
    f.getattr(intern!(f.py(), "__name__"))
        .and_then(|n| n.extract::<String>())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

/// get the `__doc__` attribute from a (hopefully) function
pub fn get_docstring(f: &Bound<'_, PyAny>) -> Option<String> {
    f.getattr(intern!(f.py(), "__doc__"))
        .and_then(|d| d.extract::<String>())
        .ok()
}
