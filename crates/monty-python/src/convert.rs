//! Type conversion between Monty's `MontyObject` and PyO3 Python objects.
//!
//! This module provides bidirectional conversion:
//! - `py_to_monty`: Convert Python objects to Monty's `MontyObject` for input
//! - `monty_to_py`: Convert Monty's `MontyObject` back to Python objects for output

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ::monty::MontyObject;
use monty::MontyException;
use pyo3::exceptions::PyBaseException;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PySet, PyString, PyTuple, PyType};
use pyo3::{intern, prelude::*};

use crate::exceptions::{exc_monty_to_py, exc_to_monty_object};

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
            let exc = exc_monty_to_py(MontyException::new(*exc_type, arg.clone()));
            Ok(exc.into_value(py).into_any())
        }
        MontyObject::Type(t) => {
            // Return Python's built-in type object
            let type_name: &str = t.into();
            let builtins = py.import("builtins")?;
            Ok(builtins.getattr(type_name)?.unbind())
        }
        // Dataclass - convert to PyDataclass
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

/// Copied from is_dataclass in pydantic
fn is_dataclass(value: &Bound<'_, PyAny>) -> bool {
    value
        .hasattr(intern!(value.py(), "__dataclass_fields__"))
        .unwrap_or(false)
        && !value.is_instance_of::<PyType>()
}

/// Converts a Python dataclass instance to MontyObject.
///
/// Extracts field names in definition order (for repr) and all field values as attrs.
fn dataclass_to_monty(value: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
    let py = value.py();

    let name = value
        .get_type()
        .getattr(intern!(py, "__name__"))?
        .cast_into::<PyString>()?
        .to_str()?
        .to_string();

    let fields_dict = value
        .getattr(intern!(py, "__dataclass_fields__"))?
        .cast_into::<PyDict>()?;

    let frozen = value
        .getattr(intern!(py, "__dataclass_params__"))?
        .getattr(intern!(py, "frozen"))?
        .extract::<bool>()?;

    let field_type_marker = get_field_marker(py)?;

    // Collect field names and attrs
    let mut field_names = Vec::new();
    let mut attrs = Vec::new();

    for (field_name_obj, field) in fields_dict.iter() {
        let field_type = field.getattr(intern!(py, "_field_type"))?;
        if field_type.is(field_type_marker) {
            let field_name_str = field_name_obj.cast::<PyString>()?.to_str()?.to_string();
            let field_value = value.getattr(field_name_obj.cast::<PyString>()?)?;
            let field_name_monty = py_to_monty(&field_name_obj)?;
            let field_value_monty = py_to_monty(&field_value)?;

            field_names.push(field_name_str);
            attrs.push((field_name_monty, field_value_monty));
        }
    }

    Ok(MontyObject::Dataclass {
        name,
        field_names,
        attrs: attrs.into(),
        methods: vec![],
        frozen,
    })
}

/// Cached import of `dataclasses._FIELD` marker.
///
/// Used to match the logic from `dataclasses.fields()`:
/// `tuple(f for f in fields.values() if f._field_type is _FIELD)`
fn get_field_marker(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FIELD_MARKER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FIELD_MARKER.import(py, "dataclasses", "_FIELD")
}

/// Cached import of `dataclasses.MISSING` sentinel.
fn get_missing(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_MISSING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_MISSING.import(py, "dataclasses", "MISSING")
}

/// Cached import of `dataclasses.Field` class.
fn get_field_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FIELD_CLASS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FIELD_CLASS.import(py, "dataclasses", "Field")
}

/// Imitation of a dataclass to allow returning MontyObject::Dataclass to Python
/// as something that behaves like a dataclass.
///
/// Supports attribute access, repr, equality, and hashing (for frozen instances).
#[pyclass(name = "MontyDataclass")]
struct PyMontyDataclass {
    /// Class name (e.g., "Point", "User")
    name: String,
    /// Declared field names in definition order (for repr)
    field_names: Vec<String>,
    /// All attributes (fields + any extra attrs)
    attrs: Py<PyDict>,
    /// Whether this instance is frozen (immutable)
    frozen: bool,
}

#[pymethods]
impl PyMontyDataclass {
    /// Returns the class name.
    #[getter]
    fn __name__(&self) -> &str {
        &self.name
    }

    /// Returns the qualified name (same as __name__ since we don't track nesting).
    #[getter]
    fn __qualname__(&self) -> &str {
        &self.name
    }

    /// Returns a dict mapping field names to Field objects.
    ///
    /// This enables compatibility with `dataclasses.is_dataclass()`, `dataclasses.fields()`,
    /// `dataclasses.asdict()`, etc.
    #[getter]
    fn __dataclass_fields__(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let field_marker = get_field_marker(py)?;
        let missing = get_missing(py)?;
        let field_class = get_field_class(py)?;
        let attrs = self.attrs.bind(py);

        let fields_dict = PyDict::new(py);
        for field_name in &self.field_names {
            // Get the field value's type for the type annotation
            let field_type = if let Some(value) = attrs.get_item(field_name)? {
                value.get_type().into_any()
            } else {
                py.None().into_bound(py).get_type().into_any()
            };

            // Create a Field object with the required attributes
            // Field(default, default_factory, init, repr, hash, compare, metadata, kw_only, doc)
            let field_obj = field_class.call1((
                missing,   // default
                missing,   // default_factory
                true,      // init
                true,      // repr
                py.None(), // hash (None means use compare value)
                true,      // compare
                py.None(), // metadata
                false,     // kw_only
                py.None(), // doc
            ))?;

            // Set name and type (these are set after construction in real dataclasses)
            field_obj.setattr("name", field_name)?;
            field_obj.setattr("type", field_type)?;
            field_obj.setattr("_field_type", field_marker)?;

            fields_dict.set_item(field_name, field_obj)?;
        }
        Ok(fields_dict.unbind())
    }

    /// Get an attribute value.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let attrs = self.attrs.bind(py);
        match attrs.get_item(name)? {
            Some(value) => Ok(value.unbind()),
            None => Err(pyo3::exceptions::PyAttributeError::new_err(format!(
                "'{}' object has no attribute '{}'",
                self.name, name
            ))),
        }
    }

    /// Set an attribute value.
    fn __setattr__(&self, py: Python<'_>, name: &str, value: Py<PyAny>) -> PyResult<()> {
        if self.frozen {
            return Err(pyo3::exceptions::PyAttributeError::new_err(format!(
                "cannot assign to field '{name}'"
            )));
        }
        let attrs = self.attrs.bind(py);
        attrs.set_item(name, value)?;
        Ok(())
    }

    /// String representation: ClassName(field1=value1, field2=value2, ...)
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs = self.attrs.bind(py);
        let mut parts = Vec::new();
        for field_name in &self.field_names {
            if let Some(value) = attrs.get_item(field_name)? {
                let value_repr: String = value.repr()?.extract()?;
                parts.push(format!("{field_name}={value_repr}"));
            }
        }
        Ok(format!("{}({})", self.name, parts.join(", ")))
    }

    /// Equality comparison.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        // Check if other is also a PyDataclass
        if let Ok(other_dc) = other.extract::<PyRef<'_, PyMontyDataclass>>() {
            if self.name != other_dc.name {
                return Ok(false);
            }
            let self_attrs = self.attrs.bind(py);
            let other_attrs = other_dc.attrs.bind(py);
            // Compare all attrs
            self_attrs.eq(other_attrs)
        } else {
            Ok(false)
        }
    }

    /// Hash (only for frozen dataclasses).
    fn __hash__(&self, py: Python<'_>) -> PyResult<isize> {
        if !self.frozen {
            return Err(pyo3::exceptions::PyTypeError::new_err(format!(
                "unhashable type: '{}'",
                self.name
            )));
        }

        let mut hasher = DefaultHasher::new();
        self.name.hash(&mut hasher);

        let attrs = self.attrs.bind(py);
        for field_name in &self.field_names {
            field_name.hash(&mut hasher);
            if let Some(value) = attrs.get_item(field_name)? {
                let value_hash: isize = value.hash()?;
                value_hash.hash(&mut hasher);
            }
        }
        Ok(hasher.finish() as isize)
    }
}

impl PyMontyDataclass {
    /// Creates a new PyDataclass from MontyObject fields.
    fn new<'a>(
        py: Python<'_>,
        name: String,
        field_names: Vec<String>,
        attrs: impl IntoIterator<Item = &'a (MontyObject, MontyObject)>,
        frozen: bool,
    ) -> PyResult<Self> {
        let dict = PyDict::new(py);
        for (k, v) in attrs {
            dict.set_item(monty_to_py(py, k)?, monty_to_py(py, v)?)?;
        }
        Ok(Self {
            name,
            field_names,
            attrs: dict.unbind(),
            frozen,
        })
    }
}
