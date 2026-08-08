//! Class-instance conversion between Python and Monty.
//!
//! This module handles:
//! - Converting `pydantic_monty.ClassInstance` wrappers to `MontyObject::ClassInstance`
//! - Converting `MontyObject::ClassInstance` back to Python: the original
//!   wrapped object when the instance is in the session's [`InstanceStore`],
//!   else a read-only [`PyMontyClassInstance`] proxy
//! - [`InstanceStore`]: the per-session `instance_id -> wrapper` map that
//!   routes method calls and lazy attribute lookups back to the host object

use monty_types::{DictPairs, MontyObject};
use pyo3::{
    Bound,
    exceptions::{PyAttributeError, PyRuntimeError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyTuple, PyType},
};

use super::convert::{monty_to_py_inner, py_to_monty};

/// Checks if a Python object is a dataclass instance (not a type).
///
/// Copied from pydantic's `is_dataclass` logic.
pub fn is_dataclass(value: &Bound<'_, PyAny>) -> bool {
    value
        .hasattr(intern!(value.py(), "__dataclass_fields__"))
        .unwrap_or(false)
        && !value.is_instance_of::<PyType>()
}

/// Checks if a Python object is a `pydantic_monty.ClassInstance` wrapper.
pub fn is_class_instance_wrapper(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(get_class_instance_class(value.py())?)
}

/// Converts a `pydantic_monty.ClassInstance` wrapper to
/// `MontyObject::ClassInstance`, registering the wrapper in `store` keyed by
/// `id(instance)` so later method calls / lazy lookups / round-tripped returns
/// resolve to the original object.
///
/// Eager attrs come from `wrapper.get_eager_attrs()` and are converted with
/// `py_to_monty`, so nested wrappers inside them register themselves too.
pub fn class_instance_to_monty(wrapper: &Bound<'_, PyAny>, store: &InstanceStore, depth: u8) -> PyResult<MontyObject> {
    let py = wrapper.py();
    let instance = wrapper.getattr(intern!(py, "class_instance"))?;
    let instance_type = instance.get_type();

    let name: String = instance_type.getattr(intern!(py, "__name__"))?.extract()?;
    let instance_id = instance.as_ptr() as u64;
    let type_id = instance_type.as_ptr() as u64;
    let frozen: bool = wrapper.call_method0(intern!(py, "get_frozen"))?.extract()?;

    let eager = wrapper
        .call_method0(intern!(py, "get_eager_attrs"))?
        .cast_into::<PyDict>()?;
    let mut attrs = Vec::with_capacity(eager.len());
    for (key, value) in eager.iter() {
        attrs.push((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?));
    }

    store.insert(instance_id, wrapper)?;

    Ok(MontyObject::ClassInstance {
        name,
        instance_id,
        type_id,
        attrs: attrs.into(),
        frozen,
        is_dataclass: is_dataclass(&instance),
    })
}

/// Converts a `MontyObject::ClassInstance` to a Python object.
///
/// When `instance_id` is found in `store`, returns the ORIGINAL wrapped object
/// (identity preserved — `result is obj` holds). Otherwise — a sandbox-defined
/// instance (id 0) or an id from a session restored into a fresh process —
/// builds a read-only [`PyMontyClassInstance`] proxy.
///
/// `depth` is the caller's current recursion depth; it is forwarded to
/// `monty_to_py_inner` so nested attr values respect the output-depth limit.
pub fn class_instance_to_py(
    py: Python<'_>,
    name: &str,
    instance_id: u64,
    attrs: &DictPairs,
    is_dataclass: bool,
    store: &InstanceStore,
    depth: u8,
) -> PyResult<Py<PyAny>> {
    if let Some(wrapper) = store.get(py, instance_id)? {
        wrapper
            .bind(py)
            .getattr(intern!(py, "class_instance"))
            .map(Bound::unbind)
    } else {
        let attributes = PyDict::new(py);
        for (key, value) in attrs {
            // Skip non-string keys — hosts and the sandbox only produce
            // string attr names, so anything else is not representable.
            if let MontyObject::String(key) = key {
                attributes.set_item(key, monty_to_py_inner(py, value, store, depth)?)?;
            }
        }
        let proxy = PyMontyClassInstance {
            name: name.to_owned(),
            is_dataclass,
            attributes: attributes.unbind(),
        };
        Ok(Py::new(py, proxy)?.into_any())
    }
}

/// Per-session map from `instance_id` (the host's `id(obj)`) to the
/// `pydantic_monty.ClassInstance` wrapper that sent it.
///
/// Populated by [`class_instance_to_monty`] whenever a wrapper crosses into
/// the sandbox; consulted to answer method calls (`FunctionCall.instance_id`),
/// lazy attribute lookups (`NameLookup.instance_id`), and to hand the original
/// object back when the sandbox returns the instance. Holding the wrapper
/// keeps the instance alive, so `id()` stays unique for the session.
///
/// Wraps a `Py<PyDict>` so `clone_ref` produces a shared handle to the same
/// underlying dict; the GIL serializes access, so no lock is needed.
#[derive(Debug)]
pub struct InstanceStore {
    instances: Py<PyDict>,
}

impl InstanceStore {
    /// Creates a new empty store.
    #[must_use]
    pub fn new(py: Python<'_>) -> Self {
        Self {
            instances: PyDict::new(py).unbind(),
        }
    }

    /// Creates a shared handle to this store (cheap Python refcount bump).
    ///
    /// The clone points to the **same** underlying Python dict, so insertions
    /// through any handle are visible to all others.
    #[must_use]
    pub fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            instances: self.instances.clone_ref(py),
        }
    }

    /// Registers a wrapper under the instance's identity. Idempotent —
    /// re-sending the same instance overwrites (last wrapper wins).
    pub fn insert(&self, instance_id: u64, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        self.instances.bind(wrapper.py()).set_item(instance_id, wrapper)
    }

    /// Looks up the wrapper registered for `instance_id`.
    pub fn get(&self, py: Python<'_>, instance_id: u64) -> PyResult<Option<Py<PyAny>>> {
        Ok(self.instances.bind(py).get_item(instance_id)?.map(Bound::unbind))
    }

    /// Calls `wrapper.call_method(name, args, kwargs)` on the instance
    /// registered for `instance_id`.
    ///
    /// A store miss raises `RuntimeError`: it means the session was restored
    /// into a process that never sent the instance (e.g. `load_session`).
    pub fn call_method(
        &self,
        py: Python<'_>,
        instance_id: u64,
        name: &str,
        args: &Bound<'_, PyTuple>,
        kwargs: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        let Some(wrapper) = self.get(py, instance_id)? else {
            return Err(store_miss_error(name, instance_id));
        };
        wrapper
            .bind(py)
            .call_method1(intern!(py, "call_method"), (name, args, kwargs))
            .map(Bound::unbind)
    }

    /// Calls `wrapper.lookup_lazy_attrs(name)` on the instance registered for
    /// `instance_id`; `Ok(None)` means "not exposed" (store miss or the
    /// wrapper raised `AttributeError`) and the sandbox raises AttributeError.
    pub fn lookup_lazy_attr(&self, py: Python<'_>, instance_id: u64, name: &str) -> PyResult<Option<Py<PyAny>>> {
        let Some(wrapper) = self.get(py, instance_id)? else {
            return Ok(None);
        };
        match wrapper.bind(py).call_method1(intern!(py, "lookup_lazy_attrs"), (name,)) {
            Ok(value) => Ok(Some(value.unbind())),
            Err(err) if err.is_instance_of::<PyAttributeError>(py) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// The error a method call on an unregistered instance id raises.
fn store_miss_error(name: &str, instance_id: u64) -> PyErr {
    PyRuntimeError::new_err(format!(
        "no host instance registered for method call '{name}' (id {instance_id}) — \
         the instance store is empty after loading a session into a new process"
    ))
}

/// Read-only proxy for a class instance the host has no original object for:
/// a sandbox-defined instance, or a host instance returned after the session
/// was restored into a fresh process.
///
/// A plain data holder — attribute values were converted when the value
/// crossed the wire, and there is no live sandbox object behind it.
#[pyclass(name = "MontyClassInstance", module = "pydantic_monty", frozen)]
pub struct PyMontyClassInstance {
    /// Class name (e.g., "Point", "User").
    name: String,
    /// Whether the origin side reported `dataclasses.is_dataclass(obj)`.
    is_dataclass: bool,
    /// The instance's attributes, converted to Python values.
    attributes: Py<PyDict>,
}

#[pymethods]
impl PyMontyClassInstance {
    /// Class name of the instance (e.g. `"Point"`).
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    /// Whether the instance was a dataclass on the side that produced it.
    #[getter]
    fn is_dataclass(&self) -> bool {
        self.is_dataclass
    }

    /// The instance's attributes as a plain dict.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> Py<PyDict> {
        self.attributes.clone_ref(py)
    }

    /// `MontyClassInstance(name='Point', attributes={'x': 1})`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs_repr: String = self.attributes.bind(py).repr()?.extract()?;
        Ok(format!(
            "MontyClassInstance(name='{}', attributes={attrs_repr})",
            self.name
        ))
    }

    /// Equal when name, dataclass-ness, and attributes all match.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other) = other.extract::<PyRef<'_, Self>>() {
            Ok(self.name == other.name
                && self.is_dataclass == other.is_dataclass
                && self.attributes.bind(py).eq(other.attributes.bind(py))?)
        } else {
            Ok(false)
        }
    }
}

/// Cached import of the `pydantic_monty.ClassInstance` wrapper class.
fn get_class_instance_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLASS_INSTANCE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    CLASS_INSTANCE.import(py, "pydantic_monty.class_instance", "ClassInstance")
}

/// Cached import of `dataclasses.FrozenInstanceError` exception class.
pub fn get_frozen_instance_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FROZEN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FROZEN_ERROR.import(py, "dataclasses", "FrozenInstanceError")
}
