//! Type conversion between Monty's `MontyObject` and PyO3 Python objects.
//!
//! This module provides bidirectional conversion:
//! - `py_to_monty`: Convert Python objects to Monty's `MontyObject` for input
//! - `monty_to_py`: Convert Monty's `MontyObject` back to Python objects for output

use ::monty::MontyObject;
use monty::MontyException;
use num_bigint::BigInt;
use pyo3::{
    exceptions::{PyBaseException, PyTypeError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyBool, PyBytes, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PySet, PyString, PyTuple},
};

use crate::{
    dataclass::{dataclass_to_monty, dataclass_to_py, is_dataclass},
    exceptions::{exc_monty_to_py, exc_to_monty_object},
};

/// Converts a Python object to Monty's `MontyObject` representation.
///
/// Handles all standard Python types that Monty supports as inputs.
/// Unsupported types will raise a `TypeError`.
///
/// # Important
/// Checks `bool` before `int` since `bool` is a subclass of `int` in Python.
pub fn py_to_monty(obj: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
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
        let items: PyResult<Vec<MontyObject>> = list.iter().map(|item| py_to_monty(&item)).collect();
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
            let values: PyResult<Vec<MontyObject>> = tuple.iter().map(|item| py_to_monty(&item)).collect();
            return Ok(MontyObject::NamedTuple {
                type_name,
                field_names: field_names?,
                values: values?,
            });
        }
        // Regular tuple
        let items: PyResult<Vec<MontyObject>> = tuple.iter().map(|item| py_to_monty(&item)).collect();
        Ok(MontyObject::Tuple(items?))
    } else if let Ok(dict) = obj.cast::<PyDict>() {
        // in theory we could provide a way of passing the iterator direct to the internal MontyObject construct
        // it's probably not worth it right now
        Ok(MontyObject::dict(
            dict.iter()
                .map(|(k, v)| Ok((py_to_monty(&k)?, py_to_monty(&v)?)))
                .collect::<PyResult<Vec<(MontyObject, MontyObject)>>>()?,
        ))
    } else if let Ok(set) = obj.cast::<PySet>() {
        let items: PyResult<Vec<MontyObject>> = set.iter().map(|item| py_to_monty(&item)).collect();
        Ok(MontyObject::Set(items?))
    } else if let Ok(frozenset) = obj.cast::<PyFrozenSet>() {
        let items: PyResult<Vec<MontyObject>> = frozenset.iter().map(|item| py_to_monty(&item)).collect();
        Ok(MontyObject::FrozenSet(items?))
    } else if obj.is(obj.py().Ellipsis()) {
        Ok(MontyObject::Ellipsis)
    } else if let Ok(exc) = obj.cast::<PyBaseException>() {
        Ok(exc_to_monty_object(exc))
    } else if is_dataclass(obj) {
        dataclass_to_monty(obj)
    } else if obj.is_instance(get_pure_posix_path(obj.py())?)? {
        // Handle pathlib.PurePosixPath and thereby pathlib.PosixPath objects
        let path_str: String = obj.str()?.extract()?;
        Ok(MontyObject::Path(path_str))
    } else if let Ok(name) = obj.get_type().name() {
        Err(PyTypeError::new_err(format!("Cannot convert {name} to Monty value")))
    } else {
        Err(PyTypeError::new_err("Cannot convert unknown type to Monty value"))
    }
}

/// Converts Monty's `MontyObject` to a native Python object, using the dataclass registry.
///
/// When a dataclass is converted and its class name is found in the registry,
/// an instance of the original Python type is created (so `isinstance()` works).
/// Otherwise, falls back to `PyMontyDataclass`.
pub fn monty_to_py(py: Python<'_>, obj: &MontyObject, dc_registry: &Bound<'_, PyDict>) -> PyResult<Py<PyAny>> {
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
            methods: _,
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
    }
}

pub fn import_builtins(py: Python<'_>) -> PyResult<&Py<PyModule>> {
    static BUILTINS: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

    BUILTINS.get_or_try_init(py, || py.import("builtins").map(Bound::unbind))
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
