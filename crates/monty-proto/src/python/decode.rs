//! Sandbox values flowing OUT to the host: [`DecodedArena`] turns one
//! message's node arena into Python objects.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use monty_types::{
    BuiltinsFunctions, MontyException, MontyObject,
    unstable::{self, ClassTypeNode, MontyGraph, MontyNode, NodeId},
};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBool, PyBytes, PyComplex, PyDate, PyDelta, PyDict, PyFrozenSet, PyList, PySet, PyString, PyTuple},
};

use super::{
    class_instance::{ClassHeader, InstanceStore, PyMontyClassProxy, PyMontyClassTypeProxy},
    convert::{
        PyMontyFileHandle, get_decimal_type, get_namedtuple, get_pure_posix_path, host_type_object, import_builtins,
        monty_datetime_to_py, monty_time_to_py, monty_timezone_to_py,
    },
    exceptions::exc_monty_to_py,
    external_module::PyExternalModule,
    std_type_proxy::{PyMontyStdTypeProxy, StdTypeRef},
};

/// The most key comparison work CPython may do while one arena's dicts and sets
/// are built, about 10 ms of colliding `int` keys.
///
/// The sandbox hashes `int`s by their bits but CPython reduces them modulo
/// `2**61 - 1`, so keys like `k * (2**61 - 1)` are free to build in the sandbox
/// yet all collide on the host, where inserting n of them costs O(n²).
/// Each comparison is weighted by the key's expanded size (see [`expanded_size`]).
const MAX_KEY_COLLISIONS: u64 = 1 << 20;

/// The expanded key size every arena may hash, whatever its size (about 100 ms).
///
/// The arena keeps sharing, so `a = (a, a)` nested 40 deep is 41 tuple nodes, but
/// CPython before 3.14 does not cache tuple hashes and walks all 2⁴⁰ leaves.
const MIN_KEY_HASH_BUDGET: u64 = 1 << 24;

/// Expanded key size each unit of the arena's own size (see [`own_size`]) adds to
/// [`MIN_KEY_HASH_BUDGET`], so unshared keys, which expand to no more than the
/// arena itself, always fit.
const KEY_HASH_BUDGET_PER_UNIT: u64 = 4;

/// Converts one value to a native Python object. A class instance found in
/// `store` resolves to the ORIGINAL wrapped object (identity preserved);
/// otherwise it becomes a read-only `MontyClassProxy`.
pub fn monty_to_py(py: Python<'_>, value: &MontyObject, store: &InstanceStore) -> PyResult<Py<PyAny>> {
    let (graph, root) = unstable::graph_parts(value);
    Ok(DecodedArena::new(py, graph, store)?.get(py, root))
}

/// One message's arena as Python objects.
///
/// Every node is converted once, in arena order (a loop, not a recursion, so
/// nesting depth costs no native stack), so a node referenced twice (a shared
/// sub-object, an argument passed twice) is one Python object. A class node
/// resolves to the registered class or one `MontyClassTypeProxy` shared by
/// every instance of it.
pub struct DecodedArena {
    built: Vec<Py<PyAny>>,
}

impl DecodedArena {
    /// Decodes every node of `graph`.
    pub fn new(py: Python<'_>, graph: &MontyGraph, store: &InstanceStore) -> PyResult<Self> {
        let mut decoder = Decoder {
            py,
            graph,
            store,
            built: Vec::with_capacity(graph.len()),
            namedtuple_types: HashMap::new(),
            classes: HashMap::new(),
            expanded: Vec::with_capacity(graph.len()),
            key_collisions: 0,
            key_hash_work: 0,
            key_hash_budget: MIN_KEY_HASH_BUDGET.max(KEY_HASH_BUDGET_PER_UNIT.saturating_mul(arena_size(graph))),
        };
        for node in graph.nodes() {
            let obj = decoder.decode(node)?;
            decoder.built.push(obj);
            let size = expanded_size(node, &decoder.expanded);
            decoder.expanded.push(size);
        }
        Ok(Self { built: decoder.built })
    }

    /// The object for `id`, which the arena's validation put in range.
    #[must_use]
    pub fn get(&self, py: Python<'_>, id: NodeId) -> Py<PyAny> {
        self.built[id.index()].clone_ref(py)
    }
}

/// The forward pass over one arena.
struct Decoder<'a, 'py> {
    py: Python<'py>,
    graph: &'a MontyGraph,
    store: &'a InstanceStore,
    /// Objects built so far, one per node decoded.
    built: Vec<Py<PyAny>>,
    /// Namedtuple types built for this arena, by `(type_name, field_names)`.
    namedtuple_types: HashMap<(String, Vec<String>), Py<PyAny>>,
    /// Each class node's header and eager attrs, built once and shared by
    /// its proxies so their cost does not scale with the instance count.
    classes: HashMap<NodeId, (Arc<ClassHeader>, Py<PyDict>)>,
    /// Each decoded node's [`expanded_size`], by index.
    expanded: Vec<u64>,
    /// Key comparison work charged so far against [`MAX_KEY_COLLISIONS`].
    key_collisions: u64,
    /// Expanded size of the keys hashed so far, charged against `key_hash_budget`.
    key_hash_work: u64,
    /// The expanded key size this arena may hash: see [`MIN_KEY_HASH_BUDGET`].
    key_hash_budget: u64,
}

impl Decoder<'_, '_> {
    /// Converts one node; every child id is lower, so already built.
    fn decode(&mut self, node: &MontyNode) -> PyResult<Py<PyAny>> {
        let py = self.py;
        match node {
            MontyNode::None => Ok(py.None()),
            MontyNode::Ellipsis => Ok(py.Ellipsis()),
            MontyNode::NotImplemented => Ok(import_builtins(py)?.getattr(py, "NotImplemented")?),
            MontyNode::Bool(b) => Ok(PyBool::new(py, *b).to_owned().into_any().unbind()),
            MontyNode::Int(i) => Ok(i.into_pyobject(py)?.clone().into_any().unbind()),
            MontyNode::BigInt(bi) => Ok(bi.into_pyobject(py)?.clone().into_any().unbind()),
            MontyNode::Float(f) => Ok(f.into_pyobject(py)?.clone().into_any().unbind()),
            MontyNode::Complex(c) => Ok(PyComplex::from_doubles(py, c.real, c.imag).into_any().unbind()),
            MontyNode::String(s) => Ok(PyString::new(py, s).into_any().unbind()),
            MontyNode::Bytes(b) => Ok(PyBytes::new(py, b).into_any().unbind()),
            MontyNode::List(items) => Ok(PyList::new(py, self.children(items))?.into_any().unbind()),
            MontyNode::Tuple(items) => Ok(PyTuple::new(py, self.children(items))?.into_any().unbind()),
            MontyNode::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                let nt_type = self.namedtuple_type(type_name, field_names)?;
                // `_make` is a public documented method despite the leading underscore
                let instance = nt_type.bind(py).call_method1("_make", (self.children(values),))?;
                Ok(instance.into_any().unbind())
            }
            MontyNode::Dict(pairs) => {
                self.charge_key_collisions(pairs.iter().map(|(key, _)| *key))?;
                let dict = PyDict::new(py);
                for (key, value) in pairs {
                    dict.set_item(self.child(*key), self.child(*value))?;
                }
                Ok(dict.into_any().unbind())
            }
            MontyNode::Set(items) => {
                self.charge_key_collisions(items.iter().copied())?;
                let set = PySet::empty(py)?;
                for item in items {
                    set.add(self.child(*item))?;
                }
                Ok(set.into_any().unbind())
            }
            MontyNode::FrozenSet(items) => {
                self.charge_key_collisions(items.iter().copied())?;
                Ok(PyFrozenSet::new(py, self.children(items))?.into_any().unbind())
            }
            // the exception instance as a value (not raised)
            MontyNode::Exception { exc_type, arg } => {
                let exc = exc_monty_to_py(py, MontyException::new(*exc_type, arg.clone()));
                Ok(exc.into_value(py).into_any())
            }
            MontyNode::Date(date) => PyDate::new(py, date.year, date.month, date.day)
                .map(Bound::into_any)
                .map(Bound::unbind),
            MontyNode::DateTime(datetime) => monty_datetime_to_py(py, datetime),
            MontyNode::Time(time) => monty_time_to_py(py, time),
            MontyNode::TimeDelta(delta) => PyDelta::new(py, delta.days, delta.seconds, delta.microseconds, true)
                .map(Bound::into_any)
                .map(Bound::unbind),
            MontyNode::TimeZone(timezone) => monty_timezone_to_py(py, timezone),
            // a data type resolves to the host class; anything else is a proxy
            MontyNode::Type(t) => match host_type_object(py, *t)? {
                Some(ty) => Ok(ty),
                None => std_type_proxy(py, StdTypeRef::Type(*t)),
            },
            // `type` is the one builtin function on the host-class allowlist; every
            // other one crosses as a proxy carrying its name, never the host's callable
            MontyNode::BuiltinFunction(BuiltinsFunctions::Type) => import_builtins(py)?.getattr(py, "type"),
            MontyNode::BuiltinFunction(f) => std_type_proxy(py, StdTypeRef::Function(*f)),
            // a registered host class resolves to the original class object,
            // anything else to a read-only `MontyClassTypeProxy`
            MontyNode::ClassType(class) => {
                if let Some(class) = self.store.get_class(py, &class.id)? {
                    Ok(class)
                } else {
                    // this node's id: nodes decode in order
                    let id = NodeId(u32::try_from(self.built.len()).expect("arena node ids fit in u32"));
                    let (class_type, attributes) = self.class_parts(id)?;
                    let proxy = PyMontyClassTypeProxy { class_type, attributes };
                    Ok(Py::new(py, proxy)?.into_any())
                }
            }
            // the original object when its id is registered, else a
            // read-only proxy keeping the ids so it can cross back
            MontyNode::ClassInstance {
                class_type,
                instance_id,
                attrs,
            } => {
                if let Some(wrapper) = self.store.get(py, instance_id)? {
                    wrapper.bind(py).getattr("value").map(Bound::unbind)
                } else {
                    let (class_type, class_attributes) = self.class_parts(*class_type)?;
                    let proxy = PyMontyClassProxy {
                        class_type,
                        instance_id: *instance_id,
                        attributes: self.attrs_dict(attrs)?,
                        class_attributes,
                        hash: OnceLock::new(),
                    };
                    Ok(Py::new(py, proxy)?.into_any())
                }
            }
            // a host module comes back as an `ExternalModule` with the id it imported with
            MontyNode::Module { id, attrs, .. } => {
                let attrs = self.attrs_dict(attrs)?;
                let module = PyExternalModule::new(py, attrs.bind(py), None, *id)?;
                Ok(Py::new(py, module)?.into_any())
            }
            MontyNode::Path(p) => Ok(get_pure_posix_path(py)?.call1((p,))?.into_any().unbind()),
            // The worker's text is untrusted; a malformed one raises from the
            // host's own `Decimal()` rather than panicking here.
            MontyNode::Decimal(d) => Ok(get_decimal_type(py)?.call1((d,))?.into_any().unbind()),
            // a sandbox file is not an OS file, so it decodes to a
            // `MontyFileHandle` exposing `path`, `mode` and `position`
            MontyNode::FileHandle(handle) => Ok(Py::new(py, PyMontyFileHandle::from_inner(handle.clone()))?.into_any()),
            // output-only nodes become their text
            MontyNode::Repr(s) | MontyNode::Cycle(s) => Ok(PyString::new(py, s).into_any().unbind()),
            // function nodes belong to the name-lookup protocol; one reaching
            // an output value decodes to its name
            MontyNode::Function { name, .. } => Ok(PyString::new(py, name).into_any().unbind()),
        }
    }

    /// The object built for a child node.
    fn child(&self, id: NodeId) -> Py<PyAny> {
        self.built[id.index()].clone_ref(self.py)
    }

    /// The objects built for a node's children, in order.
    fn children(&self, ids: &[NodeId]) -> Vec<Py<PyAny>> {
        ids.iter().map(|id| self.child(*id)).collect()
    }

    /// Charges the work CPython does hashing `keys` and inserting them into one
    /// dict or set, failing once the arena exceeds its budgets.
    ///
    /// Hashing a key is charged its expanded size, before it is hashed. Each key
    /// is compared with every earlier key of the same hash, which costs at most
    /// its expanded size, so that is charged per earlier key. Each key is hashed
    /// once more than the insertion does. Attr dicts are not charged: their keys
    /// are `str`, whose hash is salted.
    fn charge_key_collisions(&mut self, keys: impl Iterator<Item = NodeId>) -> PyResult<()> {
        let mut seen: HashMap<isize, u64> = HashMap::new();
        for key in keys {
            let size = self.expanded[key.index()];
            self.key_hash_work = self.key_hash_work.saturating_add(size);
            if self.key_hash_work > self.key_hash_budget {
                return Err(PyValueError::new_err(
                    "dict or set keys nest too many shared values to hash",
                ));
            }
            // an unhashable key is left for the insertion to reject with its own message
            let Ok(hash) = self.built[key.index()].bind(self.py).hash() else {
                continue;
            };
            let earlier = seen.entry(hash).or_default();
            self.key_collisions = self.key_collisions.saturating_add(earlier.saturating_mul(size));
            *earlier += 1;
            if self.key_collisions > MAX_KEY_COLLISIONS {
                return Err(PyValueError::new_err("too many hash collisions among dict or set keys"));
            }
        }
        Ok(())
    }

    /// Attr pairs as a Python dict, skipping non-string keys: hosts and the
    /// sandbox only produce string attr names, so anything else is not
    /// representable.
    fn attrs_dict(&self, attrs: &[(NodeId, NodeId)]) -> PyResult<Py<PyDict>> {
        let dict = PyDict::new(self.py);
        for (key, value) in attrs {
            if let MontyNode::String(name) = self.graph.node(*key) {
                dict.set_item(name, self.child(*value))?;
            }
        }
        Ok(dict.unbind())
    }

    /// The shared header and eager attrs of class node `id`, built on first use.
    fn class_parts(&mut self, id: NodeId) -> PyResult<(Arc<ClassHeader>, Py<PyDict>)> {
        let py = self.py;
        if let Some((header, attrs)) = self.classes.get(&id) {
            return Ok((Arc::clone(header), attrs.clone_ref(py)));
        }
        let MontyNode::ClassType(class) = self.graph.node(id) else {
            unreachable!("the arena's validation makes class_type a ClassType node")
        };
        let header = Arc::new(class_header(class));
        let attrs = self.attrs_dict(&class.attrs)?;
        self.classes.insert(id, (Arc::clone(&header), attrs.clone_ref(py)));
        Ok((header, attrs))
    }

    /// A real Python namedtuple type via `collections.namedtuple`, one per
    /// `(name, fields)` for the arena, with `module=` set so it round-trips
    /// back through the encoder.
    fn namedtuple_type(&mut self, type_name: &str, field_names: &[String]) -> PyResult<Py<PyAny>> {
        let py = self.py;
        let key = (type_name.to_owned(), field_names.to_vec());
        if let Some(nt_type) = self.namedtuple_types.get(&key) {
            return Ok(nt_type.clone_ref(py));
        }
        // split the full type_name (e.g. "os.stat_result") into module + name
        let (module, simple_name) = match type_name.rfind('.') {
            Some(idx) => (&type_name[..idx], &type_name[idx + 1..]),
            None => ("", type_name),
        };
        let namedtuple_fn = get_namedtuple(py)?;
        let py_field_names = PyList::new(py, field_names)?;
        let nt_type = if module.is_empty() {
            namedtuple_fn.call1((simple_name, py_field_names))?
        } else {
            let kwargs = PyDict::new(py);
            kwargs.set_item("module", module)?;
            namedtuple_fn.call((simple_name, py_field_names), Some(&kwargs))?
        }
        .unbind();
        self.namedtuple_types.insert(key, nt_type.clone_ref(py));
        Ok(nt_type)
    }
}

/// The [`own_size`] of every node in `graph`: an unshared key expands to no more.
fn arena_size(graph: &MontyGraph) -> u64 {
    graph
        .nodes()
        .iter()
        .fold(0, |size, node| size.saturating_add(own_size(node)))
}

/// The size of `node` with sharing undone, which bounds what CPython walks to
/// hash it or compare it with another key; `expanded` holds the sizes of the
/// nodes before it. Only tuples and frozensets nest inside hashable keys.
fn expanded_size(node: &MontyNode, expanded: &[u64]) -> u64 {
    let children: &[NodeId] = match node {
        MontyNode::Tuple(items) | MontyNode::FrozenSet(items) => items,
        MontyNode::NamedTuple { values, .. } => values,
        _ => &[],
    };
    children
        .iter()
        .fold(own_size(node), |size, id| size.saturating_add(expanded[id.index()]))
}

/// One for the node plus one per 8 bytes of payload, since CPython hashes and
/// compares a big `int` digit by digit and compares text byte by byte.
fn own_size(node: &MontyNode) -> u64 {
    let payload_bytes = match node {
        MontyNode::BigInt(bi) => bi.bits().div_ceil(8),
        MontyNode::String(s) | MontyNode::Decimal(s) | MontyNode::Path(s) => s.len() as u64,
        MontyNode::Bytes(b) => b.len() as u64,
        _ => 0,
    };
    1 + payload_bytes / 8
}

/// A [`PyMontyStdTypeProxy`] standing for `inner`, as a Python object.
fn std_type_proxy(py: Python<'_>, inner: StdTypeRef) -> PyResult<Py<PyAny>> {
    Ok(Py::new(py, PyMontyStdTypeProxy { inner })?.into_any())
}

/// The class as a proxy records it: the node's header without its attrs,
/// which the proxy keeps as a Python dict.
fn class_header(class: &ClassTypeNode) -> ClassHeader {
    ClassHeader {
        name: class.name.clone(),
        id: class.id,
        host_defined: class.host_defined,
        is_dataclass: class.is_dataclass,
        eq: class.eq.clone(),
        hash: class.hash.clone(),
    }
}
