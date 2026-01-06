//! Dataclass conversion between Python and Monty.
//!
//! This module handles:
//! - Converting Python dataclass instances to `MontyObject::Dataclass`
//! - Converting `MontyObject::Dataclass` back to Python via `PyMontyDataclass`
//! - `PyMontyDataclass`: A Python class that mimics dataclass behavior

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ::monty::MontyObject;
use pyo3::{
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyString, PyType},
};

use crate::convert::{monty_to_py, py_to_monty};

/// Checks if a Python object is a dataclass instance (not a type).
///
/// Copied from pydantic's `is_dataclass` logic.
pub fn is_dataclass(value: &Bound<'_, PyAny>) -> bool {
    value
        .hasattr(intern!(value.py(), "__dataclass_fields__"))
        .unwrap_or(false)
        && !value.is_instance_of::<PyType>()
}

/// Converts a Python dataclass instance to `MontyObject::Dataclass`.
///
/// Extracts field names in definition order (for repr) and all field values as attrs.
pub fn dataclass_to_monty(value: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
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

/// Python class that mimics dataclass behavior for `MontyObject::Dataclass`.
///
/// Supports:
/// - Attribute access (`__getattr__`, `__setattr__`)
/// - String representation (`__repr__`, `__str__`)
/// - Equality comparison (`__eq__`)
/// - Hashing for frozen instances (`__hash__`)
/// - `dataclasses` module compatibility (`__dataclass_fields__`)
#[pyclass(name = "MontyDataclass")]
pub struct PyMontyDataclass {
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
            let field_obj = if cfg!(Py_3_14) {
                // Field(default, default_factory, init, repr, hash, compare, metadata, kw_only, doc)
                // doc is now in 3.14
                // https://github.com/python/cpython/blob/3.14/Lib/dataclasses.py#L294
                field_class.call1((
                    missing,   // default
                    missing,   // default_factory
                    true,      // init
                    true,      // repr
                    py.None(), // hash (None means use compare value)
                    true,      // compare
                    py.None(), // metadata
                    false,     // kw_only
                    py.None(), // doc
                ))?
            } else {
                // https://github.com/python/cpython/blob/3.13/Lib/dataclasses.py#L288
                // Field(default, default_factory, init, repr, hash, compare, metadata, kw_only)
                field_class.call1((
                    missing,   // default
                    missing,   // default_factory
                    true,      // init
                    true,      // repr
                    py.None(), // hash (None means use compare value)
                    true,      // compare
                    py.None(), // metadata
                    false,     // kw_only
                ))?
            };

            // Set name and type (these are set after construction in real dataclasses)
            field_obj.setattr("name", field_name)?;
            field_obj.setattr("type", field_type)?;
            field_obj.setattr("_field_type", field_marker)?;

            fields_dict.set_item(field_name, field_obj)?;
        }
        Ok(fields_dict.unbind())
    }

    /// Returns a `_DataclassParams` object with dataclass configuration.
    ///
    /// This enables compatibility with code that checks `obj.__dataclass_params__.frozen`, etc.
    #[getter]
    fn __dataclass_params__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let params_class = get_dataclass_params_class(py)?;
        let params = if cfg!(Py_3_12) {
            // https://github.com/python/cpython/blob/3.12/Lib/dataclasses.py#L373
            // _DataclassParams(init, repr, eq, order, unsafe_hash, frozen, match_args, kw_only, slots, weakref_slot)
            params_class.call1((
                true,        // init
                true,        // repr
                true,        // eq
                false,       // order
                false,       // unsafe_hash
                self.frozen, // frozen
                true,        // match_args
                false,       // kw_only
                false,       // slots
                false,       // weakref_slot
            ))?
        } else {
            // https://github.com/python/cpython/blob/3.11/Lib/dataclasses.py#L346
            // _DataclassParams(init, repr, eq, order, unsafe_hash, frozen)
            params_class.call1((
                true,        // init
                true,        // repr
                true,        // eq
                false,       // order
                false,       // unsafe_hash
                self.frozen, // frozen
            ))?
        };
        Ok(params.unbind())
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
    ///
    /// Raises `FrozenInstanceError` (subclass of `AttributeError`) for frozen dataclasses.
    fn __setattr__(&self, py: Python<'_>, name: &str, value: Py<PyAny>) -> PyResult<()> {
        if self.frozen {
            let frozen_error = get_frozen_instance_error(py)?;
            let msg = format!("cannot assign to field '{name}'");
            return Err(PyErr::from_value(frozen_error.call1((msg,))?));
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
        // Check if other is also a PyMontyDataclass
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
    /// Creates a new `PyMontyDataclass` from `MontyObject` fields.
    pub fn new<'a>(
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

/// Cached import of `dataclasses._DataclassParams` class.
fn get_dataclass_params_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_PARAMS_CLASS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_PARAMS_CLASS.import(py, "dataclasses", "_DataclassParams")
}

/// Cached import of `dataclasses.FrozenInstanceError` exception class.
pub fn get_frozen_instance_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FROZEN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FROZEN_ERROR.import(py, "dataclasses", "FrozenInstanceError")
}
