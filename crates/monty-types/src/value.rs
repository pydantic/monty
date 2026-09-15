//! Values on top of the arena: [`MontyValue`] (an owned arena plus its root),
//! [`ValueRef`] (a borrowed root inside an arena), the message carriers
//! [`CallArgs`] and [`NamedValues`], and the conversions to and from the
//! [`MontyObject`] tree.
//!
//! Hosts build inputs as [`MontyObject`] trees and convert with `.into()`;
//! results are read back with [`ValueRef::into_object`], which expands
//! sharing and is therefore capped by [`ExpandLimits`]. Native converters
//! (the Python and JavaScript bindings) read the arena directly and never expand.

use std::{error::Error, fmt};

use crate::{
    args::PushValue,
    graph::{ClassTypeNode, GraphError, MontyGraph, MontyNode, NodeId},
    object::{DictPairs, MontyClassInstance, MontyClassType, MontyObject, MontyType},
};

/// Default cap on a tree expanded from an arena: 1 GiB of decoded objects.
pub const DEFAULT_EXPAND_BYTES: usize = 1 << 30;
/// Default cap on the nesting depth of an expanded tree; [`MontyObject`]'s
/// drop, equality and serialization are recursive, so depth bounds stack use.
pub const DEFAULT_EXPAND_DEPTH: usize = 200;

/// One owned, self-contained value: an arena plus the id of its root.
///
/// The single-value form carried by `Complete`, resume results, name lookups
/// and `os.getenv` defaults. Converting a [`MontyObject`] with `.into()`
/// yields one; [`MontyValue::into_object`] converts back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MontyValue {
    /// The arena holding the value and everything it references.
    pub graph: MontyGraph,
    /// The value's node.
    pub root: NodeId,
}

impl MontyValue {
    /// Pairs an arena with a root, checking the root is in range.
    pub fn new(graph: MontyGraph, root: NodeId) -> Result<Self, GraphError> {
        graph.check_root(root)?;
        Ok(Self { graph, root })
    }

    /// A value made of one leaf node.
    ///
    /// # Panics
    /// If `node` holds child ids (a leaf never does).
    #[must_use]
    pub fn leaf(node: MontyNode) -> Self {
        let mut graph = MontyGraph::with_capacity(1);
        let root = graph.push(node);
        Self { graph, root }
    }

    /// Borrows the root inside its arena.
    #[must_use]
    pub fn as_ref(&self) -> ValueRef<'_> {
        ValueRef {
            graph: &self.graph,
            id: self.root,
        }
    }

    /// The root node.
    #[must_use]
    pub fn root_node(&self) -> &MontyNode {
        self.graph.node(self.root)
    }

    /// Expands the value into a [`MontyObject`] tree under the default [`ExpandLimits`].
    pub fn into_object(&self) -> Result<MontyObject, ExpandError> {
        self.as_ref().into_object()
    }

    /// Expands the value into a [`MontyObject`] tree under `limits`.
    pub fn into_object_with(&self, limits: ExpandLimits) -> Result<MontyObject, ExpandError> {
        self.as_ref().into_object_with(limits)
    }
}

impl PartialEq<MontyObject> for MontyValue {
    /// Equal when the value expands (under the default limits) to `other`.
    fn eq(&self, other: &MontyObject) -> bool {
        self.into_object().is_ok_and(|object| object == *other)
    }
}

impl PartialEq<MontyValue> for MontyObject {
    fn eq(&self, other: &MontyValue) -> bool {
        other == self
    }
}

impl From<MontyObject> for MontyValue {
    fn from(object: MontyObject) -> Self {
        let mut graph = MontyGraph::new();
        let root = object.push_into(&mut graph);
        Self { graph, root }
    }
}

impl From<MontyNode> for MontyValue {
    fn from(node: MontyNode) -> Self {
        Self::leaf(node)
    }
}

impl fmt::Display for MontyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

/// A borrowed value: one root inside an arena.
///
/// What a message's arguments and inputs expose without copying the arena.
#[derive(Debug, Clone, Copy)]
pub struct ValueRef<'a> {
    /// The arena.
    pub graph: &'a MontyGraph,
    /// The value's node.
    pub id: NodeId,
}

impl<'a> ValueRef<'a> {
    /// The root node.
    #[must_use]
    pub fn node(&self) -> &'a MontyNode {
        self.graph.node(self.id)
    }

    /// The Python type name of the value, e.g. `"list"`.
    #[must_use]
    pub fn type_name(&self) -> &'a str {
        self.graph.type_name(self.id)
    }

    /// Copies the value into its own arena.
    #[must_use]
    pub fn to_owned(&self) -> MontyValue {
        let mut graph = MontyGraph::new();
        let root = self.push_into(&mut graph);
        MontyValue { graph, root }
    }

    /// Expands the value into a [`MontyObject`] tree under the default [`ExpandLimits`].
    pub fn into_object(&self) -> Result<MontyObject, ExpandError> {
        self.into_object_with(ExpandLimits::default())
    }

    /// Expands the value into a [`MontyObject`] tree.
    ///
    /// A sub-object referenced twice becomes two copies, so the result can be
    /// much larger than the arena: `limits` caps its bytes and depth.
    pub fn into_object_with(&self, limits: ExpandLimits) -> Result<MontyObject, ExpandError> {
        let mut expander = Expander {
            graph: self.graph,
            remaining: limits.max_bytes,
            limits,
        };
        expander.expand(self.id, 0)
    }
}

impl fmt::Display for ValueRef<'_> {
    /// The Python `repr()` of the value, or a note when it is too large to expand.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.into_object() {
            Ok(object) => write!(f, "{}", object.py_repr()),
            Err(err) => write!(f, "<value not shown: {err}>"),
        }
    }
}

/// Caps on [`ValueRef::into_object_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpandLimits {
    /// Maximum decoded footprint of the tree, summed over [`MontyNode::host_size`].
    pub max_bytes: usize,
    /// Maximum nesting depth of the tree.
    pub max_depth: usize,
}

impl Default for ExpandLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_EXPAND_BYTES,
            max_depth: DEFAULT_EXPAND_DEPTH,
        }
    }
}

/// Why an arena could not be expanded into a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandError {
    /// The tree would exceed [`ExpandLimits::max_bytes`].
    TooLarge {
        /// The byte cap that was hit.
        limit: usize,
    },
    /// The tree would exceed [`ExpandLimits::max_depth`].
    TooDeep {
        /// The depth cap that was hit.
        limit: usize,
    },
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { limit } => write!(f, "expanded value exceeds {limit} bytes"),
            Self::TooDeep { limit } => write!(f, "expanded value is nested deeper than {limit} levels"),
        }
    }
}

impl Error for ExpandError {}

/// `(positional, keyword)` argument trees, the expanded form of [`CallArgs`].
pub type ArgObjects = (Vec<MontyObject>, Vec<(MontyObject, MontyObject)>);

/// The arguments of one function or OS call: one arena, and the ids of the
/// positional arguments and `(key, value)` keyword pairs in it.
///
/// One arena per call means an object passed twice is sent once and the
/// host receives it as one object, as CPython would.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallArgs {
    /// The arena every argument lives in.
    pub values: MontyGraph,
    /// Positional arguments, in order.
    pub args: Vec<NodeId>,
    /// Keyword arguments as `(key, value)` ids, in order; keys are usually strings.
    pub kwargs: Vec<(NodeId, NodeId)>,
}

impl CallArgs {
    /// No arguments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a positional argument.
    pub fn push_arg(&mut self, value: impl PushValue) -> NodeId {
        let id = value.push_into(&mut self.values);
        self.args.push(id);
        id
    }

    /// Appends a keyword argument with a string key.
    pub fn push_kwarg(&mut self, name: &str, value: impl PushValue) {
        let key = self.values.push(MontyNode::String(name.to_owned()));
        let value = value.push_into(&mut self.values);
        self.kwargs.push((key, value));
    }

    /// The `index`th positional argument.
    #[must_use]
    pub fn arg(&self, index: usize) -> Option<ValueRef<'_>> {
        self.args.get(index).map(|id| self.values.value(*id))
    }

    /// The positional arguments, in order.
    pub fn args(&self) -> impl Iterator<Item = ValueRef<'_>> {
        self.args.iter().map(|id| self.values.value(*id))
    }

    /// The keyword arguments as `(key, value)` views, in order.
    pub fn kwargs(&self) -> impl Iterator<Item = (ValueRef<'_>, ValueRef<'_>)> {
        self.kwargs
            .iter()
            .map(|(key, value)| (self.values.value(*key), self.values.value(*value)))
    }

    /// Checks every argument id is inside the arena; run on decoded messages.
    pub fn check_roots(&self) -> Result<(), GraphError> {
        self.args.iter().try_for_each(|id| self.values.check_root(*id))?;
        self.kwargs.iter().try_for_each(|(key, value)| {
            self.values
                .check_root(*key)
                .and_then(|()| self.values.check_root(*value))
        })
    }

    /// Expands the arguments into `(positional, keyword)` [`MontyObject`] trees.
    pub fn into_objects(&self) -> Result<ArgObjects, ExpandError> {
        let args = self.args().map(|arg| arg.into_object()).collect::<Result<_, _>>()?;
        let kwargs = self
            .kwargs()
            .map(|(key, value)| Ok((key.into_object()?, value.into_object()?)))
            .collect::<Result<_, _>>()?;
        Ok((args, kwargs))
    }
}

/// Positional-only arguments; concrete so an empty `vec![]` infers.
impl From<Vec<MontyObject>> for CallArgs {
    fn from(args: Vec<MontyObject>) -> Self {
        Self::from((args, Vec::new()))
    }
}

impl From<ArgObjects> for CallArgs {
    fn from((args, kwargs): ArgObjects) -> Self {
        let mut call = Self::new();
        for arg in args {
            call.push_arg(arg);
        }
        for (key, value) in kwargs {
            let key = key.push_into(&mut call.values);
            let value = value.push_into(&mut call.values);
            call.kwargs.push((key, value));
        }
        call
    }
}

/// Named values sharing one arena: the inputs of a feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamedValues {
    /// The arena every value lives in.
    pub values: MontyGraph,
    /// `(name, id)` pairs, in order.
    pub names: Vec<(String, NodeId)>,
}

impl NamedValues {
    /// No values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a named value.
    pub fn push(&mut self, name: impl Into<String>, value: impl PushValue) -> NodeId {
        let id = value.push_into(&mut self.values);
        self.names.push((name.into(), id));
        id
    }

    /// Number of named values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether there are no named values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The `(name, value)` pairs, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ValueRef<'_>)> {
        self.names
            .iter()
            .map(|(name, id)| (name.as_str(), self.values.value(*id)))
    }

    /// Checks every id is inside the arena; run on decoded messages.
    pub fn check_roots(&self) -> Result<(), GraphError> {
        self.names.iter().try_for_each(|(_, id)| self.values.check_root(*id))
    }
}

/// Concrete rather than generic over [`PushValue`] so an empty `vec![]` infers.
impl From<Vec<(String, MontyObject)>> for NamedValues {
    fn from(pairs: Vec<(String, MontyObject)>) -> Self {
        let mut named = Self::new();
        for (name, value) in pairs {
            named.push(name, value);
        }
        named
    }
}

impl MontyGraph {
    /// Borrows the value rooted at `id`.
    ///
    /// # Panics
    /// If `id` is out of range; ids come from this arena, so that is a bug.
    #[must_use]
    pub fn value(&self, id: NodeId) -> ValueRef<'_> {
        assert!(id.index() < self.len(), "node id {id} is out of range");
        ValueRef { graph: self, id }
    }
}

impl PushValue for MontyObject {
    /// Post-order push of the tree; a tree cannot share, so no memo is needed.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let node = match self {
            Self::Ellipsis => MontyNode::Ellipsis,
            Self::NotImplemented => MontyNode::NotImplemented,
            Self::None => MontyNode::None,
            Self::Bool(b) => MontyNode::Bool(b),
            Self::Int(i) => MontyNode::Int(i),
            Self::BigInt(bi) => MontyNode::BigInt(bi),
            Self::Float(f) => MontyNode::Float(f),
            Self::String(s) => MontyNode::String(s),
            Self::Bytes(b) => MontyNode::Bytes(b),
            Self::Date(d) => MontyNode::Date(d),
            Self::DateTime(dt) => MontyNode::DateTime(dt),
            Self::Time(t) => MontyNode::Time(t),
            Self::TimeDelta(td) => MontyNode::TimeDelta(td),
            Self::TimeZone(tz) => MontyNode::TimeZone(tz),
            Self::Exception { exc_type, arg } => MontyNode::Exception { exc_type, arg },
            Self::Type(MontyType::Instance(class_type)) => return push_class_type(*class_type, graph),
            Self::Type(t) => MontyNode::Type(t),
            Self::BuiltinFunction(bf) => MontyNode::BuiltinFunction(bf),
            Self::Path(p) => MontyNode::Path(p),
            Self::FileHandle(fh) => MontyNode::FileHandle(fh),
            Self::Function { name, docstring } => MontyNode::Function { name, docstring },
            Self::Repr(s) => MontyNode::Repr(s),
            Self::Cycle(s) => MontyNode::Cycle(s),
            Self::List(items) => MontyNode::List(push_items(items, graph)),
            Self::Tuple(items) => MontyNode::Tuple(push_items(items, graph)),
            Self::Set(items) => MontyNode::Set(push_items(items, graph)),
            Self::FrozenSet(items) => MontyNode::FrozenSet(push_items(items, graph)),
            Self::NamedTuple {
                type_name,
                field_names,
                values,
            } => MontyNode::NamedTuple {
                type_name,
                field_names,
                values: push_items(values, graph),
            },
            Self::Dict(pairs) => MontyNode::Dict(push_pairs(pairs, graph)),
            Self::ClassInstance(instance) => {
                let MontyClassInstance {
                    class_type,
                    instance_id,
                    attrs,
                } = *instance;
                let class_type = push_class_type(class_type, graph);
                MontyNode::ClassInstance {
                    class_type,
                    instance_id,
                    attrs: push_pairs(attrs, graph),
                }
            }
        };
        graph.push(node)
    }
}

impl PushValue for MontyValue {
    /// Merges the value's arena in and returns its rebased root.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let offset = graph.merge(self.graph);
        NodeId(self.root.0 + offset)
    }
}

impl PushValue for ValueRef<'_> {
    /// Copies the reachable nodes; a sub-object shared inside the value stays shared.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let mut copier = Copier {
            source: self.graph,
            target: graph,
            copied: vec![None; self.graph.len()],
        };
        copier.copy(self.id)
    }
}

/// Pushes each item and collects the ids.
fn push_items(items: Vec<MontyObject>, graph: &mut MontyGraph) -> Vec<NodeId> {
    items.into_iter().map(|item| item.push_into(graph)).collect()
}

/// Pushes each key then value and collects the id pairs.
fn push_pairs(pairs: DictPairs, graph: &mut MontyGraph) -> Vec<(NodeId, NodeId)> {
    pairs
        .into_iter()
        .map(|(key, value)| {
            let key = key.push_into(graph);
            let value = value.push_into(graph);
            (key, value)
        })
        .collect()
}

/// Pushes a class's eager attrs then the class-type node itself.
fn push_class_type(class_type: MontyClassType, graph: &mut MontyGraph) -> NodeId {
    let MontyClassType {
        name,
        id,
        host_defined,
        is_dataclass,
        attrs,
    } = class_type;
    let attrs = push_pairs(attrs, graph);
    graph.push(MontyNode::ClassType(Box::new(ClassTypeNode {
        name,
        id,
        host_defined,
        is_dataclass,
        attrs,
    })))
}

/// Copies the nodes reachable from one root of `source` into `target`,
/// memoized so sharing within the value is preserved.
struct Copier<'a> {
    source: &'a MontyGraph,
    target: &'a mut MontyGraph,
    /// Target id of each source node already copied.
    copied: Vec<Option<NodeId>>,
}

impl Copier<'_> {
    fn copy(&mut self, id: NodeId) -> NodeId {
        if let Some(copied) = self.copied[id.index()] {
            return copied;
        }
        // Children first: every child id is lower, so this recursion is
        // bounded by the arena and terminates.
        let mut node = self.source.node(id).clone();
        node.for_each_child_mut(|child| *child = self.copy(*child));
        let target_id = self.target.push(node);
        self.copied[id.index()] = Some(target_id);
        target_id
    }
}

/// Expands one root of an arena into a [`MontyObject`] tree under a byte and depth budget.
struct Expander<'a> {
    graph: &'a MontyGraph,
    limits: ExpandLimits,
    /// Bytes left before [`ExpandError::TooLarge`].
    remaining: usize,
}

impl Expander<'_> {
    fn expand(&mut self, id: NodeId, depth: usize) -> Result<MontyObject, ExpandError> {
        if depth > self.limits.max_depth {
            return Err(ExpandError::TooDeep {
                limit: self.limits.max_depth,
            });
        }
        let node = self.graph.node(id);
        self.remaining = self
            .remaining
            .checked_sub(node.host_size())
            .ok_or(ExpandError::TooLarge {
                limit: self.limits.max_bytes,
            })?;
        let depth = depth + 1;
        Ok(match node {
            MontyNode::Ellipsis => MontyObject::Ellipsis,
            MontyNode::NotImplemented => MontyObject::NotImplemented,
            MontyNode::None => MontyObject::None,
            MontyNode::Bool(b) => MontyObject::Bool(*b),
            MontyNode::Int(i) => MontyObject::Int(*i),
            MontyNode::BigInt(bi) => MontyObject::BigInt(bi.clone()),
            MontyNode::Float(f) => MontyObject::Float(*f),
            MontyNode::String(s) => MontyObject::String(s.clone()),
            MontyNode::Bytes(b) => MontyObject::Bytes(b.clone()),
            MontyNode::Date(d) => MontyObject::Date(d.clone()),
            MontyNode::DateTime(dt) => MontyObject::DateTime(dt.clone()),
            MontyNode::Time(t) => MontyObject::Time(t.clone()),
            MontyNode::TimeDelta(td) => MontyObject::TimeDelta(td.clone()),
            MontyNode::TimeZone(tz) => MontyObject::TimeZone(tz.clone()),
            MontyNode::Exception { exc_type, arg } => MontyObject::Exception {
                exc_type: *exc_type,
                arg: arg.clone(),
            },
            MontyNode::Type(t) => MontyObject::Type(t.clone()),
            MontyNode::BuiltinFunction(bf) => MontyObject::BuiltinFunction(*bf),
            MontyNode::Path(p) => MontyObject::Path(p.clone()),
            MontyNode::FileHandle(fh) => MontyObject::FileHandle(fh.clone()),
            MontyNode::Function { name, docstring } => MontyObject::Function {
                name: name.clone(),
                docstring: docstring.clone(),
            },
            MontyNode::Repr(s) => MontyObject::Repr(s.clone()),
            MontyNode::Cycle(s) => MontyObject::Cycle(s.clone()),
            MontyNode::List(items) => MontyObject::List(self.expand_items(items, depth)?),
            MontyNode::Tuple(items) => MontyObject::Tuple(self.expand_items(items, depth)?),
            MontyNode::Set(items) => MontyObject::Set(self.expand_items(items, depth)?),
            MontyNode::FrozenSet(items) => MontyObject::FrozenSet(self.expand_items(items, depth)?),
            MontyNode::NamedTuple {
                type_name,
                field_names,
                values,
            } => MontyObject::NamedTuple {
                type_name: type_name.clone(),
                field_names: field_names.clone(),
                values: self.expand_items(values, depth)?,
            },
            MontyNode::Dict(pairs) => MontyObject::Dict(self.expand_pairs(pairs, depth)?),
            MontyNode::ClassType(_) => {
                MontyObject::Type(MontyType::Instance(Box::new(self.expand_class_type(id, depth)?)))
            }
            MontyNode::ClassInstance {
                class_type,
                instance_id,
                attrs,
            } => MontyObject::ClassInstance(Box::new(MontyClassInstance {
                class_type: self.expand_class_type(*class_type, depth)?,
                instance_id: *instance_id,
                attrs: self.expand_pairs(attrs, depth)?,
            })),
        })
    }

    fn expand_items(&mut self, ids: &[NodeId], depth: usize) -> Result<Vec<MontyObject>, ExpandError> {
        ids.iter().map(|id| self.expand(*id, depth)).collect()
    }

    fn expand_pairs(&mut self, pairs: &[(NodeId, NodeId)], depth: usize) -> Result<DictPairs, ExpandError> {
        pairs
            .iter()
            .map(|(key, value)| Ok((self.expand(*key, depth)?, self.expand(*value, depth)?)))
            .collect()
    }

    /// Expands a class-type node into the tree's class descriptor. The arena
    /// invariants guarantee `id` is a class-type node.
    fn expand_class_type(&mut self, id: NodeId, depth: usize) -> Result<MontyClassType, ExpandError> {
        let MontyNode::ClassType(class) = self.graph.node(id) else {
            unreachable!("MontyGraph guarantees class_type points at a ClassType node")
        };
        Ok(MontyClassType {
            name: class.name.clone(),
            id: class.id,
            host_defined: class.host_defined,
            is_dataclass: class.is_dataclass,
            attrs: self.expand_pairs(&class.attrs, depth)?,
        })
    }
}
