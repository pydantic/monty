//! Host values flowing INTO the sandbox: [`GraphEncoder`] turns Python
//! objects into one post-order node arena per message.

use std::{collections::HashMap, vec::IntoIter};

use monty_types::{ClassTypeNode, MontyDate, MontyException, MontyGraph, MontyNode, MontyUuid, MontyValue, NodeId};
use num_bigint::BigInt;
use pyo3::{
    exceptions::{PyBaseException, PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{
        PyBool, PyBytes, PyDate, PyDateAccess, PyDateTime, PyDelta, PyDict, PyFloat, PyFrozenSet, PyInt, PyList,
        PyModule, PySet, PyString, PyTime, PyTuple, PyType,
    },
};

use super::{
    class_instance::{
        ClassHeader, InstanceStore, PyMontyClassProxy, PyMontyClassTypeProxy, is_class_instance_wrapper,
        is_class_type_wrapper, wrapper_uuid,
    },
    convert::{
        PyMontyFileHandle, get_datetime_timezone_type, get_docstring, get_name, get_pure_posix_path,
        py_datetime_to_monty, py_time_to_monty, py_timedelta_to_monty, py_timezone_to_monty, py_type_object_to_monty,
    },
    exceptions::{exc_py_to_monty, exc_to_monty_node},
};

/// Encodes one host value as its own arena; unsupported types raise `TypeError`.
///
/// The single-value form of [`GraphEncoder`]: a return value, a resumed
/// lookup, an OS-call result. Values that share one message (a feed's inputs)
/// go through one encoder so their sharing survives.
pub fn py_to_monty(obj: &Bound<'_, PyAny>, store: &InstanceStore) -> PyResult<MontyValue> {
    let mut encoder = GraphEncoder::new(obj.py(), store);
    let root = encoder.push(obj)?;
    Ok(encoder.finish_value(root))
}

/// Like [`py_to_monty`], but converts any `PyErr` into a `MontyException`.
///
/// Use this at every boundary where an untrusted host value flows into Monty
/// (inputs, external/OS return values, snapshot resume values). Callers then
/// wrap the `MontyException` as they see fit — `MontyError::new_err(py, e)` for
/// Python-API returns, or `ExtFunctionResult::Error(e)` for mid-execution
/// dispatch — so raw PyO3 errors like `UnicodeEncodeError` never escape.
pub fn py_to_monty_value(obj: &Bound<'_, PyAny>, store: &InstanceStore) -> Result<MontyValue, MontyException> {
    py_to_monty(obj, store).map_err(|e| exc_py_to_monty(obj.py(), &e))
}

/// Builds one message's arena from host values, preserving sharing.
///
/// Every container, wrapper and proxy is memoized by host identity for the
/// life of the encoder, so pushing the same object twice (within one value or
/// across the inputs of a feed) yields the same node id and the sandbox sees
/// one object; leaves are re-encoded per reference. Memoized objects are kept
/// alive so a temporary (an eager-attrs dict, a property result) cannot free
/// its address for a later object to reuse. Nesting is walked on an explicit
/// stack, so depth is bounded by memory, not the native stack. A cycle is
/// rejected with `ValueError`: the arena is post-order, so a value cannot
/// reach itself.
///
/// Class instances cross only when wrapped in `pydantic_monty.ClassInstance`
/// — the wrapper (nested ones inside eager attrs too) registers in the store
/// so method calls, lazy lookups and round-tripped returns resolve to the
/// original object. A bare non-callable instance is a `TypeError`; a callable
/// one converts as a host function like any other callable.
pub struct GraphEncoder<'a, 'py> {
    py: Python<'py>,
    store: &'a InstanceStore,
    graph: MontyGraph,
    /// `id()` of every container seen → its node, or a marker while its
    /// children are still being pushed (a hit on that marker is a cycle).
    memo: HashMap<usize, Slot>,
    /// Pins every memoized object so its `id()` stays unique while encoding.
    keepalive: Vec<Bound<'py, PyAny>>,
    /// Class nodes with no eager attrs, one per class id: every instance of a
    /// class shares its node, as the sandbox's export does.
    class_types: HashMap<MontyUuid, NodeId>,
}

/// A memoized container's state.
enum Slot {
    /// Its children are still being pushed.
    InProgress,
    /// Its node.
    Done(NodeId),
}

impl<'a, 'py> GraphEncoder<'a, 'py> {
    /// An empty arena for one message.
    #[must_use]
    pub fn new(py: Python<'py>, store: &'a InstanceStore) -> Self {
        Self {
            py,
            store,
            graph: MontyGraph::new(),
            memo: HashMap::new(),
            keepalive: Vec::new(),
            class_types: HashMap::new(),
        }
    }

    /// Encodes `obj` into the arena and returns its node; an object pushed
    /// before returns the node it already has.
    pub fn push(&mut self, obj: &Bound<'py, PyAny>) -> PyResult<NodeId> {
        let mut stack: Vec<Frame<'py>> = Vec::new();
        let mut next = Child::Value(obj.clone());
        loop {
            // descend until a leaf, a memo hit, or an empty container
            let mut done = match self.step(next)? {
                Step::Done(id) => id,
                Step::Enter(mut frame) => match frame.children.next() {
                    Some(child) => {
                        stack.push(frame);
                        next = child;
                        continue;
                    }
                    None => self.complete(frame),
                },
            };
            // hand the finished node up, completing every holder it finishes
            loop {
                let Some(mut frame) = stack.pop() else {
                    return Ok(done);
                };
                frame.ids.push(done);
                if let Some(child) = frame.children.next() {
                    stack.push(frame);
                    next = child;
                    break;
                }
                done = self.complete(frame);
            }
        }
    }

    /// The arena, once every root has been pushed.
    #[must_use]
    pub fn finish(self) -> MontyGraph {
        self.graph
    }

    /// The arena as one value rooted at `root`, an id [`push`](Self::push) returned.
    #[must_use]
    pub fn finish_value(self, root: NodeId) -> MontyValue {
        // `push` returned `root`, so it is in range
        MontyValue {
            graph: self.graph,
            root,
        }
    }

    /// Resolves one pending child: a leaf is pushed at once, a container
    /// opens a frame for its children.
    fn step(&mut self, child: Child<'py>) -> PyResult<Step<'py>> {
        match child {
            Child::Value(obj) => self.encode(&obj),
            Child::ClassType(source) => self.enter_class_type(source),
        }
    }

    /// Dispatches on the host type. Order matters: `bool` before `int` (a
    /// subclass), and the generic callable check last since many types
    /// (classes, wrappers) are callable.
    fn encode(&mut self, obj: &Bound<'py, PyAny>) -> PyResult<Step<'py>> {
        let py = self.py;
        if obj.is_none() {
            Ok(self.leaf(MontyNode::None))
        } else if let Ok(bool) = obj.cast::<PyBool>() {
            Ok(self.leaf(MontyNode::Bool(bool.is_true())))
        } else if let Ok(int) = obj.cast::<PyInt>() {
            // i64 first (fast path), BigInt for anything wider
            match int.extract::<i64>() {
                Ok(i) => Ok(self.leaf(MontyNode::Int(i))),
                Err(_) => Ok(self.leaf(MontyNode::BigInt(int.extract::<BigInt>()?))),
            }
        } else if let Ok(float) = obj.cast::<PyFloat>() {
            Ok(self.leaf(MontyNode::Float(float.extract()?)))
        } else if let Ok(string) = obj.cast::<PyString>() {
            Ok(self.leaf(MontyNode::String(string.extract()?)))
        } else if let Ok(bytes) = obj.cast::<PyBytes>() {
            Ok(self.leaf(MontyNode::Bytes(bytes.extract()?)))
        } else if let Ok(list) = obj.cast::<PyList>() {
            self.enter(obj, |_| Ok((Pending::List, values(list.iter()))))
        } else if let Ok(tuple) = obj.cast::<PyTuple>() {
            // a namedtuple (detected by `_fields`) carries its type name
            if let Ok(fields) = obj.getattr(intern!(py, "_fields"))
                && let Ok(fields) = fields.cast::<PyTuple>()
            {
                let type_name = namedtuple_type_name(obj)?;
                let field_names = fields.iter().map(|f| f.extract::<String>()).collect::<PyResult<_>>()?;
                let pending = Pending::NamedTuple { type_name, field_names };
                self.enter(obj, |_| Ok((pending, values(tuple.iter()))))
            } else {
                self.enter(obj, |_| Ok((Pending::Tuple, values(tuple.iter()))))
            }
        } else if let Ok(dict) = obj.cast::<PyDict>() {
            self.enter(obj, |_| Ok((Pending::Dict, pairs(dict))))
        } else if let Ok(set) = obj.cast::<PySet>() {
            self.enter(obj, |_| Ok((Pending::Set, values(set.iter()))))
        } else if let Ok(frozenset) = obj.cast::<PyFrozenSet>() {
            self.enter(obj, |_| Ok((Pending::FrozenSet, values(frozenset.iter()))))
        } else if obj.is(py.Ellipsis()) {
            Ok(self.leaf(MontyNode::Ellipsis))
        } else if obj.is(PyModule::import(py, "builtins")?.getattr("NotImplemented")?) {
            Ok(self.leaf(MontyNode::NotImplemented))
        } else if let Ok(datetime) = obj.cast::<PyDateTime>() {
            Ok(self.leaf(py_datetime_to_monty(datetime)?))
        } else if let Ok(date) = obj.cast::<PyDate>() {
            Ok(self.leaf(MontyNode::Date(MontyDate {
                year: date.get_year(),
                month: date.get_month(),
                day: date.get_day(),
            })))
        } else if let Ok(time) = obj.cast::<PyTime>() {
            Ok(self.leaf(py_time_to_monty(time)?))
        } else if let Ok(delta) = obj.cast::<PyDelta>() {
            Ok(self.leaf(MontyNode::TimeDelta(py_timedelta_to_monty(delta))))
        } else if obj.is_instance(get_datetime_timezone_type(py)?)? {
            Ok(self.leaf(MontyNode::TimeZone(py_timezone_to_monty(obj)?)))
        } else if let Ok(exc) = obj.cast::<PyBaseException>() {
            Ok(self.leaf(exc_to_monty_node(exc)))
        } else if is_class_type_wrapper(obj)? {
            // `ClassType` and `ClassInstance` are sibling `BaseWrapper`s; the
            // class check simply comes first.
            let source = ClassTypeSource::from_wrapper(obj, true)?;
            self.enter_class_type(source)
        } else if is_class_instance_wrapper(obj)? {
            self.enter(obj, |encoder| encoder.begin_instance_wrapper(obj))
        } else if let Ok(proxy) = obj.cast::<PyMontyClassProxy>() {
            // a proxy crosses back with the ids it arrived with, so the
            // sandbox hands over its original object
            let proxy = proxy.get();
            let class = ClassTypeSource {
                class_type: proxy.class_type.clone(),
                attrs: proxy.class_attributes.bind(py).clone(),
                identity: None,
                register: None,
            };
            let mut children = vec![Child::ClassType(class)];
            children.extend(pairs(proxy.attributes.bind(py)));
            let pending = Pending::ClassInstance {
                instance_id: proxy.instance_id,
            };
            self.enter(obj, |_| Ok((pending, children)))
        } else if let Ok(proxy) = obj.cast::<PyMontyClassTypeProxy>() {
            let proxy = proxy.get();
            self.enter_class_type(ClassTypeSource {
                class_type: proxy.class_type.clone(),
                attrs: proxy.attributes.bind(py).clone(),
                identity: Some(obj.clone()),
                register: None,
            })
        } else if obj.is_instance(get_pure_posix_path(py)?)? {
            // pathlib.PurePosixPath and thereby pathlib.PosixPath
            Ok(self.leaf(MontyNode::Path(obj.str()?.extract()?)))
        } else if let Ok(handle) = obj.cast::<PyMontyFileHandle>() {
            // a `MontyFileHandle` returned from Python (e.g. the result of an
            // `Open` OS callback) crosses back as the file it stands for
            Ok(self.leaf(MontyNode::FileHandle(handle.get().inner().clone())))
        } else if let Ok(ty) = obj.cast::<PyType>() {
            // A class is callable, so it would otherwise fall into the generic
            // callable branch. Classes Monty models are preserved as type
            // objects (they round-trip and `isinstance` works in the sandbox);
            // any other host class has no Monty `Type` and crosses as a callable.
            match py_type_object_to_monty(ty)? {
                Some(t) => Ok(self.leaf(MontyNode::Type(t))),
                None => Ok(self.leaf(callable_node(obj))),
            }
        } else if obj.is_callable() {
            Ok(self.leaf(callable_node(obj)))
        } else if let Ok(name) = obj.get_type().qualname() {
            let msg = match obj.get_type().module() {
                Ok(module) => format!(
                    "Cannot convert {module}.{name} to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)"
                ),
                Err(_) => format!("Cannot convert {name} to Monty value"),
            };
            Err(PyTypeError::new_err(msg))
        } else {
            Err(PyTypeError::new_err("Cannot convert unknown type to Monty value"))
        }
    }

    /// Pushes a leaf node.
    fn leaf(&mut self, node: MontyNode) -> Step<'py> {
        Step::Done(self.graph.push(node))
    }

    /// Opens a frame for a container, unless it is memoized: a finished one
    /// returns its node, one still being pushed is a cycle.
    fn enter(
        &mut self,
        obj: &Bound<'py, PyAny>,
        begin: impl FnOnce(&mut Self) -> PyResult<(Pending, Vec<Child<'py>>)>,
    ) -> PyResult<Step<'py>> {
        let key = obj.as_ptr() as usize;
        match self.memo.get(&key) {
            Some(Slot::Done(id)) => Ok(Step::Done(*id)),
            Some(Slot::InProgress) => Err(PyValueError::new_err("Circular reference detected")),
            None => {
                self.memo.insert(key, Slot::InProgress);
                self.keepalive.push(obj.clone());
                let (pending, children) = begin(self)?;
                Ok(Step::Enter(Frame::new(pending, Some(key), children)))
            }
        }
    }

    /// The children of a `ClassInstance` wrapper: its class (registered only
    /// if the class has no entry yet, so an auto-materialized default never
    /// clobbers an explicitly granted policy) then the eager attrs. The
    /// wrapper itself is registered under the instance's uuid.
    fn begin_instance_wrapper(&mut self, wrapper: &Bound<'py, PyAny>) -> PyResult<(Pending, Vec<Child<'py>>)> {
        let py = self.py;
        // `ClassInstance.__post_init__` always materializes a `ClassType`
        // wrapper for the value's class; its `id` (from the name-keyed cache)
        // is the class identity every crossing of this class shares
        let class_wrapper = wrapper.getattr(intern!(py, "class_type"))?;
        let class = ClassTypeSource::from_wrapper(&class_wrapper, false)?;
        let instance_id = wrapper_uuid(wrapper, "ClassInstance")?;
        let eager = wrapper
            .call_method0(intern!(py, "get_eager_attrs"))?
            .cast_into::<PyDict>()?;
        self.store.register(&instance_id, wrapper)?;
        let mut children = Vec::with_capacity(1 + 2 * eager.len());
        children.push(Child::ClassType(class));
        children.extend(pairs(&eager));
        Ok((Pending::ClassInstance { instance_id }, children))
    }

    /// Opens a frame for a class node, or reuses one: the same wrapper or
    /// proxy gives the same node, and a class with no eager attrs has one
    /// node per id however it is spelled. A class met again while its own
    /// attrs are being pushed (a class constant that is an instance of the
    /// class) gets an attr-less duplicate rather than a cycle error, as the
    /// sandbox's export does.
    fn enter_class_type(&mut self, source: ClassTypeSource<'py>) -> PyResult<Step<'py>> {
        if let Some((wrapper, overwrite)) = &source.register {
            if *overwrite {
                self.store.register(&source.class_type.id, wrapper)?;
            } else {
                self.store
                    .register_class_type_if_absent(&source.class_type.id, wrapper)?;
            }
        }
        let key = source.identity.as_ref().map(|obj| obj.as_ptr() as usize);
        if let Some(key) = key {
            match self.memo.get(&key) {
                Some(Slot::Done(id)) => return Ok(Step::Done(*id)),
                Some(Slot::InProgress) => return Ok(self.leaf(source.node(vec![]))),
                None => {
                    self.memo.insert(key, Slot::InProgress);
                    self.keepalive
                        .push(source.identity.clone().expect("key came from the identity"));
                }
            }
        }
        if source.attrs.is_empty()
            && let Some(id) = self.class_types.get(&source.class_type.id)
        {
            let id = *id;
            if let Some(key) = key {
                self.memo.insert(key, Slot::Done(id));
            }
            return Ok(Step::Done(id));
        }
        let children = pairs(&source.attrs);
        Ok(Step::Enter(Frame::new(
            Pending::ClassType(source.class_type),
            key,
            children,
        )))
    }

    /// Builds a container's node once every child id is known.
    fn complete(&mut self, frame: Frame<'py>) -> NodeId {
        let ids = frame.ids;
        let node = match frame.pending {
            Pending::List => MontyNode::List(ids),
            Pending::Tuple => MontyNode::Tuple(ids),
            Pending::Set => MontyNode::Set(ids),
            Pending::FrozenSet => MontyNode::FrozenSet(ids),
            Pending::NamedTuple { type_name, field_names } => MontyNode::NamedTuple {
                type_name,
                field_names,
                values: ids,
            },
            Pending::Dict => MontyNode::Dict(id_pairs(&ids)),
            Pending::ClassType(class_type) => ClassTypeSource::node_from(class_type, id_pairs(&ids)),
            Pending::ClassInstance { instance_id } => MontyNode::ClassInstance {
                class_type: ids[0],
                instance_id,
                attrs: id_pairs(&ids[1..]),
            },
        };
        let attr_less_class = match &node {
            MontyNode::ClassType(class) if class.attrs.is_empty() => Some(class.id),
            _ => None,
        };
        let id = self.graph.push(node);
        if let Some(class_id) = attr_less_class {
            self.class_types.entry(class_id).or_insert(id);
        }
        if let Some(key) = frame.key {
            self.memo.insert(key, Slot::Done(id));
        }
        id
    }
}

/// The outcome of resolving one pending child.
enum Step<'py> {
    /// A leaf, or a container already in the arena.
    Done(NodeId),
    /// A container whose children come next.
    Enter(Frame<'py>),
}

/// A value still to be pushed.
enum Child<'py> {
    Value(Bound<'py, PyAny>),
    /// The class of an instance, or a class crossing as a value.
    ClassType(ClassTypeSource<'py>),
}

/// A container mid-encoding: its children are pushed one at a time on the
/// explicit stack, then the node is built from their ids.
struct Frame<'py> {
    pending: Pending,
    /// The memo key of the host object, if it has one.
    key: Option<usize>,
    /// Children still to push, in order.
    children: IntoIter<Child<'py>>,
    /// Ids of the children pushed so far.
    ids: Vec<NodeId>,
}

impl<'py> Frame<'py> {
    fn new(pending: Pending, key: Option<usize>, children: Vec<Child<'py>>) -> Self {
        let ids = Vec::with_capacity(children.len());
        Self {
            pending,
            key,
            children: children.into_iter(),
            ids,
        }
    }
}

/// What a frame builds once its children are pushed.
enum Pending {
    List,
    Tuple,
    Set,
    FrozenSet,
    NamedTuple {
        type_name: String,
        field_names: Vec<String>,
    },
    /// Children alternate key, value.
    Dict,
    /// Children alternate attr name, value.
    ClassType(ClassHeader),
    /// The first child is the class node, then attr name, value pairs.
    ClassInstance {
        instance_id: MontyUuid,
    },
}

/// Where a class node comes from: a `ClassType` wrapper, a proxy, or the
/// class recorded on an instance proxy.
struct ClassTypeSource<'py> {
    /// The class header.
    class_type: ClassHeader,
    /// Eager class attrs, `name -> value`.
    attrs: Bound<'py, PyDict>,
    /// The host object to memoize by identity, when there is one.
    identity: Option<Bound<'py, PyAny>>,
    /// A wrapper to register in the store, and whether it replaces an
    /// existing entry for its id.
    register: Option<(Bound<'py, PyAny>, bool)>,
}

impl<'py> ClassTypeSource<'py> {
    /// Reads a `pydantic_monty.ClassType` wrapper: name from the class, `id`
    /// from the wrapper (the name-keyed cache makes it stable per class), and
    /// the wrapper's eager class attrs (`get_eager_attrs`).
    fn from_wrapper(wrapper: &Bound<'py, PyAny>, overwrite: bool) -> PyResult<Self> {
        let py = wrapper.py();
        let class = wrapper.getattr(intern!(py, "value"))?;
        let attrs = wrapper
            .call_method0(intern!(py, "get_eager_attrs"))?
            .cast_into::<PyDict>()?;
        Ok(Self {
            class_type: ClassHeader {
                name: class.getattr(intern!(py, "__name__"))?.extract()?,
                id: wrapper_uuid(wrapper, "ClassType")?,
                host_defined: true,
                is_dataclass: wrapper.call_method0(intern!(py, "is_dataclass"))?.extract()?,
            },
            attrs,
            identity: Some(wrapper.clone()),
            register: Some((wrapper.clone(), overwrite)),
        })
    }

    /// The class node with the given attr pairs.
    fn node(&self, attrs: Vec<(NodeId, NodeId)>) -> MontyNode {
        Self::node_from(self.class_type.clone(), attrs)
    }

    fn node_from(class_type: ClassHeader, attrs: Vec<(NodeId, NodeId)>) -> MontyNode {
        MontyNode::ClassType(Box::new(ClassTypeNode {
            name: class_type.name,
            id: class_type.id,
            host_defined: class_type.host_defined,
            is_dataclass: class_type.is_dataclass,
            attrs,
        }))
    }
}

/// The items of a sequence or set as pending children.
fn values<'py>(items: impl Iterator<Item = Bound<'py, PyAny>>) -> Vec<Child<'py>> {
    items.map(Child::Value).collect()
}

/// A dict's entries as pending children, key then value.
fn pairs<'py>(dict: &Bound<'py, PyDict>) -> Vec<Child<'py>> {
    dict.iter()
        .flat_map(|(key, value)| [Child::Value(key), Child::Value(value)])
        .collect()
}

/// Regroups the ids of children pushed key, value, key, value, … into pairs.
fn id_pairs(ids: &[NodeId]) -> Vec<(NodeId, NodeId)> {
    ids.as_chunks::<2>()
        .0
        .iter()
        .map(|&[key, value]| (key, value))
        .collect()
}

/// The full type name of a namedtuple (e.g. `os.stat_result`), dropping the
/// module prefix for built-ins.
fn namedtuple_type_name(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let py_type = obj.get_type();
    let simple_name = py_type.name()?.to_string();
    let module: String = py_type.getattr("__module__")?.extract()?;
    Ok(if module.starts_with('_') || module == "builtins" {
        simple_name
    } else {
        format!("{module}.{simple_name}")
    })
}

/// A host callable with no richer Monty mapping, carrying its `__name__` and
/// docstring: plain callables and host classes Monty does not model.
fn callable_node(obj: &Bound<'_, PyAny>) -> MontyNode {
    MontyNode::Function {
        name: get_name(obj),
        docstring: get_docstring(obj),
    }
}
