//! Type conversion between Monty's `MontyObject` and PyO3 Python objects.
//!
//! This module provides bidirectional conversion:
//! - `py_to_monty`: Convert Python objects to Monty's `MontyObject` for input
//! - `monty_to_py`: Convert Monty's `MontyObject` back to Python objects for output

use ::monty::MontyObject;
use monty::MontyException;
use pyo3::{
    exceptions::PyBaseException,
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PySet, PyString, PyTuple},
};

use crate::{
    dataclass::{dataclass_to_monty, is_dataclass, PyMontyDataclass},
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
        Ok(MontyObject::Int(int.extract()?))
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
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
            "Cannot convert {} to Monty value",
            obj.get_type().name()?
        )))
    }
}

/// Converts Monty's `MontyObject` to a native Python object.
///
/// All Monty values can be converted to Python, including output-only
/// types like `Repr` which become strings.
pub fn monty_to_py(py: Python<'_>, obj: &MontyObject) -> PyResult<Py<PyAny>> {
    match obj {
        MontyObject::None => Ok(py.None()),
        MontyObject::Ellipsis => Ok(py.Ellipsis()),
        MontyObject::Bool(b) => Ok(PyBool::new(py, *b).to_owned().into_any().unbind()),
        MontyObject::Int(i) => Ok(i.into_pyobject(py)?.clone().into_any().unbind()),
        MontyObject::Float(f) => Ok(f.into_pyobject(py)?.clone().into_any().unbind()),
        MontyObject::String(s) => Ok(PyString::new(py, s).into_any().unbind()),
        MontyObject::Bytes(b) => Ok(PyBytes::new(py, b).into_any().unbind()),
        MontyObject::List(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> = items.iter().map(|item| monty_to_py(py, item)).collect();
            Ok(PyList::new(py, py_items?)?.into_any().unbind())
        }
        MontyObject::Tuple(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> = items.iter().map(|item| monty_to_py(py, item)).collect();
            Ok(PyTuple::new(py, py_items?)?.into_any().unbind())
        }
        MontyObject::Dict(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(monty_to_py(py, k)?, monty_to_py(py, v)?)?;
            }
            Ok(dict.into_any().unbind())
        }
        MontyObject::Set(items) => {
            let set = PySet::empty(py)?;
            for item in items {
                set.add(monty_to_py(py, item)?)?;
            }
            Ok(set.into_any().unbind())
        }
        MontyObject::FrozenSet(items) => {
            let py_items: PyResult<Vec<Py<PyAny>>> = items.iter().map(|item| monty_to_py(py, item)).collect();
            Ok(PyFrozenSet::new(py, &py_items?)?.into_any().unbind())
        }
        // Return the exception instance as a value (not raised)
        MontyObject::Exception { exc_type, arg } => {
            let exc = exc_monty_to_py(py, MontyException::new(*exc_type, arg.clone()));
            Ok(exc.into_value(py).into_any())
        }
        MontyObject::Type(t) => {
            // Return Python's built-in type object
            let type_name: &str = t.into();
            let builtins = py.import("builtins")?;
            Ok(builtins.getattr(type_name)?.unbind())
        }
        // Dataclass - convert to PyMontyDataclass
        MontyObject::Dataclass {
            name,
            field_names,
            attrs,
            frozen,
            methods: _,
        } => {
            let dc = PyMontyDataclass::new(py, name.clone(), field_names.clone(), attrs, *frozen)?;
            Ok(Py::new(py, dc)?.into_any())
        }
        // Output-only types - convert to string representation
        MontyObject::Repr(s) => Ok(PyString::new(py, s).into_any().unbind()),
        MontyObject::Cycle(_, placeholder) => Ok(PyString::new(py, placeholder).into_any().unbind()),
    }
}
