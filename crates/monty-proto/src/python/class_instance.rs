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
    types::{PyBytes, PyDict, PyTuple, PyType},
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

/// Converts a `pydantic_monty.ClassInstance` wrapper to a [`MontyClassInstance`],
/// registering the wrapper in `store` under the instance's uuid so later
/// method calls / lazy lookups / round-tripped returns resolve to the
/// original object.
///
/// Eager attrs come from `wrapper.get_eager_attrs()` and are converted with
/// `py_to_monty`, so nested wrappers inside them register themselves too.
pub fn class_instance_to_monty(
    wrapper: &Bound<'_, PyAny>,
    store: &InstanceStore,
    depth: u8,
) -> PyResult<MontyClassInstance> {
    let py = wrapper.py();

    // `ClassInstance.__post_init__` always materializes a `ClassType` wrapper
    // for the value's class; its `id` (from the name-keyed cache) is the
    // class identity every crossing of this class shares.
    let ct_wrapper = wrapper.getattr(intern!(py, "class_type"))?;
    let class_type = class_type_from_wrapper(&ct_wrapper)?;
    // Register the class wrapper for routing (lazy class attrs, classmethod
    // calls) only when the class has no wrapper yet: an auto-materialized
    // default must not clobber an explicitly granted `ClassType` policy.
    let class = ct_wrapper.getattr(intern!(py, "value"))?;
    store.register_class_wrapper_if_absent(&class_type.id, &class, &ct_wrapper)?;

    let instance_id = wrapper_uuid(wrapper)?;

    let eager = wrapper
        .call_method0(intern!(py, "get_eager_attrs"))?
        .cast_into::<PyDict>()?;
    let mut attrs = Vec::with_capacity(eager.len());
    for (key, value) in eager.iter() {
        attrs.push((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?));
    }

    store.register_instance(&instance_id, wrapper)?;

    Ok(MontyClassInstance {
        class_type,
        instance_id,
        attrs: attrs.into(),
    })
}

/// Converts a `pydantic_monty.ClassType` wrapper to the [`MontyClassType`] it
/// crosses as (the caller wraps it in `MontyObject::Type`), registering the
/// class and wrapper in `store` under the class uuid so sandbox method calls
/// (`__call__` construction included) and lazy class attr lookups route back
/// to it.
///
/// Unlike [`class_type_from_wrapper`], this is the class crossing as a value:
/// eager class attrs come from the wrapper's class-object policy
/// (`get_eager_attrs`) and cross inside the wire `Type`.
pub fn class_type_to_monty(wrapper: &Bound<'_, PyAny>, store: &InstanceStore, depth: u8) -> PyResult<MontyClassType> {
    let py = wrapper.py();
    let class = wrapper.getattr(intern!(py, "value"))?;
    if !class.is_instance_of::<PyType>() {
        return Err(PyTypeError::new_err("ClassType.value must be a class"));
    }

    let mut class_type = class_type_from_wrapper(wrapper)?;

    let eager = wrapper
        .call_method0(intern!(py, "get_eager_attrs"))?
        .cast_into::<PyDict>()?;
    let mut attrs = Vec::with_capacity(eager.len());
    for (key, value) in eager.iter() {
        attrs.push((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?));
    }
    class_type.attrs = attrs.into();

    store.register_class_wrapper(&class_type.id, &class, wrapper)?;
    Ok(class_type)
}

/// Builds the wire [`MontyClassType`] from a `pydantic_monty.ClassType` wrapper:
/// name from the class, `id` from the wrapper (the name-keyed cache makes it
/// stable per class). Registration in the store is the caller's job.
fn class_type_from_wrapper(wrapper: &Bound<'_, PyAny>) -> PyResult<MontyClassType> {
    let py = wrapper.py();
    let class = wrapper.getattr(intern!(py, "value"))?;
    Ok(MontyClassType {
        name: class.getattr(intern!(py, "__name__"))?.extract()?,
        id: wrapper_uuid(wrapper)?,
        host_defined: true,
        is_dataclass: wrapper.call_method0(intern!(py, "is_dataclass"))?.extract()?,
        // Eager class attrs are set only when a `ClassType` wrapper crosses
        // as a value; the type branch of an instance stays attr-less.
        attrs: DictPairs::default(),
    })
}

/// Converts a [`MontyClassInstance`] to a Python object.
///
/// When its `instance_id` is found in `store`, returns the ORIGINAL wrapped object
/// (identity preserved — `result is obj` holds). Otherwise — a sandbox-defined
/// instance, or an id from a session restored into a fresh session — builds a
/// read-only [`PyMontyClassProxy`] proxy.
///
/// `depth` is the caller's current recursion depth; it is forwarded to
/// `monty_to_py_inner` so nested attr values respect the output-depth limit.
pub fn class_instance_to_py(
    py: Python<'_>,
    instance: &MontyClassInstance,
    store: &InstanceStore,
    depth: u8,
) -> PyResult<Py<PyAny>> {
    if let Some(wrapper) = store.get(py, &instance.instance_id)? {
        wrapper.bind(py).getattr(intern!(py, "value")).map(Bound::unbind)
    } else {
        let attributes = PyDict::new(py);
        for (key, value) in &instance.attrs {
            // Skip non-string keys — hosts and the sandbox only produce
            // string attr names, so anything else is not representable.
            if let MontyObject::String(key) = key {
                attributes.set_item(key, monty_to_py_inner(py, value, store, depth)?)?;
            }
        }
        let proxy = PyMontyClassProxy {
            name: instance.class_type.name.clone(),
            is_dataclass: instance.class_type.is_dataclass,
            attributes: attributes.unbind(),
        };
        Ok(Py::new(py, proxy)?.into_any())
    }
}

/// Resolves a class type object crossing back out of the sandbox: the
/// original class when its uuid is registered, else `None` (a sandbox class,
/// or a host class from a session restored into a fresh session).
pub fn class_type_to_py(
    py: Python<'_>,
    class_type: &MontyClassType,
    store: &InstanceStore,
) -> PyResult<Option<Py<PyAny>>> {
    if class_type.host_defined {
        store.get_class(py, &class_type.id)
    } else {
        Ok(None)
    }
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

/// Per-session identity map from host-generated uuids to the wrappers behind
/// them.
///
/// Populated whenever a `ClassInstance`/`ClassType` wrapper crosses into the
/// sandbox; consulted to answer method calls and lazy attribute lookups
/// (`FunctionCall.object_id` / `NameLookup.object_id` — construction of a
/// host class arrives as a `__call__` method call on the class uuid), and to
/// hand original objects back when the sandbox returns them. Registered
/// entries pin their wrapper (and through it the instance / class) for the
/// life of the session.
///
/// Wraps a `Py<PyDict>` so `clone_ref` produces shared handles to the same
/// underlying dict; the GIL serializes access, so no lock is needed.
#[derive(Debug)]
pub struct InstanceStore {
    /// uuid bytes → wrapper (`ClassInstance` for instances, `ClassType` for
    /// classes — one routing namespace, since `call_method` /
    /// `lookup_lazy_attrs` are the shared wrapper surface). Alias checks
    /// compare wrapped objects by identity (`is`), so a metaclass overriding
    /// `__eq__`/`__hash__` (which can make a class unhashable, or make
    /// distinct classes compare equal) is never consulted.
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

    /// Registers an instance wrapper under its uuid. Re-sending the same
    /// wrapper (or another wrapper of the same object) overwrites the entry;
    /// an id already routing to a different object is rejected.
    pub fn register_instance(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = wrapper.py();
        self.check_no_alias(uuid, &wrapper.getattr(intern!(py, "value"))?)?;
        self.objects
            .bind(py)
            .set_item(PyBytes::new(py, uuid.as_bytes()), wrapper)
    }

    /// Registers a `ClassType` wrapper for the class under its uuid, for
    /// routing (method calls, `__call__` construction, lazy class attrs) and
    /// identity round-trips. Re-granting the same class with a new wrapper
    /// overwrites (last policy wins); an id already routing to a different
    /// object is rejected.
    pub fn register_class_wrapper(
        &self,
        uuid: &MontyUuid,
        class: &Bound<'_, PyAny>,
        wrapper: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let py = wrapper.py();
        self.check_no_alias(uuid, class)?;
        self.objects
            .bind(py)
            .set_item(PyBytes::new(py, uuid.as_bytes()), wrapper)
    }

    /// [`Self::register_class_wrapper`], but only when the class is not yet
    /// registered — used for the `ClassType` a `ClassInstance` materializes,
    /// so an auto-built default policy never clobbers an explicitly granted
    /// one. Still rejects an id aliasing a different object.
    pub fn register_class_wrapper_if_absent(
        &self,
        uuid: &MontyUuid,
        class: &Bound<'_, PyAny>,
        wrapper: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let py = wrapper.py();
        self.check_no_alias(uuid, class)?;
        if self.objects.bind(py).contains(PyBytes::new(py, uuid.as_bytes()))? {
            Ok(())
        } else {
            self.register_class_wrapper(uuid, class, wrapper)
        }
    }

    /// Errors if `uuid` is already registered for an object other than
    /// `value` (compared by identity). Two wrappers sharing an id but
    /// wrapping different objects would silently re-route method calls and
    /// round-trips from one host object to the other.
    fn check_no_alias(&self, uuid: &MontyUuid, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = value.py();
        if let Some(existing) = self.objects.bind(py).get_item(PyBytes::new(py, uuid.as_bytes()))?
            && !existing.getattr(intern!(py, "value"))?.is(value)
        {
            return Err(PyValueError::new_err(format!(
                "wrapper id {uuid} already identifies a different object in this session"
            )));
        }
        Ok(())
    }

    /// Looks up the wrapper registered for `uuid` (instance or class type).
    pub fn get(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .objects
            .bind(py)
            .get_item(PyBytes::new(py, uuid.as_bytes()))?
            .map(Bound::unbind))
    }

    /// Looks up the class registered for `uuid`: the `value` of its
    /// `ClassType` wrapper.
    pub fn get_class(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        let Some(wrapper) = self.get(py, uuid)? else {
            return Ok(None);
        };
        Ok(Some(wrapper.bind(py).getattr(intern!(py, "value"))?.unbind()))
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
}

/// The error a routed call on an unregistered uuid raises: the store is
/// empty because the session was restored into a fresh session.
fn store_miss_error(name: &str, uuid: &MontyUuid) -> PyErr {
    PyRuntimeError::new_err(format!(
        "no host object registered for method call '{name}' (id {uuid}) — \
         the instance store is empty after loading a dump into a fresh session"
    ))
}

/// Reads a `ClassInstance`/`ClassType` wrapper's `id` field (a `uuid.UUID`,
/// uuid4 by default) as a [`MontyUuid`] — the wrapper owns its identity, so
/// re-sending the same wrapper preserves it and the host never derives ids
/// from addresses.
fn wrapper_uuid(wrapper: &Bound<'_, PyAny>) -> PyResult<MontyUuid> {
    let py = wrapper.py();
    let bytes: [u8; 16] = wrapper
        .getattr(intern!(py, "id"))?
        .getattr(intern!(py, "bytes"))
        .and_then(|bytes| bytes.extract())
        .map_err(|_| PyTypeError::new_err("ClassInstance.id must be a uuid.UUID"))?;
    Ok(MontyUuid::from_bytes(bytes))
}

/// Read-only proxy for a class instance the host has no original object for:
/// a sandbox-defined instance, or a host instance returned after the session
/// was restored into a fresh session.
///
/// A plain data holder — attribute values were converted when the value
/// crossed the wire, and there is no live sandbox object behind it.
#[pyclass(name = "MontyClassProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyClassProxy {
    /// Class name (e.g., "Point", "User").
    name: String,
    /// Whether the origin side reported `dataclasses.is_dataclass(obj)`.
    is_dataclass: bool,
    /// The instance's attributes, converted to Python values.
    attributes: Py<PyDict>,
}

#[pymethods]
impl PyMontyClassProxy {
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

    /// `MontyClassProxy(name='Point', attributes={'x': 1})`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs_repr: String = self.attributes.bind(py).repr()?.extract()?;
        Ok(format!(
            "MontyClassProxy(name='{}', attributes={attrs_repr})",
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

/// Cached import of the `pydantic_monty.ClassType` wrapper class.
fn get_class_type_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLASS_TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    CLASS_TYPE.import(py, "pydantic_monty.class_instance", "ClassType")
}

/// Cached import of `dataclasses.FrozenInstanceError` exception class.
pub fn get_frozen_instance_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FROZEN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FROZEN_ERROR.import(py, "dataclasses", "FrozenInstanceError")
}
