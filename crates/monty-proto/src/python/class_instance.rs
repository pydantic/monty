//! Class-instance conversion between Python and Monty.
//!
//! This module holds:
//! - [`InstanceStore`]: the per-session uuid → wrapper/class maps that route
//!   method calls and lazy attribute lookups (on instances and class types
//!   alike — construction arrives as a `__call__` method call) back to the
//!   host objects, and resolve returned instances to their original objects
//! - [`PyMontyClassProxy`] / [`PyMontyClassTypeProxy`]: read-only stand-ins
//!   for instances and classes the host has no original object for
//!
//! The wrappers are read and the proxies built by `encode` / `decode`.
//!
//! Identity comes from each wrapper's `id` field: instances default to a
//! fresh uuid4 per wrapper, classes to `pydantic_monty`'s name-keyed
//! `type_id_cache` (an instance's class and parent classes get `ClassType`
//! wrappers built on demand). Never `id()`, which leaks heap addresses to
//! the worker and is reused by CPython.

use std::{
    cell::RefCell,
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    ptr,
    sync::{Arc, OnceLock},
};

use monty_types::{
    MontyUuid,
    unstable::{ClassEq, ClassHash},
};
use pyo3::{
    Bound,
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    pyclass::CompareOp,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyTuple},
};

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
    pub(super) fn register(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
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
    pub(super) fn register_class_type_if_absent(&self, uuid: &MontyUuid, wrapper: &Bound<'_, PyAny>) -> PyResult<()> {
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
    pub fn get(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .objects
            .bind(py)
            .get_item(PyBytes::new(py, uuid.as_bytes()))?
            .map(Bound::unbind))
    }

    /// Looks up the class registered for `uuid`: the `value` of its
    /// `ClassType` wrapper.
    pub(super) fn get_class(&self, py: Python<'_>, uuid: &MontyUuid) -> PyResult<Option<Py<PyAny>>> {
        let Some(wrapper) = self.get(py, uuid)? else {
            return Ok(None);
        };
        Ok(Some(wrapper.bind(py).getattr(intern!(py, "value"))?.unbind()))
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

/// A class as a proxy records it: the fields of its arena node other than
/// the attrs, which the proxies keep as Python dicts. Proxies share one per
/// class node through an `Arc`, so many instances of a class with many fields
/// do not copy its rules once each.
#[derive(Debug, Clone)]
pub(super) struct ClassHeader {
    /// The Python-visible class name.
    pub(super) name: String,
    /// Identity of the class, generated by whichever side defined it.
    pub(super) id: MontyUuid,
    /// True for a host-defined class, false for a sandbox-defined one.
    pub(super) host_defined: bool,
    /// Whether `dataclasses.is_dataclass` is true for the class.
    pub(super) is_dataclass: bool,
    /// How the sandbox compares instances, which the proxy's `__eq__` follows.
    pub(super) eq: ClassEq,
    /// How the sandbox hashes instances, which the proxy's `__hash__` follows.
    pub(super) hash: ClassHash,
}

/// Read-only proxy for a class instance the host has no original object for:
/// a sandbox-defined instance, or a host instance returned after the session
/// was restored into a fresh session.
///
/// A plain data holder — attribute values were converted when the value
/// crossed the wire. It keeps the wire ids, so passing it back into the
/// sandbox hands over the original object (a live sandbox instance resolves
/// by identity; the attributes are not applied).
///
/// `==` and `hash()` follow the rule the class reported (see [`ClassEq`] and
/// [`ClassHash`]): a `@dataclass` compares and hashes its fields. Where the
/// sandbox's answer needs sandbox code (a user `__eq__` or `__hash__`), the
/// proxy falls back to instance identity rather than guess.
#[pyclass(name = "MontyClassProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyClassProxy {
    /// The instance's class as it crossed the wire (name, id, dataclass-ness).
    pub(super) class_type: Arc<ClassHeader>,
    /// Identity of the instance, generated by the side that defined it.
    pub(super) instance_id: MontyUuid,
    /// The instance's attributes, converted to Python values.
    pub(super) attributes: Py<PyDict>,
    /// The eager class attrs that crossed with the instance's class, kept so
    /// the class crosses back as it arrived.
    pub(super) class_attributes: Py<PyDict>,
    /// `__hash__`'s result, computed once: a shared sub-object is one proxy,
    /// so caching keeps hashing a graph of nested dataclasses linear.
    pub(super) hash: OnceLock<Result<u64, PyErr>>,
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

    /// A copy of the instance's attributes as a plain dict. A copy so host
    /// edits cannot change the proxy's `==`, or its hash while it is a set
    /// member or dict key.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        Ok(self.attributes.bind(py).copy()?.unbind())
    }

    /// `MontyClassProxy(name='Point', attributes={'x': 1})`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs_repr: String = self.attributes.bind(py).repr()?.extract()?;
        Ok(format!(
            "MontyClassProxy(name='{}', attributes={attrs_repr})",
            self.class_type.name
        ))
    }

    /// A `@dataclass` `__eq__` is replayed (same class, fields `==` in order);
    /// any other rule compares instance ids. A non-proxy operand is
    /// `NotImplemented`.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let Ok(other) = other.cast::<Self>() else {
            return Ok(py.NotImplemented());
        };
        let other = other.get();
        let equal = match &self.class_type.eq {
            ClassEq::Fields(fields) if self.class_type.id == other.class_type.id => {
                self.fields_equal(py, other, fields)?
            }
            ClassEq::Identity | ClassEq::Fields(_) | ClassEq::Custom => self.instance_id == other.instance_id,
        };
        Ok(equal.into_pyobject(py)?.to_owned().into_any().unbind())
    }

    /// Mirrors the sandbox's hash rule, consistently with `__eq__`: a
    /// `@dataclass` hashes its fields, an unhashable class raises `TypeError`,
    /// and identity hashes the instance id. The result, error included, is
    /// cached.
    fn __hash__(&self, py: Python<'_>) -> PyResult<u64> {
        let cached = if let Some(cached) = self.hash.get() {
            cached
        } else {
            let computed = self.compute_hash(py);
            self.hash.get_or_init(|| computed)
        };
        cached.as_ref().copied().map_err(|err| err.clone_ref(py))
    }
}

impl PyMontyClassProxy {
    /// The hash `__hash__` caches.
    fn compute_hash(&self, py: Python<'_>) -> PyResult<u64> {
        match (&self.class_type.hash, &self.class_type.eq) {
            (ClassHash::Fields(fields), _) => {
                let mut hasher = DefaultHasher::new();
                PyTuple::new(py, self.field_values(py, fields)?)?
                    .hash()?
                    .hash(&mut hasher);
                Ok(hasher.finish())
            }
            (ClassHash::Unhashable, _) => Err(PyTypeError::new_err(format!(
                "unhashable type: '{}'",
                self.class_type.name
            ))),
            // A user `__hash__` beside a generated `__eq__`: the host cannot
            // run it, and an identity hash would split equal proxies, so it
            // hashes what `__eq__` compares, skipping values it cannot hash.
            (ClassHash::Custom, ClassEq::Fields(fields)) => {
                let mut hasher = DefaultHasher::new();
                for value in self.field_values(py, fields)? {
                    match value.hash() {
                        Ok(hash) => hash.hash(&mut hasher),
                        Err(err) if err.is_instance_of::<PyTypeError>(py) => {}
                        Err(err) => return Err(err),
                    }
                }
                Ok(hasher.finish())
            }
            // A user `__hash__` with identity or user `==`: identity is the
            // only hash consistent with the proxy's `==`.
            (ClassHash::Identity | ClassHash::Custom, _) => Ok(hash_uuid(&self.instance_id)),
        }
    }

    /// The generated `__eq__`'s field walk: each pair compared with the `==`
    /// operator, stopping at the first unequal one.
    ///
    /// Two proxies of one sandbox object are snapshots that may differ (the
    /// object changed between results), so they compare fields too, a pair
    /// that is the same Python object counting as equal: `p == p` holds as
    /// the sandbox's `self is other` check makes it.
    fn fields_equal(&self, py: Python<'_>, other: &Self, fields: &[String]) -> PyResult<bool> {
        let same_object = self.instance_id == other.instance_id;
        EqualPairs::compare(self, other, || {
            for field in fields {
                let a = self.field_value(py, field)?;
                let b = other.field_value(py, field)?;
                if !((same_object && a.is(&b)) || a.rich_compare(&b, CompareOp::Eq)?.is_truthy()?) {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }

    /// The values of `fields`, in order.
    fn field_values<'py>(&self, py: Python<'py>, fields: &[String]) -> PyResult<Vec<Bound<'py, PyAny>>> {
        fields.iter().map(|field| self.field_value(py, field)).collect()
    }

    /// One field's value, raising the `AttributeError` the generated method
    /// would for a field the instance no longer has.
    fn field_value<'py>(&self, py: Python<'py>, field: &str) -> PyResult<Bound<'py, PyAny>> {
        self.attributes.bind(py).get_item(field)?.ok_or_else(|| {
            PyAttributeError::new_err(format!("'{}' object has no attribute '{field}'", self.class_type.name))
        })
    }
}

thread_local! {
    /// See [`EqualPairs`].
    static EQUAL_PAIRS: RefCell<EqualPairs> = RefCell::new(EqualPairs::default());
}

/// Proxy pairs found equal during the outermost proxy `==` on this thread.
///
/// A shared sub-object decodes to one proxy, so two separately built but
/// equal graphs of nested dataclasses meet the same pair once per path,
/// exponentially often; remembering equal pairs keeps the walk linear.
/// Unequal pairs need no memo: the first one ends the walk.
#[derive(Default)]
struct EqualPairs {
    /// Nesting depth of proxy comparisons in progress; the set is cleared when
    /// the outermost one ends, since addresses are only stable while it runs.
    depth: usize,
    /// Address pairs of proxies compared equal.
    pairs: HashSet<(usize, usize)>,
}

impl EqualPairs {
    /// Runs `compare` for the pair `(a, b)` unless it was already found equal.
    fn compare(
        a: &PyMontyClassProxy,
        b: &PyMontyClassProxy,
        compare: impl FnOnce() -> PyResult<bool>,
    ) -> PyResult<bool> {
        let key = (ptr::from_ref(a) as usize, ptr::from_ref(b) as usize);
        let known = EQUAL_PAIRS.with_borrow_mut(|memo| {
            memo.depth += 1;
            memo.pairs.contains(&key)
        });
        let _exit = EqualPairsExit;
        let equal = known || compare()?;
        if equal {
            EQUAL_PAIRS.with_borrow_mut(|memo| memo.pairs.insert(key));
        }
        Ok(equal)
    }
}

/// Leaves one level of [`EqualPairs`] on every path, error included.
struct EqualPairsExit;

impl Drop for EqualPairsExit {
    fn drop(&mut self) {
        EQUAL_PAIRS.with_borrow_mut(|memo| {
            memo.depth -= 1;
            if memo.depth == 0 {
                memo.pairs.clear();
            }
        });
    }
}

/// Hashes a wire uuid, for proxies that compare by identity.
fn hash_uuid(uuid: &MontyUuid) -> u64 {
    let mut hasher = DefaultHasher::new();
    uuid.hash(&mut hasher);
    hasher.finish()
}

/// Read-only proxy for a host class the store has no class object for: one
/// returned (as a value, or as `type(x)`) after the session was restored into
/// a fresh session. Keeps the wire id, so passing it back into the sandbox
/// re-enters as the same type object.
#[pyclass(name = "MontyClassTypeProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyClassTypeProxy {
    /// The class as it crossed the wire (name, id, dataclass-ness).
    pub(super) class_type: Arc<ClassHeader>,
    /// The eager class attrs that crossed with it, converted to Python values.
    pub(super) attributes: Py<PyDict>,
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

    /// A copy of the eager class attrs as a plain dict, so host edits cannot
    /// reach the attrs the class crosses back with.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        Ok(self.attributes.bind(py).copy()?.unbind())
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

    /// Hashes the class id, consistently with `__eq__`.
    fn __hash__(&self) -> u64 {
        hash_uuid(&self.class_type.id)
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
pub(super) fn wrapper_uuid(wrapper: &Bound<'_, PyAny>, kind: &str) -> PyResult<MontyUuid> {
    py_uuid(&wrapper.getattr(intern!(wrapper.py(), "id"))?, &format!("{kind}.id"))
}

/// A `uuid.UUID` as a [`MontyUuid`]; `what` names the value in the error.
pub(super) fn py_uuid(value: &Bound<'_, PyAny>, what: &str) -> PyResult<MontyUuid> {
    let bytes: [u8; 16] = value
        .getattr(intern!(value.py(), "bytes"))
        .and_then(|bytes| bytes.extract())
        .map_err(|_| PyTypeError::new_err(format!("{what} must be a uuid.UUID")))?;
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
