//! [`MontyGraph`], the flat node arena Python values cross the sandbox
//! boundary in, and [`MontyNode`], one entry of it.
//!
//! A tree would copy a shared sub-object once per reference, so a small heap
//! graph could become an exponentially larger message. The arena keeps the
//! heap's shape: a container holds the ids of its children, a sub-object
//! referenced twice is one node referenced twice, and a message carries one
//! arena plus the ids of its roots ([`MontyObject`](crate::MontyObject),
//! [`CallArgs`](crate::CallArgs)).
//!
//! Nodes are in post-order: every child id is lower than the id of the node
//! holding it, so a decoder builds values in one forward pass without
//! recursion and no arena can contain a cycle. A reference back to an
//! enclosing container is instead a [`MontyNode::Cycle`] leaf holding the
//! placeholder its repr shows.

use std::{error::Error, fmt, mem::size_of};

use num_bigint::BigInt;

use crate::{
    builtins::BuiltinsFunctions,
    exceptions::ExcType,
    object::{MontyDate, MontyDateTime, MontyFileHandle, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType},
    uuid::MontyUuid,
};

/// Index of a node in a [`MontyGraph`].
///
/// Only meaningful together with the arena it was issued by: ids are dense
/// positions, not identities, and [`MontyGraph::merge`] rebases them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The id as a vector index.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One entry of a [`MontyGraph`]: a leaf value, or a container holding the
/// ids of its children.
///
/// A non-builtin class is its own [`ClassType`](Self::ClassType) node, shared
/// by every instance of it. Build values with the
/// [`MontyObject`](crate::MontyObject) constructors rather than from nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MontyNode {
    /// Python's `Ellipsis` singleton (`...`).
    Ellipsis,
    /// Python's `NotImplemented` singleton.
    NotImplemented,
    /// Python's `None` singleton.
    None,
    /// Python boolean.
    Bool(bool),
    /// Python integer fitting in 64 bits.
    Int(i64),
    /// Python integer wider than 64 bits.
    BigInt(BigInt),
    /// Python float.
    Float(f64),
    /// Python string.
    String(String),
    /// Python bytes.
    Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Python `datetime.date`.
    Date(MontyDate),
    /// Python `datetime.datetime`.
    DateTime(MontyDateTime),
    /// Python `datetime.time`.
    Time(MontyTime),
    /// Python `datetime.timedelta`.
    TimeDelta(MontyTimeDelta),
    /// Python `datetime.timezone`.
    TimeZone(MontyTimeZone),
    /// A Python exception value with type and optional message.
    Exception {
        /// The exception type.
        exc_type: ExcType,
        /// Optional string argument passed to the exception constructor.
        arg: Option<String>,
    },
    /// A builtin type object; a non-builtin class is a [`ClassType`](Self::ClassType) node.
    Type(MontyType),
    /// A builtin function such as `len`.
    BuiltinFunction(BuiltinsFunctions),
    /// A `pathlib.Path` (always a virtual POSIX path).
    Path(String),
    /// An open file object.
    FileHandle(MontyFileHandle),
    /// An external function provided by the host.
    Function {
        /// The function name.
        name: String,
        /// Optional docstring.
        docstring: Option<String>,
    },
    /// Output-only fallback: the `repr()` of a value with no other representation.
    Repr(String),
    /// Output-only: a reference back to a container that encloses this node,
    /// as the placeholder its repr shows (`[...]`, `(...)`, `{...}` or `...`).
    Cycle(String),
    /// Python list: ids of its items.
    List(Vec<NodeId>),
    /// Python tuple: ids of its items.
    Tuple(Vec<NodeId>),
    /// Python set: ids of its elements.
    Set(Vec<NodeId>),
    /// Python frozenset: ids of its elements.
    FrozenSet(Vec<NodeId>),
    /// Python named tuple: field names and the ids of the values.
    NamedTuple {
        /// Type name used in repr, e.g. `os.stat_result`.
        type_name: String,
        /// Attribute names, one per value.
        field_names: Vec<String>,
        /// Ids of the values, in field order.
        values: Vec<NodeId>,
    },
    /// Python dict: `(key, value)` id pairs in insertion order.
    Dict(Vec<(NodeId, NodeId)>),
    /// A sandbox- or host-defined class, shared by every instance of it.
    /// Boxed so the variant does not widen the node.
    ClassType(Box<ClassTypeNode>),
    /// An instance of a non-builtin class.
    ClassInstance {
        /// Id of the instance's [`ClassType`](Self::ClassType) node.
        class_type: NodeId,
        /// Identity of the instance, generated by whichever side defined it.
        instance_id: MontyUuid,
        /// Eagerly-sent attributes as `(name, value)` id pairs.
        attrs: Vec<(NodeId, NodeId)>,
    },
}

impl MontyNode {
    /// Calls `f` with every child id this node holds, in order; for
    /// [`ClassInstance`](Self::ClassInstance) the class id comes first.
    pub fn for_each_child(&self, mut f: impl FnMut(NodeId)) {
        match self {
            Self::List(ids) | Self::Tuple(ids) | Self::Set(ids) | Self::FrozenSet(ids) => {
                ids.iter().copied().for_each(f);
            }
            Self::NamedTuple { values, .. } => values.iter().copied().for_each(f),
            Self::Dict(pairs) => pairs.iter().for_each(|(key, value)| {
                f(*key);
                f(*value);
            }),
            Self::ClassType(class) => class.attrs.iter().for_each(|(key, value)| {
                f(*key);
                f(*value);
            }),
            Self::ClassInstance { class_type, attrs, .. } => {
                f(*class_type);
                for (key, value) in attrs {
                    f(*key);
                    f(*value);
                }
            }
            _ => {}
        }
    }

    /// Mutable counterpart of [`for_each_child`](Self::for_each_child), used to
    /// rebase ids when arenas are merged.
    pub fn for_each_child_mut(&mut self, mut f: impl FnMut(&mut NodeId)) {
        match self {
            Self::List(ids) | Self::Tuple(ids) | Self::Set(ids) | Self::FrozenSet(ids) => ids.iter_mut().for_each(f),
            Self::NamedTuple { values, .. } => values.iter_mut().for_each(f),
            Self::Dict(pairs) => pairs.iter_mut().for_each(|(key, value)| {
                f(key);
                f(value);
            }),
            Self::ClassType(class) => class.attrs.iter_mut().for_each(|(key, value)| {
                f(key);
                f(value);
            }),
            Self::ClassInstance { class_type, attrs, .. } => {
                f(class_type);
                for (key, value) in attrs {
                    f(key);
                    f(value);
                }
            }
            _ => {}
        }
    }

    /// The placeholder a reference back to this node renders as, matching
    /// CPython's recursive `repr()` markers.
    #[must_use]
    pub fn cycle_placeholder(&self) -> &'static str {
        match self {
            Self::List(_) => "[...]",
            Self::Tuple(_) | Self::NamedTuple { .. } => "(...)",
            Self::Dict(_) => "{...}",
            _ => "...",
        }
    }

    /// Whether the node holds no child ids.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        let mut leaf = true;
        self.for_each_child(|_| leaf = false);
        leaf
    }

    /// The host footprint of one owned string in a metadata vector, such as
    /// a namedtuple's field names.
    #[must_use]
    pub const fn metadata_string_size(value: &str) -> usize {
        size_of::<String>().saturating_add(value.len())
    }

    /// Host footprint of this node once decoded: the fixed enum size plus the
    /// bytes it owns directly (string, bytes and bigint payloads, field
    /// names, and its child-id vectors). Children charge themselves, so an
    /// arena's footprint is the plain sum over its nodes.
    #[must_use]
    pub fn decoded_size(&self) -> usize {
        let name_len = |name: &Option<String>| -> usize { name.as_ref().map_or(0, String::len) };
        let ids = |ids: &[NodeId]| ids.len().saturating_mul(size_of::<NodeId>());
        let pairs = |pairs: &[(NodeId, NodeId)]| pairs.len().saturating_mul(2 * size_of::<NodeId>());

        let payload = match self {
            Self::String(s) | Self::Path(s) | Self::Repr(s) | Self::Cycle(s) => s.len(),
            Self::Bytes(b) => b.len(),
            // Saturate rather than truncate on a 32-bit `usize`: an over-large
            // estimate only trips a budget sooner, which is the safe direction.
            Self::BigInt(bi) => usize::try_from(bi.bits().div_ceil(8)).unwrap_or(usize::MAX),
            Self::Exception { arg, .. } => name_len(arg),
            Self::FileHandle(fh) => fh.path.len(),
            Self::Function { name, docstring } => name.len().saturating_add(name_len(docstring)),
            Self::DateTime(dt) => name_len(&dt.timezone_name),
            Self::Time(t) => name_len(&t.timezone_name),
            Self::TimeZone(tz) => name_len(&tz.name),
            Self::List(items) | Self::Tuple(items) | Self::Set(items) | Self::FrozenSet(items) => ids(items),
            Self::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                let names: usize = field_names.iter().map(|name| Self::metadata_string_size(name)).sum();
                type_name.len().saturating_add(names).saturating_add(ids(values))
            }
            Self::Dict(entries) => pairs(entries),
            // The boxed payload is outside `size_of::<Self>()`, so charge it too.
            Self::ClassType(class) => size_of::<ClassTypeNode>()
                .saturating_add(class.name.len())
                .saturating_add(pairs(&class.attrs)),
            Self::ClassInstance { attrs, .. } => pairs(attrs),
            _ => 0,
        };
        size_of::<Self>().saturating_add(payload)
    }
}

/// Payload of [`MontyNode::ClassType`]: a class shared by every instance of
/// it in the arena.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClassTypeNode {
    /// The Python-visible class name.
    pub name: String,
    /// Identity of the class, generated by whichever side defined it.
    pub id: MontyUuid,
    /// True for a host-defined class, false for a sandbox-defined one.
    pub host_defined: bool,
    /// Whether `dataclasses.is_dataclass` is true for the class.
    pub is_dataclass: bool,
    /// Eagerly-sent class attributes as `(name, value)` id pairs.
    pub attrs: Vec<(NodeId, NodeId)>,
}

impl PartialEq for MontyNode {
    /// Per-node equality (child ids compare as ids); floats compare
    /// bit-for-bit so `NaN` round-trips equal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ellipsis, Self::Ellipsis)
            | (Self::NotImplemented, Self::NotImplemented)
            | (Self::None, Self::None) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::BigInt(a), Self::BigInt(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b))
            | (Self::Path(a), Self::Path(b))
            | (Self::Repr(a), Self::Repr(b))
            | (Self::Cycle(a), Self::Cycle(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            (Self::Time(a), Self::Time(b)) => a == b,
            (Self::TimeDelta(a), Self::TimeDelta(b)) => a == b,
            (Self::TimeZone(a), Self::TimeZone(b)) => a == b,
            (
                Self::Exception {
                    exc_type: a_type,
                    arg: a_arg,
                },
                Self::Exception {
                    exc_type: b_type,
                    arg: b_arg,
                },
            ) => a_type == b_type && a_arg == b_arg,
            (Self::Type(a), Self::Type(b)) => a == b,
            (Self::BuiltinFunction(a), Self::BuiltinFunction(b)) => a == b,
            (Self::FileHandle(a), Self::FileHandle(b)) => {
                a.path == b.path && a.mode == b.mode && a.position == b.position
            }
            (
                Self::Function {
                    name: a_name,
                    docstring: a_doc,
                },
                Self::Function {
                    name: b_name,
                    docstring: b_doc,
                },
            ) => a_name == b_name && a_doc == b_doc,
            (Self::List(a), Self::List(b))
            | (Self::Tuple(a), Self::Tuple(b))
            | (Self::Set(a), Self::Set(b))
            | (Self::FrozenSet(a), Self::FrozenSet(b)) => a == b,
            (
                Self::NamedTuple {
                    type_name: a_name,
                    field_names: a_fields,
                    values: a_values,
                },
                Self::NamedTuple {
                    type_name: b_name,
                    field_names: b_fields,
                    values: b_values,
                },
            ) => a_name == b_name && a_fields == b_fields && a_values == b_values,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            (Self::ClassType(a), Self::ClassType(b)) => a == b,
            (
                Self::ClassInstance {
                    class_type: a_class,
                    instance_id: a_id,
                    attrs: a_attrs,
                },
                Self::ClassInstance {
                    class_type: b_class,
                    instance_id: b_id,
                    attrs: b_attrs,
                },
            ) => a_class == b_class && a_id == b_id && a_attrs == b_attrs,
            _ => false,
        }
    }
}

impl Eq for MontyNode {}

/// A post-order node arena.
///
/// [`push`](Self::push) and [`from_nodes`](Self::from_nodes) check the
/// invariants (see [`validate`](Self::validate)) and [`merge`](Self::merge)
/// preserves them, so readers index without re-checking. Roots are held by
/// the carrying message, so one arena serves every value in that message.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MontyGraph {
    nodes: Vec<MontyNode>,
}

impl MontyGraph {
    /// An empty arena.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty arena with room for `capacity` nodes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
        }
    }

    /// Adopts already-built nodes after validating them.
    pub fn from_nodes(nodes: Vec<MontyNode>) -> Result<Self, GraphError> {
        Self::validate(&nodes)?;
        Ok(Self { nodes })
    }

    /// Checks that child ids precede their holders and class-instance nodes
    /// reference class-type nodes.
    pub fn validate(nodes: &[MontyNode]) -> Result<(), GraphError> {
        nodes
            .iter()
            .enumerate()
            .try_for_each(|(index, node)| Self::check_node(nodes, index, node))
    }

    /// Appends a node, returning its id.
    ///
    /// # Panics
    /// If the node violates the arena invariants; children must be pushed
    /// before the node that holds them.
    pub fn push(&mut self, node: MontyNode) -> NodeId {
        let index = self.nodes.len();
        if let Err(err) = Self::check_node(&self.nodes, index, &node) {
            panic!("invalid node pushed onto a MontyGraph: {err}");
        }
        self.nodes.push(node);
        Self::id_at(index)
    }

    /// Appends every node of `other`, rebasing its ids, and returns the
    /// offset added to them so the caller can rebase its own roots.
    pub fn merge(&mut self, other: Self) -> u32 {
        let offset = Self::id_at(self.nodes.len()).0;
        self.nodes.reserve(other.nodes.len());
        for mut node in other.nodes {
            node.for_each_child_mut(|id| id.0 += offset);
            self.nodes.push(node);
        }
        offset
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the arena has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node at `id`.
    ///
    /// # Panics
    /// If `id` is out of range; ids come from this arena, so that is a bug.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &MontyNode {
        &self.nodes[id.index()]
    }

    /// Mutable access to the node at `id`. Callers must keep the invariants:
    /// replacing a node's child ids with lower ones is fine, raising them is not.
    ///
    /// # Panics
    /// If `id` is out of range.
    pub fn node_mut(&mut self, id: NodeId) -> &mut MontyNode {
        &mut self.nodes[id.index()]
    }

    /// Every node, in post-order, for editing payloads in place; reordering
    /// nodes or raising a child id breaks the arena's invariants.
    pub fn nodes_mut(&mut self) -> &mut [MontyNode] {
        &mut self.nodes
    }

    /// Every node, in post-order.
    #[must_use]
    pub fn nodes(&self) -> &[MontyNode] {
        &self.nodes
    }

    /// Takes the nodes out of the arena.
    #[must_use]
    pub fn into_nodes(self) -> Vec<MontyNode> {
        self.nodes
    }

    /// Checks that `root` (an id received alongside this arena) is in range.
    pub fn check_root(&self, root: NodeId) -> Result<(), GraphError> {
        if root.index() < self.nodes.len() {
            Ok(())
        } else {
            Err(GraphError::RootOutOfRange {
                root,
                len: self.nodes.len(),
            })
        }
    }

    /// The Python type name of the value at `id`, e.g. `"list"`; a class
    /// instance reports its class name.
    #[must_use]
    pub fn type_name(&self, id: NodeId) -> &str {
        match self.node(id) {
            MontyNode::None => "NoneType",
            MontyNode::Ellipsis => "ellipsis",
            MontyNode::NotImplemented => "NotImplementedType",
            MontyNode::Bool(_) => "bool",
            MontyNode::Int(_) | MontyNode::BigInt(_) => "int",
            MontyNode::Float(_) => "float",
            MontyNode::String(_) => "str",
            MontyNode::Bytes(_) => "bytes",
            MontyNode::List(_) => "list",
            MontyNode::Tuple(_) => "tuple",
            MontyNode::NamedTuple { .. } => "namedtuple",
            MontyNode::Dict(_) => "dict",
            MontyNode::Set(_) => "set",
            MontyNode::FrozenSet(_) => "frozenset",
            MontyNode::Date(_) => "date",
            MontyNode::DateTime(_) => "datetime",
            MontyNode::Time(_) => "time",
            MontyNode::TimeDelta(_) => "timedelta",
            MontyNode::TimeZone(_) => "timezone",
            MontyNode::Exception { .. } => "Exception",
            MontyNode::Path(_) => "PosixPath",
            MontyNode::FileHandle(handle) => handle.mode.type_name(),
            MontyNode::ClassInstance { class_type, .. } => match self.node(*class_type) {
                MontyNode::ClassType(class) => &class.name,
                _ => "object",
            },
            MontyNode::Type(_) | MontyNode::ClassType(_) => "type",
            MontyNode::BuiltinFunction(_) => "builtin_function_or_method",
            MontyNode::Function { .. } => "function",
            MontyNode::Repr(_) => "repr",
            MontyNode::Cycle(_) => "cycle",
        }
    }

    /// Sum of [`MontyNode::decoded_size`] over every node: the arena's decoded
    /// footprint, used by transport budgets.
    #[must_use]
    pub fn decoded_size(&self) -> usize {
        self.nodes
            .iter()
            .fold(0usize, |size, node| size.saturating_add(node.decoded_size()))
    }

    /// Validates one node against the nodes that precede it.
    fn check_node(nodes: &[MontyNode], index: usize, node: &MontyNode) -> Result<(), GraphError> {
        let holder = Self::id_at(index);
        let mut result = Ok(());
        node.for_each_child(|child| {
            if result.is_ok() && child.index() >= index {
                result = Err(GraphError::IndexNotLower { node: holder, child });
            }
        });
        result?;
        match node {
            MontyNode::ClassInstance { class_type, .. }
                if !matches!(nodes[class_type.index()], MontyNode::ClassType(_)) =>
            {
                Err(GraphError::ClassTypeNotAClass { node: holder })
            }
            _ => Ok(()),
        }
    }

    /// The id of the node at `index`; panics past `u32::MAX` nodes.
    fn id_at(index: usize) -> NodeId {
        NodeId(u32::try_from(index).expect("MontyGraph exceeds u32::MAX nodes"))
    }
}

/// Why a node sequence is not a valid [`MontyGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// A node references a child at or above its own index.
    IndexNotLower {
        /// The holding node.
        node: NodeId,
        /// The offending child id.
        child: NodeId,
    },
    /// A class-instance node's `class_type` is not a class-type node.
    ClassTypeNotAClass {
        /// The instance node.
        node: NodeId,
    },
    /// A root id is outside the arena.
    RootOutOfRange {
        /// The root id.
        root: NodeId,
        /// The arena length.
        len: usize,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexNotLower { node, child } => {
                write!(f, "value node {node} references node {child}, which is not below it")
            }
            Self::ClassTypeNotAClass { node } => write!(f, "class instance node {node} does not point at a class type"),
            Self::RootOutOfRange { root, len } => {
                write!(f, "value root {root} is out of range for an arena of {len} nodes")
            }
        }
    }
}

impl Error for GraphError {}
