//! Class-instance conversion between Python and Monty.
//!
//! This module handles:
//! - Converting `pydantic_monty.ClassInstance` / `pydantic_monty.ClassType`
//!   wrappers to `MontyObject::ClassInstance` / `MontyObject::Type`
//! - Converting `MontyObject::ClassInstance` back to Python: the original
//!   wrapped object when the instance is in the session's [`InstanceStore`],
//!   else a read-only [`PyMontyClassProxy`] proxy
//! - [`InstanceStore`]: the per-session uuid → wrapper/class maps that route
//!   method calls and lazy attribute lookups (on instances and class types
//!   alike — construction arrives as a `__call__` method call) back to the
//!   host objects
//!
//! Identity comes from each wrapper's `id` field: instances default to a
//! fresh uuid4 per wrapper, classes to `pydantic_monty`'s name-keyed
//! `type_id_cache` (an instance's class and parent classes get `ClassType`
//! wrappers built on demand). Never `id()`, which leaks heap addresses to
//! the worker and is reused by CPython.

use monty_types::{DictPairs, MontyClassInstance, MontyClassType, MontyObject, MontyUuid};
use pyo3::{
    Bound,
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyTuple},
};

use super::convert::{monty_to_py_inner, py_to_monty};

/// Checks if a Python object is a `pydantic_monty.ClassInstance` wrapper.
pub fn is_class_instance_wrapper(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(get_class_instance_class(value.py())?)
}

/// Checks if a Python object is a `pydantic_monty.ClassType` wrapper.
pub fn is_class_type_wrapper(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(get_class_type_class(value.py())?)
}

/// Per-session map from wrapper uuids to the `ClassInstance` / `ClassType`
/// wrappers that crossed into the sandbox. Answers method calls and lazy
/// attribute lookups by `object_id` (construction arrives as `__call__` on
/// the class uuid) and hands original objects back when the sandbox returns
/// them; entries pin their wrapper for the life of the session.
#[derive(Debug)]
pub struct InstanceStore {
    /// uuid bytes → wrapper, one namespace for instances and classes. Shared
    /// by `clone_ref` handles (the GIL serializes access); alias checks
    /// compare wrapped objects by identity, never a metaclass `__eq__`.
    objects: Py<PyDict>,
}

impl InstanceStore {
    /// Creates a new empty store.
    #[must_use]
    pub fn new(py: Python<'_>) -> Self {
        Self {
            objects: PyDict::new(py).unbind(),
        }
    }

    /// Creates a shared handle to this store (cheap Python refcount bump).
    ///
    /// The clone points to the **same** underlying Python dict, so
    /// insertions through any handle are visible to all others.
    #[must_use]
    pub fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            objects: self.objects.clone_ref(py),
        }
    }

    /// Converts a `pydantic_monty.ClassInstance` wrapper to a [`MontyClassInstance`],
    /// registering the wrapper under the instance's uuid so later
    /// method calls / lazy lookups / round-tripped returns resolve to the
    /// original object.
    ///
    /// Eager attrs come from `wrapper.get_eager_attrs()` and are converted with
    /// `py_to_monty`, so nested wrappers inside them register themselves too.
    pub(crate) fn class_instance_to_monty(
        &self,
        wrapper: &Bound<'_, PyAny>,
        depth: u8,
    ) -> PyResult<MontyClassInstance> {
        let py = wrapper.py();

        // `ClassInstance.__post_init__` always materializes a `ClassType` wrapper
        // for the value's class; its `id` (from the name-keyed cache) is the
        // class identity every crossing of this class shares.
        let ct_wrapper = wrapper.getattr(intern!(py, "class_type"))?;
        let class_type = self.class_type_from_wrapper(&ct_wrapper, depth)?;
        // Register the `ClassType` wrapper for routing (lazy class attrs,
        // classmethod calls) unless the class already has one: an auto-materialized
        // default must not clobber an explicitly granted policy.
        self.register_class_type_if_absent(&class_type.id, &ct_wrapper)?;

        let instance_id = wrapper_uuid(wrapper, "ClassInstance")?;

        let eager = wrapper
            .call_method0(intern!(py, "get_eager_attrs"))?
            .cast_into::<PyDict>()?;
        let mut attrs = Vec::with_capacity(eager.len());
        for (key, value) in eager.iter() {
            attrs.push((py_to_monty(&key, self, depth)?, py_to_monty(&value, self, depth)?));
        }

        self.register(&instance_id, wrapper)?;

        Ok(MontyClassInstance {
            class_type,
            instance_id,
            attrs: attrs.into(),
        })
    }

    /// Converts a `pydantic_monty.ClassType` wrapper to the [`MontyClassType`] it
    /// crosses as (the caller wraps it in `MontyObject::Type`), registering the
    /// wrapper under the class uuid so sandbox method calls
    /// (`__call__` construction included) and lazy class attr lookups route back
    /// to it.
    pub(crate) fn class_type_to_monty(&self, wrapper: &Bound<'_, PyAny>, depth: u8) -> PyResult<MontyClassType> {
        let class_type = self.class_type_from_wrapper(wrapper, depth)?;
        self.register(&class_type.id, wrapper)?;
        Ok(class_type)
    }

    /// Builds the wire [`MontyClassType`] from a `pydantic_monty.ClassType`
    /// wrapper: name from the class, `id` from the wrapper (the name-keyed
    /// cache makes it stable per class), and the wrapper's eager class attrs
    /// (`get_eager_attrs`) converted with `py_to_monty`. Sent both when the
    /// class crosses as a value and as the type branch of every instance
    /// crossing, so the sandbox's one type object per class sees the attrs
    /// whichever arrives first. Registration in the store is the caller's job.
    fn class_type_from_wrapper(&self, wrapper: &Bound<'_, PyAny>, depth: u8) -> PyResult<MontyClassType> {
        let py = wrapper.py();
        let class = wrapper.getattr(intern!(py, "value"))?;
        let eager = wrapper
            .call_method0(intern!(py, "get_eager_attrs"))?
            .cast_into::<PyDict>()?;
        let mut attrs = Vec::with_capacity(eager.len());
        for (key, value) in eager.iter() {
            attrs.push((py_to_monty(&key, self, depth)?, py_to_monty(&value, self, depth)?));
        }
        Ok(MontyClassType {
            name: class.getattr(intern!(py, "__name__"))?.extract()?,
            id: wrapper_uuid(wrapper, "ClassType")?,
            host_defined: true,
            is_dataclass: wrapper.call_method0(intern!(py, "is_dataclass"))?.extract()?,
            attrs: attrs.into(),
        })
    }

    /// Converts a [`MontyClassInstance`] to a Python object.
    ///
    /// When its `instance_id` is registered, returns the ORIGINAL wrapped object
    /// (identity preserved — `result is obj` holds). Otherwise — a sandbox-defined
    /// instance, or an id from a session restored into a fresh session — builds a
    /// read-only [`PyMontyClassProxy`] that keeps the ids, so passing it back
    /// hands the sandbox its original object.
    ///
    /// `depth` is the caller's current recursion depth; it is forwarded to
    /// `monty_to_py_inner` so nested attr values respect the output-depth limit.
    pub(crate) fn class_instance_to_py(
        &self,
        py: Python<'_>,
        instance: &MontyClassInstance,
        depth: u8,
    ) -> PyResult<Py<PyAny>> {
        if let Some(wrapper) = self.get(py, &instance.instance_id)? {
            wrapper.bind(py).getattr(intern!(py, "value")).map(Bound::unbind)
        } else {
            let proxy = PyMontyClassProxy {
                class_type: instance.class_type.clone(),
                instance_id: instance.instance_id,
                attributes: attrs_to_py_dict(py, &instance.attrs, self, depth)?,
            };
            Ok(Py::new(py, proxy)?.into_any())
        }
    }

    /// Resolves a class type object crossing back out of the sandbox: the
    /// original class when its uuid is registered, else a read-only
    /// [`PyMontyClassTypeProxy`] (a sandbox class, or a host class from a
    /// session restored into a fresh session) that keeps the ids so passing
    /// it back re-enters as the same type. `depth` is forwarded to
    /// `monty_to_py_inner` for the eager class attrs.
    pub(crate) fn class_type_to_py(
        &self,
        py: Python<'_>,
        class_type: &MontyClassType,
        depth: u8,
    ) -> PyResult<Py<PyAny>> {
        if let Some(class) = self.get_class(py, &class_type.id)? {
            Ok(class)
        } else {
            let proxy = PyMontyClassTypeProxy {
                class_type: class_type.clone(),
                attributes: attrs_to_py_dict(py, &class_type.attrs, self, depth)?,
            };
            Ok(Py::new(py, proxy)?.into_any())
        }
    }

    /// Calls `wrapper.call_method(name, args, kwargs)` on the object
    /// registered for `uuid` — an instance wrapper, or a `ClassType` wrapper
    /// (whose `call_method` routes `__call__` to `construct`, re-checking its
    /// own host-side `init` policy; nothing about instantiability crosses
    /// the wire).
    ///
    /// A store miss raises `RuntimeError`: the session was restored into a
    /// process that never sent the object.
    pub fn call_method(
        &self,
        py: Python<'_>,
        uuid: &MontyUuid,
        name: &str,
        args: &Bound<'_, PyTuple>,
        kwargs: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        let Some(wrapper) = self.get(py, uuid)? else {
            return Err(store_miss_error(name, uuid));
        };
        wrapper
            .bind(py)
            .call_method1(intern!(py, "call_method"), (name, args, kwargs))
            .map(Bound::unbind)
    }

    /// Calls `wrapper.lookup_lazy_attrs(name)` on the object registered for
    /// `uuid`; `Ok(None)` means "not exposed" (store miss or the wrapper
    /// raised `AttributeError`) and the sandbox raises AttributeError.
    pub fn lookup_lazy_attr(&self, py: Python<'_>, uuid: &MontyUuid, name: &str) -> PyResult<Option<Py<PyAny>>> {
        let Some(wrapper) = self.get(py, uuid)? else {
            return Ok(None);
        };
        match wrapper.bind(py).call_method1(intern!(py, "lookup_lazy_attrs"), (name,)) {
            Ok(value) => Ok(Some(value.unbind())),
            Err(err) if err.is_instance_of::<PyAttributeError>(py) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Registers a `ClassInstance` / `ClassType` wrapper under its uuid, for
    /// routing and identity round-trips. Re-sending the same object with a
    /// new wrapper overwrites the entry (last policy wins); an id already
    /// routing to a different object is rejected.
    fn register(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = wrapper.py();
        self.check_no_alias(uuid, wrapper)?;
        self.objects
            .bind(py)
            .set_item(PyBytes::new(py, uuid.as_bytes()), wrapper)
    }

    /// [`Self::register`] with `setdefault` semantics: the wrapper
    /// is registered only if its class uuid has no entry yet. Used for the
    /// `ClassType` a `ClassInstance` materializes, so an auto-built default
    /// never clobbers an explicitly granted policy. An id aliasing a
    /// different object is still rejected.
    fn register_class_type_if_absent(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = wrapper.py();
        self.check_no_alias(uuid, wrapper)?;
        self.objects
            .bind(py)
            .set_default(PyBytes::new(py, uuid.as_bytes()), wrapper)?;
        Ok(())
    }

    /// Errors if `uuid` is already registered for an object other than the
    /// one `wrapper` wraps (compared by identity). Two wrappers sharing an id
    /// but wrapping different objects would silently re-route method calls
    /// and round-trips from one host object to the other. `wrapper.value` is
    /// only read when there is an existing entry to compare against.
    fn check_no_alias(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = wrapper.py();
        let value_str = intern!(py, "value");
        if let Some(existing) = self.objects.bind(py).get_item(PyBytes::new(py, uuid.as_bytes()))?
            && !existing.getattr(value_str)?.is(&wrapper.getattr(value_str)?)
        {
            return Err(PyValueError::new_err(format!(
                "wrapper id {uuid} already identifies a different object in this session"
            )));
        }
        Ok(())
    }

    /// Looks up the wrapper registered for `uuid` (instance or class type).
    fn get(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .objects
            .bind(py)
            .get_item(PyBytes::new(py, uuid.as_bytes()))?
            .map(Bound::unbind))
    }

    /// Looks up the class registered for `uuid`: the `value` of its
    /// `ClassType` wrapper.
    fn get_class(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        let Some(wrapper) = self.get(py, uuid)? else {
            return Ok(None);
        };
        Ok(Some(wrapper.bind(py).getattr(intern!(py, "value"))?.unbind()))
    }
}

/// Converts wire attrs to a Python dict, skipping non-string keys — hosts
/// and the sandbox only produce string attr names, so anything else is not
/// representable.
fn attrs_to_py_dict(py: Python<'_>, attrs: &DictPairs, store: &InstanceStore, depth: u8) -> PyResult<Py<PyDict>> {
    let attributes = PyDict::new(py);
    for (key, value) in attrs {
        if let MontyObject::String(key) = key {
            attributes.set_item(key, monty_to_py_inner(py, value, store, depth)?)?;
        }
    }
    Ok(attributes.unbind())
}

/// Builds a Python `uuid.UUID` from a [`MontyUuid`], for the public snapshot
/// surfaces.
pub fn uuid_to_py(py: Python<'_>, uuid: &MontyUuid) -> PyResult<Py<PyAny>> {
    static UUID_CLASS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    let class = UUID_CLASS.import(py, "uuid", "UUID")?;
    let kwargs = PyDict::new(py);
    kwargs.set_item(intern!(py, "bytes"), PyBytes::new(py, uuid.as_bytes()))?;
    class.call((), Some(&kwargs)).map(Bound::unbind)
}

/// Read-only proxy for a class instance the host has no original object for:
/// a sandbox-defined instance, or a host instance returned after the session
/// was restored into a fresh session.
///
/// A plain data holder — attribute values were converted when the value
/// crossed the wire. It keeps the wire ids, so passing it back into the
/// sandbox hands over the original object (a live sandbox instance resolves
/// by identity; the attributes are not applied).
#[pyclass(name = "MontyClassProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyClassProxy {
    /// The instance's class as it crossed the wire (name, id, dataclass-ness).
    class_type: MontyClassType,
    /// Identity of the instance, generated by the side that defined it.
    instance_id: MontyUuid,
    /// The instance's attributes, converted to Python values.
    attributes: Py<PyDict>,
}

impl PyMontyClassProxy {
    /// Rebuilds the wire value the proxy was built from, so it can cross back
    /// into the sandbox; `attributes` are converted with `py_to_monty`.
    pub(crate) fn to_monty(&self, py: Python<'_>, store: &InstanceStore, depth: u8) -> PyResult<MontyClassInstance> {
        let attrs = self
            .attributes
            .bind(py)
            .iter()
            .map(|(key, value)| Ok((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?)))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(MontyClassInstance {
            class_type: self.class_type.clone(),
            instance_id: self.instance_id,
            attrs: attrs.into(),
        })
    }
}

#[pymethods]
impl PyMontyClassProxy {
    /// Class name of the instance (e.g. `"Point"`).
    #[getter]
    fn name(&self) -> &str {
        &self.class_type.name
    }

    /// Whether the instance was a dataclass on the side that produced it.
    #[getter]
    fn is_dataclass(&self) -> bool {
        self.class_type.is_dataclass
    }

    /// Identity of the instance as a `uuid.UUID`, the id the sandbox resolves
    /// the original object by when the proxy is passed back.
    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        uuid_to_py(py, &self.instance_id)
    }

    /// The instance's attributes as a plain dict.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> Py<PyDict> {
        self.attributes.clone_ref(py)
    }

    /// `MontyClassProxy(name='Point', attributes={'x': 1})`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs_repr: String = self.attributes.bind(py).repr()?.extract()?;
        Ok(format!(
            "MontyClassProxy(name='{}', attributes={attrs_repr})",
            self.class_type.name
        ))
    }

    /// Equal when name, dataclass-ness, and attributes all match.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other) = other.extract::<PyRef<'_, Self>>() {
            Ok(self.class_type.name == other.class_type.name
                && self.class_type.is_dataclass == other.class_type.is_dataclass
                && self.attributes.bind(py).eq(other.attributes.bind(py))?)
        } else {
            Ok(false)
        }
    }
}

/// Read-only proxy for a host class the store has no class object for: one
/// returned (as a value, or as `type(x)`) after the session was restored into
/// a fresh session. Keeps the wire id, so passing it back into the sandbox
/// re-enters as the same type object.
#[pyclass(name = "MontyClassTypeProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyClassTypeProxy {
    /// The class as it crossed the wire (name, id, dataclass-ness).
    class_type: MontyClassType,
    /// The eager class attrs that crossed with it, converted to Python values.
    attributes: Py<PyDict>,
}

impl PyMontyClassTypeProxy {
    /// Rebuilds the wire type the proxy was built from, so it can cross back
    /// into the sandbox; `attributes` are converted with `py_to_monty`.
    pub(crate) fn to_monty(&self, py: Python<'_>, store: &InstanceStore, depth: u8) -> PyResult<MontyClassType> {
        let attrs = self
            .attributes
            .bind(py)
            .iter()
            .map(|(key, value)| Ok((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?)))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(MontyClassType {
            attrs: attrs.into(),
            ..self.class_type.clone()
        })
    }
}

#[pymethods]
impl PyMontyClassTypeProxy {
    /// Name of the class (e.g. `"Point"`).
    #[getter]
    fn name(&self) -> &str {
        &self.class_type.name
    }

    /// Whether the class is a dataclass on the side that produced it.
    #[getter]
    fn is_dataclass(&self) -> bool {
        self.class_type.is_dataclass
    }

    /// Identity of the class as a `uuid.UUID`, the id the sandbox resolves
    /// the type by when the proxy is passed back.
    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        uuid_to_py(py, &self.class_type.id)
    }

    /// The eager class attrs as a plain dict.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> Py<PyDict> {
        self.attributes.clone_ref(py)
    }

    /// `MontyClassTypeProxy(name='Point', attributes={})`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs_repr: String = self.attributes.bind(py).repr()?.extract()?;
        Ok(format!(
            "MontyClassTypeProxy(name='{}', attributes={attrs_repr})",
            self.class_type.name
        ))
    }

    /// Equal when the class ids match: one type object per id in the sandbox.
    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other
            .extract::<PyRef<'_, Self>>()
            .is_ok_and(|other| self.class_type.id == other.class_type.id)
    }
}

/// The error a routed call on an unregistered uuid raises: the store is
/// empty because the session was restored into a fresh session.
fn store_miss_error(name: &str, uuid: &MontyUuid) -> PyErr {
    PyRuntimeError::new_err(format!(
        "no host object registered for method call '{name}' (id {uuid}) — \
         the instance store is empty after loading a dump into a fresh session"
    ))
}

/// Reads a wrapper's `id` field (a `uuid.UUID`, uuid4 by default) as a
/// [`MontyUuid`] — the wrapper owns its identity, so re-sending the same
/// wrapper preserves it and the host never derives ids from addresses.
/// `kind` names the wrapper class (`ClassInstance` / `ClassType`) in the error.
fn wrapper_uuid(wrapper: &Bound<'_, PyAny>, kind: &str) -> PyResult<MontyUuid> {
    let py = wrapper.py();
    let bytes: [u8; 16] = wrapper
        .getattr(intern!(py, "id"))?
        .getattr(intern!(py, "bytes"))
        .and_then(|bytes| bytes.extract())
        .map_err(|_| PyTypeError::new_err(format!("{kind}.id must be a uuid.UUID")))?;
    Ok(MontyUuid::from_bytes(bytes))
}

/// Cached import of the `pydantic_monty.ClassInstance` wrapper class.
fn get_class_instance_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLASS_INSTANCE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    CLASS_INSTANCE.import(py, "pydantic_monty.class_instance", "ClassInstance")
}

/// Cached import of the `pydantic_monty.ClassType` wrapper class.
fn get_class_type_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLASS_TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    CLASS_TYPE.import(py, "pydantic_monty.class_instance", "ClassType")
}
