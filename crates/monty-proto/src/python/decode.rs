//! Sandbox values flowing OUT to the host: [`DecodedArena`] turns one
//! message's node arena into Python objects.

use std::collections::HashMap;

use monty_types::{
    BuiltinsFunctions, MontyException, MontyObject,
    unstable::{self, ClassTypeNode, MontyGraph, MontyNode, NodeId},
};
use pyo3::{
    prelude::*,
    types::{PyBool, PyBytes, PyDate, PyDelta, PyDict, PyFrozenSet, PyList, PySet, PyString, PyTuple},
};

use super::{
    class_instance::{ClassHeader, InstanceStore, PyMontyClassProxy, PyMontyClassTypeProxy},
    convert::{
        PyMontyFileHandle, get_namedtuple, get_pure_posix_path, host_type_object, import_builtins,
        monty_datetime_to_py, monty_time_to_py, monty_timezone_to_py,
    },
    exceptions::exc_monty_to_py,
    std_type_proxy::{PyMontyStdTypeProxy, StdTypeRef},
};

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
        };
        for node in graph.nodes() {
            let obj = decoder.decode(node)?;
            decoder.built.push(obj);
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
                let dict = PyDict::new(py);
                for (key, value) in pairs {
                    dict.set_item(self.child(*key), self.child(*value))?;
                }
                Ok(dict.into_any().unbind())
            }
            MontyNode::Set(items) => {
                let set = PySet::empty(py)?;
                for item in items {
                    set.add(self.child(*item))?;
                }
                Ok(set.into_any().unbind())
            }
            MontyNode::FrozenSet(items) => Ok(PyFrozenSet::new(py, self.children(items))?.into_any().unbind()),
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
            MontyNode::Type(t) => match host_type_object(py, t)? {
                Some(ty) => Ok(ty),
                None => std_type_proxy(py, StdTypeRef::Type(t.clone())),
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
                    let proxy = PyMontyClassTypeProxy {
                        class_type: class_header(class),
                        attributes: self.attrs_dict(&class.attrs)?,
                    };
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
                    let MontyNode::ClassType(class) = self.graph.node(*class_type) else {
                        unreachable!("the arena's validation makes class_type a ClassType node")
                    };
                    let proxy = PyMontyClassProxy {
                        class_type: class_header(class),
                        instance_id: *instance_id,
                        attributes: self.attrs_dict(attrs)?,
                        class_attributes: self.attrs_dict(&class.attrs)?,
                    };
                    Ok(Py::new(py, proxy)?.into_any())
                }
            }
            MontyNode::Path(p) => Ok(get_pure_posix_path(py)?.call1((p,))?.into_any().unbind()),
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
    }
}
