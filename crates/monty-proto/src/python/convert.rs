//! Bidirectional conversion between Monty's `MontyObject` and PyO3 Python
//! objects: `py_to_monty` for inputs, `monty_to_py` for outputs.

use std::borrow::Cow;

use monty::{
    FileMode, MontyDate, MontyDateTime, MontyException, MontyFileHandle, MontyObject, MontyTimeDelta, MontyTimeZone,
    MontyType, StringRepr,
};
use num_bigint::BigInt;
use pyo3::{
    exceptions::{PyBaseException, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{
        PyBool, PyBytes, PyDate, PyDateAccess, PyDateTime, PyDelta, PyDeltaAccess, PyDict, PyFloat, PyFrozenSet, PyInt,
        PyList, PyModule, PySet, PyString, PyTimeAccess, PyTuple, PyType, PyTzInfo, PyTzInfoAccess,
    },
};

use super::{
    dataclass::{DcRegistry, dataclass_to_monty, dataclass_to_py, is_dataclass},
    exceptions::{exc_monty_to_py, exc_py_to_monty, exc_to_monty_object},
};
use crate::MAX_VALUE_DEPTH;

/// Depth limit for converting host values INTO the sandbox: values must fit
/// the wire protocol, whose decoder caps nesting (see [`MAX_VALUE_DEPTH`]) —
/// checking here gives the caller a clean `Max input depth exceeded` error
/// before anything is sent to a worker.
#[expect(clippy::cast_possible_truncation, reason = "MAX_VALUE_DEPTH is 48")]
const MAX_INPUT_DEPTH: u8 = MAX_VALUE_DEPTH as u8;
/// Depth limit when converting sandbox values back to Python objects; values
/// arriving over the wire are already bounded well below this, so it is a
/// pure defence-in-depth backstop.
const MAX_DEPTH: u8 = 200;

/// Like `py_to_monty`, but converts any `PyErr` into a `MontyException`.
///
/// Use this at every boundary where an untrusted host value flows into Monty
/// (inputs, external/OS return values, snapshot resume values). Callers then
/// wrap the `MontyException` as they see fit — `MontyError::new_err(py, e)` for
/// Python-API returns, or `ExtFunctionResult::Error(e)` for mid-execution
/// dispatch — so raw PyO3 errors like `UnicodeEncodeError` never escape.
pub fn py_to_monty_value(obj: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> Result<MontyObject, MontyException> {
    py_to_monty(obj, dc_registry, 0).map_err(|e| exc_py_to_monty(obj.py(), &e))
}

/// Converts a Python object to Monty's `MontyObject` representation; unsupported
/// types raise `TypeError`.
///
/// Dataclasses (including nested ones) are auto-registered in `dc_registry` so
/// the original Python type can be reconstructed on output (enabling
/// `isinstance()`).
///
/// Match order matters: `bool` before `int` (subclass), and the generic
/// callable check is last since many types (classes, etc.) are callable.
pub fn py_to_monty(obj: &Bound<'_, PyAny>, dc_registry: &DcRegistry, mut depth: u8) -> PyResult<MontyObject> {
    depth += 1;
    if depth > MAX_INPUT_DEPTH {
        Err(PyRuntimeError::new_err("Max input depth exceeded"))
    } else if obj.is_none() {
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
        let items: PyResult<Vec<MontyObject>> =
            list.iter().map(|item| py_to_monty(&item, dc_registry, depth)).collect();
        Ok(MontyObject::List(items?))
    } else if let Ok(tuple) = obj.cast::<PyTuple>() {
        // namedtuples (detected by their `_fields` attribute) carry their type
        // name, so check before treating as a regular tuple.
        if let Ok(fields) = obj.getattr("_fields")
            && let Ok(fields_tuple) = fields.cast::<PyTuple>()
        {
            let py_type = obj.get_type();
            let simple_name = py_type.name()?.to_string();
            let module: String = py_type.getattr("__module__")?.extract()?;
            // Build the full type name (e.g. "os.stat_result"), dropping the
            // module prefix for built-ins.
            let type_name = if module.starts_with('_') || module == "builtins" {
                simple_name
            } else {
                format!("{module}.{simple_name}")
            };
            let field_names: PyResult<Vec<String>> = fields_tuple.iter().map(|f| f.extract::<String>()).collect();
            let values: PyResult<Vec<MontyObject>> = tuple
                .iter()
                .map(|item| py_to_monty(&item, dc_registry, depth))
                .collect();
            return Ok(MontyObject::NamedTuple {
                type_name,
                field_names: field_names?,
                values: values?,
            });
        }
        let items: PyResult<Vec<MontyObject>> = tuple
            .iter()
            .map(|item| py_to_monty(&item, dc_registry, depth))
            .collect();
        Ok(MontyObject::Tuple(items?))
    } else if let Ok(dict) = obj.cast::<PyDict>() {
        // in theory we could provide a way of passing the iterator direct to the internal MontyObject construct
        // it's probably not worth it right now
        Ok(MontyObject::dict(
            dict.iter()
                .map(|(k, v)| {
                    Ok((
                        py_to_monty(&k, dc_registry, depth)?,
                        py_to_monty(&v, dc_registry, depth)?,
                    ))
                })
                .collect::<PyResult<Vec<(MontyObject, MontyObject)>>>()?,
        ))
    } else if let Ok(set) = obj.cast::<PySet>() {
        let items: PyResult<Vec<MontyObject>> = set.iter().map(|item| py_to_monty(&item, dc_registry, depth)).collect();
        Ok(MontyObject::Set(items?))
    } else if let Ok(frozenset) = obj.cast::<PyFrozenSet>() {
        let items: PyResult<Vec<MontyObject>> = frozenset
            .iter()
            .map(|item| py_to_monty(&item, dc_registry, depth))
            .collect();
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
        dataclass_to_monty(obj, dc_registry, depth)
    } else if obj.is_instance(get_pure_posix_path(obj.py())?)? {
        // Handle pathlib.PurePosixPath and thereby pathlib.PosixPath objects
        let path_str: String = obj.str()?.extract()?;
        Ok(MontyObject::Path(path_str))
    } else if let Ok(handle) = obj.cast::<PyMontyFileHandle>() {
        // Round-trip a `MontyFileHandle` returned from Python (e.g. as the
        // result of an `Open` OS callback) back into `MontyObject::FileHandle`.
        Ok(MontyObject::FileHandle(handle.borrow().0.clone()))
    } else if let Ok(ty) = obj.cast::<PyType>() {
        // A class is callable, so it would otherwise fall into the generic callable
        // branch below. Classes Monty models are preserved as type objects (so they
        // round-trip and `isinstance` works in the sandbox); any other host class has
        // no Monty `Type`, so it falls back to the callable representation.
        match py_type_object_to_monty(ty)? {
            Some(t) => Ok(MontyObject::Type(t)),
            None => Ok(callable_to_monty_function(obj)),
        }
    } else if obj.is_callable() {
        // Callable check is last since many Python types (classes, etc.) are technically callable,
        // and we want to match more specific types first (e.g. dataclasses).
        Ok(callable_to_monty_function(obj))
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

/// Inverse of [`type_object_to_py`]: maps a host class passed *into* the sandbox
/// to the Monty [`MontyType`] it represents, so it round-trips instead of degrading to
/// a callable. Matches by type-object **identity**, not `__module__`/`__name__` —
/// the latter is spoofable and churns across Python versions (e.g. `pathlib` paths
/// report `pathlib._local` on 3.13). Every `pathlib` path class collapses to
/// [`MontyType::Path`]. Returns `None` for classes Monty does not model, which the
/// caller then represents as a [`MontyObject::Function`].
fn py_type_object_to_monty(ty: &Bound<'_, PyType>) -> PyResult<Option<MontyType>> {
    let py = ty.py();
    for (obj, t) in round_trip_type_table(py)? {
        if ty.is(obj) {
            return Ok(Some(t.clone()));
        }
    }
    // pathlib's concrete path classes (PurePath, PosixPath, …) all subclass
    // PurePath and collapse to one Monty path type.
    Ok(ty.is_subclass(get_pure_path(py)?)?.then_some(MontyType::Path))
}

/// Host type objects that round-trip into the sandbox, each paired with its Monty
/// [`MontyType`]. Built once and cached. Identities are taken from [`type_object_to_py`]
/// so the two directions stay in lock-step. [`MontyType::Path`] is handled separately
/// (by subclass check) since pathlib exposes several concrete path classes.
fn round_trip_type_table(py: Python<'_>) -> PyResult<&'static Vec<(Py<PyAny>, MontyType)>> {
    static TABLE: PyOnceLock<Vec<(Py<PyAny>, MontyType)>> = PyOnceLock::new();
    TABLE.get_or_try_init(py, || {
        [
            MontyType::NoneType,
            MontyType::Ellipsis,
            MontyType::Bool,
            MontyType::Int,
            MontyType::Float,
            MontyType::Str,
            MontyType::Bytes,
            MontyType::List,
            MontyType::ListIterator,
            MontyType::Tuple,
            MontyType::Dict,
            MontyType::Set,
            MontyType::FrozenSet,
            MontyType::Range,
            MontyType::Slice,
            MontyType::Type,
            MontyType::Property,
            MontyType::Date,
            MontyType::DateTime,
            MontyType::TimeDelta,
            MontyType::TimeZone,
            MontyType::RePattern,
            MontyType::ReMatch,
            MontyType::TextIOWrapper,
            MontyType::BufferedReader,
            MontyType::BufferedWriter,
            MontyType::BufferedRandom,
            MontyType::SpecialForm,
        ]
        .into_iter()
        .map(|t| Ok((type_object_to_py(py, t.clone())?, t)))
        .collect()
    })
}

/// Represents a host callable with no richer Monty mapping as a
/// [`MontyObject::Function`], carrying its `__name__` and docstring. Used for
/// plain callables and for host classes Monty does not model.
fn callable_to_monty_function(obj: &Bound<'_, PyAny>) -> MontyObject {
    MontyObject::Function {
        name: get_name(obj),
        docstring: get_docstring(obj),
    }
}

/// Converts Monty's `MontyObject` to a native Python object. A dataclass found
/// in `dc_registry` reconstructs the original Python type (so `isinstance()`
/// works), otherwise it falls back to `PyMontyDataclass`.
pub fn monty_to_py(py: Python<'_>, obj: &MontyObject, dc_registry: &DcRegistry) -> PyResult<Py<PyAny>> {
    monty_to_py_inner(py, obj, dc_registry, 0)
}

/// Recursive worker for [`monty_to_py`] that threads a native-stack depth counter.
///
/// `depth` is the current nesting level on entry; the function bumps it before
/// processing and raises `RuntimeError` once it exceeds [`MAX_DEPTH`]. This
/// prevents adversarial input — e.g. deeply nested tuples built in a `for`
/// loop that never push a Python call frame — from overflowing the Rust call
/// stack and aborting the host process.
pub(crate) fn monty_to_py_inner(
    py: Python<'_>,
    obj: &MontyObject,
    dc_registry: &DcRegistry,
    mut depth: u8,
) -> PyResult<Py<PyAny>> {
    depth += 1;
    if depth > MAX_DEPTH {
        return Err(PyRuntimeError::new_err("Max output depth exceeded"));
    }
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
            let py_items: PyResult<Vec<Py<PyAny>>> = items
                .iter()
                .map(|item| monty_to_py_inner(py, item, dc_registry, depth))
                .collect();
            Ok(PyList::new(py, py_items?)?.into_any().unbind())
        }
        MontyObject::Tuple(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> = items
                .iter()
                .map(|item| monty_to_py_inner(py, item, dc_registry, depth))
                .collect();
            Ok(PyTuple::new(py, py_items?)?.into_any().unbind())
        }
        // Rebuild a real Python namedtuple via collections.namedtuple.
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => {
            // Split the full type_name (e.g. "os.stat_result") into module + name.
            let (module, simple_name) = if let Some(idx) = type_name.rfind('.') {
                (&type_name[..idx], &type_name[idx + 1..])
            } else {
                ("", type_name.as_str())
            };

            // Set `module=` on the type so it round-trips back through py_to_monty.
            let namedtuple_fn = get_namedtuple(py)?;
            let py_field_names = PyList::new(py, field_names)?;
            let nt_type = if module.is_empty() {
                namedtuple_fn.call1((simple_name, py_field_names))?
            } else {
                let kwargs = PyDict::new(py);
                kwargs.set_item("module", module)?;
                namedtuple_fn.call((simple_name, py_field_names), Some(&kwargs))?
            };

            // `_make` is a public documented method despite the leading underscore.
            let py_values: PyResult<Vec<Py<PyAny>>> = values
                .iter()
                .map(|item| monty_to_py_inner(py, item, dc_registry, depth))
                .collect();
            let instance = nt_type.call_method1("_make", (py_values?,))?;
            Ok(instance.into_any().unbind())
        }
        MontyObject::Dict(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(
                    monty_to_py_inner(py, k, dc_registry, depth)?,
                    monty_to_py_inner(py, v, dc_registry, depth)?,
                )?;
            }
            Ok(dict.into_any().unbind())
        }
        MontyObject::Set(items) => {
            let set = PySet::empty(py)?;
            for item in items {
                set.add(monty_to_py_inner(py, item, dc_registry, depth)?)?;
            }
            Ok(set.into_any().unbind())
        }
        MontyObject::FrozenSet(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> = items
                .iter()
                .map(|item| monty_to_py_inner(py, item, dc_registry, depth))
                .collect();
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
        // Return the host Python type object the sandbox type maps to.
        MontyObject::Type(t) => type_object_to_py(py, t.clone()),
        MontyObject::BuiltinFunction(f) => import_builtins(py)?.getattr(py, f.to_string()),
        // Dataclass - use registry to reconstruct original type if available
        MontyObject::Dataclass {
            name,
            type_id,
            field_names,
            attrs,
            frozen,
        } => dataclass_to_py(py, name, *type_id, field_names, attrs, *frozen, dc_registry, depth),
        // Path - convert to Python pathlib.Path
        MontyObject::Path(p) => {
            let pure_posix_path = get_pure_posix_path(py)?;
            let path_obj = pure_posix_path.call1((p,))?;
            Ok(path_obj.into_any().unbind())
        }
        // A Monty file object has no faithful host-Python representation
        // (it is not a real OS file). Surface it as a `MontyFileHandle` so
        // callers can inspect `path`, `mode`, `position`, and `id` directly
        // instead of parsing the repr string.
        MontyObject::FileHandle(handle) => Ok(Py::new(py, PyMontyFileHandle::from_inner(handle.clone()))?.into_any()),
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

/// Reconstructs the host Python *type object* for a Monty [`MontyType`] crossing the
/// boundary as a value (e.g. sandbox code passing `type(Path('/x'))` to a host call).
///
/// Genuine builtins resolve from `builtins`; modeled stdlib types resolve from their
/// real defining module (the `Path` class maps to `PurePosixPath`, like its instances).
/// The import path can differ from [`MontyType`]'s `Display` (io types show `_io.*` but
/// live in `io`). Unmodeled types fall through to `builtins` and raise `AttributeError`.
/// Each modeled type's host class is cached in its own `PyOnceLock` (imported once).
fn type_object_to_py(py: Python<'_>, t: MontyType) -> PyResult<Py<PyAny>> {
    // Each expansion gets a distinct hygienic `LOCK` static, so every arm caches
    // its own resolved type object. `PyOnceLock::import` imports + getattrs once.
    macro_rules! cached {
        ($module:literal, $name:literal) => {{
            static LOCK: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
            LOCK.import(py, $module, $name).map(|b| b.clone().unbind())
        }};
    }
    match t {
        MontyType::Date => cached!("datetime", "date"),
        MontyType::DateTime => cached!("datetime", "datetime"),
        MontyType::TimeDelta => cached!("datetime", "timedelta"),
        MontyType::TimeZone => cached!("datetime", "timezone"),
        MontyType::ListIterator => get_list_iterator_type(py).map(|b| b.clone().unbind()),
        // Consistent with the Path *instance* arm, which marshals as PurePosixPath
        // and is instantiable on every host OS (unlike PosixPath on Windows).
        MontyType::Path => get_pure_posix_path(py).map(|b| b.clone().unbind()),
        MontyType::RePattern => cached!("re", "Pattern"),
        MontyType::ReMatch => cached!("re", "Match"),
        MontyType::TextIOWrapper => cached!("io", "TextIOWrapper"),
        MontyType::BufferedReader => cached!("io", "BufferedReader"),
        MontyType::BufferedWriter => cached!("io", "BufferedWriter"),
        MontyType::BufferedRandom => cached!("io", "BufferedRandom"),
        MontyType::SpecialForm => cached!("typing", "_SpecialForm"),
        // `NoneType` and `ellipsis` aren't `builtins` attributes; take them from
        // the singletons (`type(None)` / `type(...)`).
        MontyType::NoneType => Ok(py.None().bind(py).get_type().into_any().unbind()),
        MontyType::Ellipsis => Ok(py.Ellipsis().bind(py).get_type().into_any().unbind()),
        // A sandbox-defined class has no host type object to map to.
        MontyType::Instance(name) => Err(PyValueError::new_err(format!(
            "cannot convert sandbox-defined class '{name}' to a host type object"
        ))),
        _ => import_builtins(py)?.getattr(py, t.to_string()),
    }
}

/// Returns CPython's private `list_iterator` type without relying on a module attribute.
fn get_list_iterator_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    TYPE.get_or_try_init(py, || Ok(PyList::empty(py).try_iter()?.get_type().into_any().unbind()))
        .map(|ty| ty.bind(py))
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

/// Cached import of `pathlib.PurePath` — the common base of every path class,
/// used to recognise any path type passed into the sandbox.
fn get_pure_path(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static PUREPATH: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    PUREPATH.import(py, "pathlib", "PurePath")
}

/// Host-side mirror of [`MontyObject::FileHandle`]: a thin PyO3 wrapper holding
/// the same [`MontyFileHandle`] value the interpreter does.
///
/// A Python host sees one when a sandbox-opened file flows back across the
/// boundary (e.g. the return of an `Open` OS callback, or the first argument to
/// a `read`/`write` callback). It is a plain data holder — the runtime
/// guarantees the host never owns a live OS file descriptor for a Monty file,
/// so there is nothing to clean up.
///
/// Fields are read-only via getters; `binary`/`readable`/`writable` are derived
/// from the underlying [`FileMode`] on demand.
#[pyclass(name = "MontyFileHandle", module = "pydantic_monty", frozen)]
pub struct PyMontyFileHandle(MontyFileHandle);

impl PyMontyFileHandle {
    /// Wraps an existing [`MontyFileHandle`] for surfacing back to Python,
    /// reusing the interpreter's value instead of repacking its fields.
    pub(crate) fn from_inner(inner: MontyFileHandle) -> Self {
        Self(inner)
    }
}

#[pymethods]
impl PyMontyFileHandle {
    /// Constructs a `MontyFileHandle` from Python.
    ///
    /// `mode` is parsed via [`FileMode::from_str`] and rewritten to its
    /// canonical form, so `MontyFileHandle('/x', 'rt').mode == 'r'`. This
    /// is the path Python callbacks use to return file handles from the
    /// `Open` OS function.
    #[new]
    #[pyo3(signature = (path, mode, *, position = 0))]
    fn py_new(path: String, mode: &str, position: u64) -> PyResult<Self> {
        let mode: FileMode = mode
            .parse()
            .map_err(|e: Cow<'static, str>| PyValueError::new_err(e.to_string()))?;
        Ok(Self::from_inner(MontyFileHandle { path, mode, position }))
    }

    /// Virtual sandbox path of the open file. Always POSIX-style; never a host path.
    #[getter]
    fn path(&self) -> &str {
        &self.0.path
    }

    /// Canonical `open()` mode string (e.g. `'r'`, `'rb'`, `'w+'`).
    #[getter]
    fn mode(&self) -> &'static str {
        self.0.mode.as_str()
    }

    /// Current position for sized/line/seek operations: char index in text
    /// mode, byte index in binary mode. `0` for freshly opened files.
    #[getter]
    fn position(&self) -> u64 {
        self.0.position
    }

    /// `True` if the underlying mode opens the file in binary form (`'rb'`, `'wb'`, …).
    #[getter]
    fn binary(&self) -> bool {
        self.0.mode.is_binary()
    }

    /// `True` if the file's mode permits `read()`.
    #[getter]
    fn readable(&self) -> bool {
        self.0.mode.readable()
    }

    /// `True` if the file's mode permits `write()`.
    #[getter]
    fn writable(&self) -> bool {
        self.0.mode.writable()
    }

    fn __repr__(&self) -> String {
        format!(
            "MontyFileHandle(path={}, mode={})",
            StringRepr(&self.0.path),
            StringRepr(self.0.mode.as_str())
        )
    }
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
