//! Dataclass conversion between Python and Monty.
//!
//! This module handles:
//! - Converting Python dataclass instances to `MontyObject::Dataclass`
//! - Converting `MontyObject::Dataclass` back to Python via `PyUnknownDataclass`
//! - `PyUnknownDataclass`: A Python class that mimics dataclass behavior

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ::monty::{DictPairs, MontyObject};
use pyo3::{
    Bound,
    exceptions::{PyAttributeError, PyTypeError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyList, PyString, PyType},
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
/// The `type_id` is set to `id(type(dc))` in Python, allowing registry lookups by type identity.
/// The `dc_registry` is threaded through to `py_to_monty` so that nested dataclasses
/// in field values are also auto-registered.
pub fn dataclass_to_monty(value: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> PyResult<MontyObject> {
    let py = value.py();

    let dc_type = value.get_type();
    let name: String = dc_type.getattr(intern!(py, "__name__"))?.extract()?;

    // Get type_id from id(type(dc)) for registry lookups
    let type_id = dc_type.as_ptr() as u64;

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
            let field_name_monty = py_to_monty(&field_name_obj, dc_registry)?;
            let field_value_monty = py_to_monty(&field_value, dc_registry)?;

            field_names.push(field_name_str);
            attrs.push((field_name_monty, field_value_monty));
        }
    }

    Ok(MontyObject::Dataclass {
        name,
        type_id,
        field_names,
        attrs: attrs.into(),
        frozen,
    })
}

/// Converts a `MontyObject::Dataclass` to a Python object.
///
/// If the `type_id` is found in the dc_registry, creates an instance of the original
/// Python dataclass type (so `isinstance(result, OriginalClass)` works).
/// Otherwise, falls back to creating a `PyUnknownDataclass`.
pub fn dataclass_to_py(
    py: Python<'_>,
    name: &str,
    type_id: u64,
    field_names: &[String],
    attrs: &DictPairs,
    frozen: bool,
    dc_registry: &DcRegistry,
) -> PyResult<Py<PyAny>> {
    // Try to use the original type from the dc_registry (keyed by type_id)
    if let Some(original_type_py) = dc_registry.get(py, type_id)? {
        let original_type = original_type_py.bind(py).cast::<PyType>()?;
        // Build kwargs dict from field names and values
        let kwargs = PyDict::new(py);
        for (key, value) in attrs {
            // Skip non-string keys
            if let MontyObject::String(s) = key {
                // Only include declared fields in constructor kwargs
                let key_str = s.as_str();
                if field_names.iter().any(|f| f.as_str() == key_str) {
                    kwargs.set_item(key_str, monty_to_py(py, value, dc_registry)?)?;
                }
            }
        }

        // Call the dataclass constructor with kwargs
        original_type.call((), Some(&kwargs)).map(Bound::unbind)
    } else {
        // Fall back to PyUnknownDataclass
        let dc = PyUnknownDataclass::new(py, name.to_string(), field_names.to_vec(), attrs, frozen, dc_registry)?;
        Ok(Py::new(py, dc)?.into_any())
    }
}

/// Maps Python dataclass type identity (pointer address as `u64`) to the original
/// Python type object (`Py<PyAny>`).
///
/// This registry enables round-trip reconstruction of dataclass types: when a
/// dataclass passes through Monty, its type is stored here so that on output,
/// `isinstance(result, OriginalClass)` works correctly.
///
/// Wraps a `Py<PyDict>` so that `clone_ref` produces a shared handle to the same
/// underlying dict — all clones see the same data without needing `Arc<Mutex>`.
/// The GIL already serializes access, making additional locking unnecessary.
#[derive(Debug)]
pub struct DcRegistry {
    registry: Py<PyDict>,
}

impl DcRegistry {
    /// Creates a new empty registry.
    pub fn new(py: Python<'_>) -> Self {
        Self {
            registry: PyDict::new(py).unbind(),
        }
    }

    /// Creates a `DcRegistry` from an optional Python list of dataclass types.
    ///
    /// Each type in the list is registered by its pointer identity, matching the key
    /// format used by `dataclass_to_monty`.
    pub fn from_list(py: Python<'_>, dataclass_registry: Option<&Bound<'_, PyList>>) -> PyResult<Self> {
        let slf = Self::new(py);

        if let Some(registry_list) = dataclass_registry {
            for cls in registry_list {
                slf.insert(&cls)?;
            }
        }
        Ok(slf)
    }

    /// Creates a shared handle to this registry (cheap Python refcount bump).
    ///
    /// The clone points to the **same** underlying Python dict, so insertions
    /// through any handle are visible to all others.
    pub fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            registry: self.registry.clone_ref(py),
        }
    }

    /// Registers a Python type in the dataclass registry, keyed by pointer identity.
    ///
    /// This is idempotent — calling it multiple times with the same type is safe and
    /// simply overwrites the existing entry. The key is the raw pointer address of the
    /// type object, matching what `dataclass_to_monty` stores as `type_id` in
    /// `MontyObject::Dataclass`. This allows `dataclass_to_py` to look up the original
    /// Python class when reconstructing output values.
    pub fn insert<T>(&self, obj: &Bound<'_, T>) -> PyResult<()> {
        let py = obj.py();
        let type_id = obj.as_ptr() as u64;
        self.registry.bind(py).set_item(type_id, obj.as_any())
    }

    /// Looks up an original Python type by its pointer identity.
    pub fn get(&self, py: Python<'_>, type_id: u64) -> PyResult<Option<Py<PyAny>>> {
        Ok(self.registry.bind(py).get_item(type_id)?.map(Bound::unbind))
    }
}

/// Python class that mimics dataclass behavior for `MontyObject::Dataclass`.
///
/// Supports:
/// - Attribute access (`__getattr__`, `__setattr__`)
/// - String representation (`__repr__`, `__str__`)
/// - Equality comparison (`__eq__`)
/// - Hashing for frozen instances (`__hash__`)
/// - `dataclasses` module compatibility (`__dataclass_fields__`)
#[pyclass(name = "UnknownDataclass")]
pub struct PyUnknownDataclass {
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
impl PyUnknownDataclass {
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
            None => Err(PyAttributeError::new_err(format!(
                "'UnknownDataclass' object has no attribute '{name}'",
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
        Ok(format!("<Unknown Dataclass {}({})>", self.name, parts.join(", ")))
    }

    /// Equality comparison.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        // Check if other is also a PyUnknownDataclass
        if let Ok(other_dc) = other.extract::<PyRef<'_, Self>>() {
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
            return Err(PyTypeError::new_err("unhashable type: 'UnknownDataclass'"));
        }

        let mut hasher = DefaultHasher::new();

        let attrs = self.attrs.bind(py);
        for field_name in &self.field_names {
            field_name.hash(&mut hasher);
            if let Some(value) = attrs.get_item(field_name)? {
                let value_hash: isize = value.hash()?;
                value_hash.hash(&mut hasher);
            }
        }
        // Python's hash returns a signed integer; reinterpret bits for large values
        let hash_u64 = hasher.finish();
        #[cfg(target_pointer_width = "64")]
        let hash_isize = isize::from_ne_bytes(hash_u64.to_ne_bytes());
        #[cfg(not(target_pointer_width = "64"))]
        let hash_isize = {
            // On 32-bit: truncate to lower 32 bits, then reinterpret as i32 -> isize
            let hash_u32 = hash_u64 as u32;
            i32::from_ne_bytes(hash_u32.to_ne_bytes()) as isize
        };
        Ok(hash_isize)
    }
}

impl PyUnknownDataclass {
    /// Creates a new `PyUnknownDataclass` from `MontyObject` fields.
    pub fn new<'a>(
        py: Python<'_>,
        name: String,
        field_names: Vec<String>,
        attrs: impl IntoIterator<Item = &'a (MontyObject, MontyObject)>,
        frozen: bool,
        dc_registry: &DcRegistry,
    ) -> PyResult<Self> {
        let dict = PyDict::new(py);
        for (k, v) in attrs {
            dict.set_item(monty_to_py(py, k, dc_registry)?, monty_to_py(py, v, dc_registry)?)?;
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
