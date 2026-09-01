//! Class-instance conversion between Python and Monty.
//!
//! This module handles:
//! - Converting `pydantic_monty.ClassInstance` / `pydantic_monty.ClassType`
//!   wrappers to `MontyObject::ClassInstance` / `MontyObject::Type`
//! - Converting `MontyObject::ClassInstance` back to Python: the original
//!   wrapped object when the instance is in the session's [`InstanceStore`],
//!   else a read-only [`PyMontyClassInstance`] proxy
//! - [`InstanceStore`]: the per-session uuid → wrapper/class maps that route
//!   method calls, lazy attribute lookups, and instantiations back to the
//!   host objects
//!
//! Identity is a host-minted uuid4 per instance and per class — never
//! `id()`, which leaks heap addresses to the worker and is reused by CPython.

use monty_types::{ClassType, DictPairs, MontyObject, MontyType, MontyUuid};
use pyo3::{
    Bound,
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyTuple, PyType},
};

use super::convert::{MAX_INPUT_DEPTH, monty_to_py_inner, py_to_monty, py_type_object_to_monty};

/// Checks if a Python object is a dataclass instance (not a type).
///
/// Copied from pydantic's `is_dataclass` logic.
pub fn is_dataclass(value: &Bound<'_, PyAny>) -> bool {
    value
        .hasattr(intern!(value.py(), "__dataclass_fields__"))
        .unwrap_or(false)
        && !value.is_instance_of::<PyType>()
}

/// Checks if a Python class has `@dataclass(frozen=True)` semantics.
fn is_frozen_dataclass_class(class: &Bound<'_, PyAny>) -> bool {
    class
        .getattr(intern!(class.py(), "__dataclass_params__"))
        .and_then(|params| params.getattr(intern!(class.py(), "frozen")))
        .and_then(|frozen| frozen.extract::<bool>())
        .unwrap_or(false)
}

/// Checks if a Python object is a `pydantic_monty.ClassInstance` wrapper.
pub fn is_class_instance_wrapper(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(get_class_instance_class(value.py())?)
}

/// Checks if a Python object is a `pydantic_monty.ClassType` wrapper.
pub fn is_class_type_wrapper(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(get_class_type_class(value.py())?)
}

/// Converts a `pydantic_monty.ClassInstance` wrapper to
/// `MontyObject::ClassInstance`, registering the wrapper in `store` under the
/// instance's uuid so later method calls / lazy lookups / round-tripped
/// returns resolve to the original object.
///
/// Eager attrs come from `wrapper.get_eager_attrs()` and are converted with
/// `py_to_monty`, so nested wrappers inside them register themselves too.
pub fn class_instance_to_monty(wrapper: &Bound<'_, PyAny>, store: &InstanceStore, depth: u8) -> PyResult<MontyObject> {
    let py = wrapper.py();
    let instance = wrapper.getattr(intern!(py, "class_instance"))?;

    let frozen: bool = wrapper.call_method0(intern!(py, "get_frozen"))?.extract()?;
    let class_type = class_type_for(
        &instance.get_type().into_any(),
        store,
        WrapperPolicy {
            is_dataclass: is_dataclass(&instance),
            frozen,
        },
        depth,
    )?;
    let instance_id = store.instance_uuid(&instance)?;

    let eager = wrapper
        .call_method0(intern!(py, "get_eager_attrs"))?
        .cast_into::<PyDict>()?;
    let mut attrs = Vec::with_capacity(eager.len());
    for (key, value) in eager.iter() {
        attrs.push((py_to_monty(&key, store, depth)?, py_to_monty(&value, store, depth)?));
    }

    store.register_instance(&instance_id, wrapper)?;

    Ok(MontyObject::ClassInstance {
        class_type,
        instance_id,
        attrs: attrs.into(),
    })
}

/// Converts a `pydantic_monty.ClassType` wrapper to `MontyObject::Type`,
/// registering the class and wrapper in `store` under the class uuid so
/// sandbox instantiations (`Type.init` granted) route back to it.
pub fn class_type_to_monty(wrapper: &Bound<'_, PyAny>, store: &InstanceStore, depth: u8) -> PyResult<MontyObject> {
    let py = wrapper.py();
    let class = wrapper.getattr(intern!(py, "class_type"))?;
    if !class.is_instance_of::<PyType>() {
        return Err(PyTypeError::new_err("ClassType.class_type must be a class"));
    }
    let class_type = class_type_for(
        &class,
        store,
        WrapperPolicy {
            is_dataclass: class.hasattr(intern!(py, "__dataclass_fields__"))?,
            frozen: is_frozen_dataclass_class(&class),
        },
        depth,
    )?;
    store.register_class_wrapper(&class_type.id, &class, wrapper)?;
    Ok(MontyObject::Type(MontyType::Instance(Box::new(class_type))))
}

/// The per-wrapper flags stamped onto the outgoing wire `Type`.
#[derive(Clone, Copy)]
struct WrapperPolicy {
    is_dataclass: bool,
    frozen: bool,
}

/// Builds the wire [`ClassType`] for a host class: dedup-minted uuid, wrapper
/// policy flags, and `parents` from `__bases__` (skipping `object`; builtin
/// bases map through the round-trip type table, class bases recurse with
/// their own uuids and default flags).
fn class_type_for(
    class: &Bound<'_, PyAny>,
    store: &InstanceStore,
    policy: WrapperPolicy,
    depth: u8,
) -> PyResult<ClassType> {
    let py = class.py();
    if depth >= MAX_INPUT_DEPTH {
        return Err(PyRuntimeError::new_err("Max input depth exceeded"));
    }
    let name: String = class.getattr(intern!(py, "__name__"))?.extract()?;
    let mut parents = Vec::new();
    for base in class.getattr(intern!(py, "__bases__"))?.cast::<PyTuple>()?.iter() {
        if base.is(get_object_type(py)?) {
            continue;
        }
        if let Ok(base_type) = base.clone().cast_into::<PyType>()
            && let Some(builtin) = py_type_object_to_monty(&base_type)?
        {
            parents.push(builtin);
            continue;
        }
        let base_policy = WrapperPolicy {
            is_dataclass: base.hasattr(intern!(py, "__dataclass_fields__"))?,
            frozen: is_frozen_dataclass_class(&base),
        };
        parents.push(MontyType::Instance(Box::new(class_type_for(
            &base,
            store,
            base_policy,
            depth + 1,
        )?)));
    }
    Ok(ClassType {
        name,
        id: store.type_uuid(class)?,
        host_defined: true,
        parents,
        is_dataclass: policy.is_dataclass,
        frozen: policy.frozen,
    })
}

/// Converts a `MontyObject::ClassInstance` to a Python object.
///
/// When `instance_id` is found in `store`, returns the ORIGINAL wrapped object
/// (identity preserved — `result is obj` holds). Otherwise — a sandbox-defined
/// instance, or an id from a session restored into a fresh process — builds a
/// read-only [`PyMontyClassInstance`] proxy.
///
/// `depth` is the caller's current recursion depth; it is forwarded to
/// `monty_to_py_inner` so nested attr values respect the output-depth limit.
pub fn class_instance_to_py(
    py: Python<'_>,
    class_type: &ClassType,
    instance_id: &MontyUuid,
    attrs: &DictPairs,
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
            name: class_type.name.clone(),
            is_dataclass: class_type.is_dataclass,
            attributes: attributes.unbind(),
        };
        Ok(Py::new(py, proxy)?.into_any())
    }
}

/// Resolves a class type object crossing back out of the sandbox: the
/// original class when its uuid is registered, else `None` (a sandbox class,
/// or a host class from a session restored into a fresh process).
pub fn class_type_to_py(py: Python<'_>, class_type: &ClassType, store: &InstanceStore) -> PyResult<Option<Py<PyAny>>> {
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

/// Per-session identity maps from host-minted uuids to the Python objects
/// behind them.
///
/// Populated whenever a `ClassInstance`/`ClassType` wrapper crosses into the
/// sandbox; consulted to answer method calls (`FunctionCall.instance_id`),
/// instantiations (`FunctionCall.type_id`), lazy attribute lookups
/// (`NameLookup.instance_id`), and to hand original objects back when the
/// sandbox returns them. Registered entries pin their wrapper (and through it
/// the instance / class) for the life of the session, which is also what
/// keeps the `id()`-keyed *internal* dedup maps sound — those raw ids never
/// cross the wire.
///
/// Wraps `Py<PyDict>`s so `clone_ref` produces shared handles to the same
/// underlying dicts; the GIL serializes access, so no lock is needed.
#[derive(Debug)]
pub struct InstanceStore {
    /// uuid bytes → `ClassInstance` wrapper (pins wrapper + instance).
    instances: Py<PyDict>,
    /// `id(instance)` → uuid bytes; dedup so re-sending an object reuses its
    /// uuid. Sound because `instances` pins the instance for the session.
    instance_ids: Py<PyDict>,
    /// uuid bytes → `(class, ClassType wrapper | None)`; pins the class, and
    /// carries the wrapper whose policy gates instantiation.
    classes: Py<PyDict>,
    /// `id(class)` → uuid bytes; dedup, sound because `classes` pins.
    class_ids: Py<PyDict>,
}

impl InstanceStore {
    /// Creates a new empty store.
    #[must_use]
    pub fn new(py: Python<'_>) -> Self {
        Self {
            instances: PyDict::new(py).unbind(),
            instance_ids: PyDict::new(py).unbind(),
            classes: PyDict::new(py).unbind(),
            class_ids: PyDict::new(py).unbind(),
        }
    }

    /// Creates a shared handle to this store (cheap Python refcount bump).
    ///
    /// The clone points to the **same** underlying Python dicts, so
    /// insertions through any handle are visible to all others.
    #[must_use]
    pub fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            instances: self.instances.clone_ref(py),
            instance_ids: self.instance_ids.clone_ref(py),
            classes: self.classes.clone_ref(py),
            class_ids: self.class_ids.clone_ref(py),
        }
    }

    /// Returns the instance's session uuid, minting one on first sight.
    ///
    /// Dedup is keyed by object identity *inside the host process only*, so
    /// re-sending the same object (directly or via a round-trip) reuses its
    /// uuid and identity is preserved.
    pub fn instance_uuid(&self, instance: &Bound<'_, PyAny>) -> PyResult<MontyUuid> {
        mint_or_reuse(self.instance_ids.bind(instance.py()), instance)
    }

    /// Returns the class's session uuid, minting one on first sight and
    /// pinning the class (with no instantiation wrapper) in the store.
    pub fn type_uuid(&self, class: &Bound<'_, PyAny>) -> PyResult<MontyUuid> {
        let py = class.py();
        let uuid = mint_or_reuse(self.class_ids.bind(py), class)?;
        let key = PyBytes::new(py, uuid.as_bytes());
        // First sight pins the class with no wrapper; a `ClassType` wrapper
        // upgrades the entry via `register_class_wrapper`.
        if !self.classes.bind(py).contains(&key)? {
            self.classes.bind(py).set_item(key, (class, py.None()))?;
        }
        Ok(uuid)
    }

    /// Registers an instance wrapper under its uuid. Idempotent — re-sending
    /// the same instance overwrites (last wrapper wins).
    pub fn register_instance(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = wrapper.py();
        self.instances
            .bind(py)
            .set_item(PyBytes::new(py, uuid.as_bytes()), wrapper)
    }

    /// Registers a `ClassType` wrapper for the class under its uuid — the
    /// wrapper's policy is what instantiation dispatch consults.
    pub fn register_class_wrapper(
        &self,
        uuid: &MontyUuid,
        class: &Bound<'_, PyAny>,
        wrapper: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let py = wrapper.py();
        self.classes
            .bind(py)
            .set_item(PyBytes::new(py, uuid.as_bytes()), (class, wrapper))
    }

    /// Looks up the instance wrapper registered for `uuid`.
    pub fn get(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .instances
            .bind(py)
            .get_item(PyBytes::new(py, uuid.as_bytes()))?
            .map(Bound::unbind))
    }

    /// Looks up the class registered for `uuid`.
    pub fn get_class(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        let Some(entry) = self.classes.bind(py).get_item(PyBytes::new(py, uuid.as_bytes()))? else {
            return Ok(None);
        };
        Ok(Some(entry.get_item(0)?.unbind()))
    }

    /// Calls `wrapper.call_method(name, args, kwargs)` on the instance
    /// registered for `uuid`.
    ///
    /// A store miss raises `RuntimeError`: it means the session was restored
    /// into a process that never sent the instance (e.g. `load_session`).
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

    /// Calls `wrapper.construct(args, kwargs)` on the class registered for
    /// `uuid` — the wrapper's host-side `init` policy decides whether
    /// construction is allowed; nothing about instantiability crosses the
    /// wire.
    ///
    /// A store miss, or a class that crossed without a `ClassType` wrapper,
    /// raises `RuntimeError`.
    pub fn instantiate(
        &self,
        py: Python<'_>,
        uuid: &MontyUuid,
        name: &str,
        args: &Bound<'_, PyTuple>,
        kwargs: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        let entry = self.classes.bind(py).get_item(PyBytes::new(py, uuid.as_bytes()))?;
        let wrapper = entry.map(|e| e.get_item(1)).transpose()?;
        let Some(wrapper) = wrapper.filter(|w| !w.is_none()) else {
            return Err(PyRuntimeError::new_err(format!(
                "no host class registered for instantiation of '{name}' (id {uuid}) — \
                 pass the class as a pydantic_monty.ClassType(..., init=True)"
            )));
        };
        wrapper
            .call_method1(intern!(py, "construct"), (args, kwargs))
            .map(Bound::unbind)
    }

    /// Calls `wrapper.lookup_lazy_attrs(name)` on the instance registered for
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

/// Reuses the uuid recorded for `obj` in `ids` (keyed by `id(obj)`, which the
/// store's pinning keeps stable) or mints a fresh uuid4.
fn mint_or_reuse(ids: &Bound<'_, PyDict>, obj: &Bound<'_, PyAny>) -> PyResult<MontyUuid> {
    let key = obj.as_ptr() as usize;
    if let Some(existing) = ids.get_item(key)? {
        let bytes: [u8; 16] = existing.extract()?;
        return Ok(MontyUuid::from_bytes(bytes));
    }
    let uuid = MontyUuid::from_bytes(*uuid::Uuid::new_v4().as_bytes());
    ids.set_item(key, PyBytes::new(ids.py(), uuid.as_bytes()))?;
    Ok(uuid)
}

/// The error a method call on an unregistered instance uuid raises.
fn store_miss_error(name: &str, uuid: &MontyUuid) -> PyErr {
    PyRuntimeError::new_err(format!(
        "no host instance registered for method call '{name}' (id {uuid}) — \
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

/// Cached import of the `pydantic_monty.ClassType` wrapper class.
fn get_class_type_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLASS_TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    CLASS_TYPE.import(py, "pydantic_monty.class_instance", "ClassType")
}

/// Cached reference to the `object` base type, skipped when walking bases.
fn get_object_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static OBJECT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    OBJECT.import(py, "builtins", "object")
}

/// Cached import of `dataclasses.FrozenInstanceError` exception class.
pub fn get_frozen_instance_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static DC_FROZEN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    DC_FROZEN_ERROR.import(py, "dataclasses", "FrozenInstanceError")
}
