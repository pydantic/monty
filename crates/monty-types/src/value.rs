//! Values on top of the arena: [`MontyValue`] (an owned arena plus its root),
//! [`ValueRef`] (a borrowed root inside an arena), and the message carriers
//! [`CallArgs`] and [`NamedValues`].
//!
//! Hosts build inputs with the [`MontyValue`] constructors (`MontyValue::int`,
//! `MontyValue::list`, ...) and read results through [`ValueRef`]: the root
//! node, typed accessors, structural equality and the Python `repr()`. Nothing
//! here expands sharing, so every operation is linear in the arena.

use std::{
    collections::HashSet,
    fmt::{self, Write},
};

use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use crate::{
    args::PushValue,
    builtins::BuiltinsFunctions,
    exceptions::ExcType,
    format::{FormatFloat, StringRepr, bytes_repr_fmt, format_offset_timedelta_repr, string_repr_fmt},
    graph::{ClassTypeNode, GraphError, MontyGraph, MontyNode, NodeId},
    object::{
        ConversionError, MontyDate, MontyDateTime, MontyFileHandle, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType,
    },
    uuid::MontyUuid,
};

/// One owned, self-contained value: an arena plus the id of its root.
///
/// The single-value form carried by `Complete`, resume results, name lookups
/// and `os.getenv` defaults, and the value hosts construct inputs with. Two
/// values are equal when they are structurally equal as Python values,
/// whatever the layout of their arenas.
#[derive(Debug, Clone, Eq, serde::Serialize, serde::Deserialize)]
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

    /// Python `None`.
    #[must_use]
    pub fn none() -> Self {
        Self::leaf(MontyNode::None)
    }

    /// Python `Ellipsis`.
    #[must_use]
    pub fn ellipsis() -> Self {
        Self::leaf(MontyNode::Ellipsis)
    }

    /// Python `NotImplemented`.
    #[must_use]
    pub fn not_implemented() -> Self {
        Self::leaf(MontyNode::NotImplemented)
    }

    /// A `bool`.
    #[must_use]
    pub fn bool(value: bool) -> Self {
        Self::leaf(MontyNode::Bool(value))
    }

    /// An `int` that fits in 64 bits.
    #[must_use]
    pub fn int(value: i64) -> Self {
        Self::leaf(MontyNode::Int(value))
    }

    /// An `int` of any size.
    #[must_use]
    pub fn bigint(value: BigInt) -> Self {
        Self::leaf(MontyNode::BigInt(value))
    }

    /// A `float`.
    #[must_use]
    pub fn float(value: f64) -> Self {
        Self::leaf(MontyNode::Float(value))
    }

    /// A `str`.
    #[must_use]
    pub fn string(value: impl Into<String>) -> Self {
        Self::leaf(MontyNode::String(value.into()))
    }

    /// A `bytes`.
    #[must_use]
    pub fn bytes(value: impl Into<Vec<u8>>) -> Self {
        Self::leaf(MontyNode::Bytes(value.into()))
    }

    /// A `pathlib.Path`, always a virtual POSIX path.
    #[must_use]
    pub fn path(value: impl Into<String>) -> Self {
        Self::leaf(MontyNode::Path(value.into()))
    }

    /// A `datetime.date`.
    #[must_use]
    pub fn date(value: MontyDate) -> Self {
        Self::leaf(MontyNode::Date(value))
    }

    /// A `datetime.datetime`.
    #[must_use]
    pub fn datetime(value: MontyDateTime) -> Self {
        Self::leaf(MontyNode::DateTime(value))
    }

    /// A `datetime.time`.
    #[must_use]
    pub fn time(value: MontyTime) -> Self {
        Self::leaf(MontyNode::Time(value))
    }

    /// A `datetime.timedelta`.
    #[must_use]
    pub fn timedelta(value: MontyTimeDelta) -> Self {
        Self::leaf(MontyNode::TimeDelta(value))
    }

    /// A `datetime.timezone`.
    #[must_use]
    pub fn timezone(value: MontyTimeZone) -> Self {
        Self::leaf(MontyNode::TimeZone(value))
    }

    /// An exception instance as a value (not raised), with its message.
    #[must_use]
    pub fn exception(exc_type: ExcType, arg: Option<String>) -> Self {
        Self::leaf(MontyNode::Exception { exc_type, arg })
    }

    /// A host function the sandbox calls back by `name`.
    #[must_use]
    pub fn function(name: impl Into<String>, docstring: Option<String>) -> Self {
        Self::leaf(MontyNode::Function {
            name: name.into(),
            docstring,
        })
    }

    /// A builtin function such as `len`.
    #[must_use]
    pub fn builtin_function(function: BuiltinsFunctions) -> Self {
        Self::leaf(MontyNode::BuiltinFunction(function))
    }

    /// A builtin type object such as `int`.
    #[must_use]
    pub fn type_object(value: MontyType) -> Self {
        Self::leaf(MontyNode::Type(value))
    }

    /// An open file object, as the result of an `open()` OS call.
    #[must_use]
    pub fn file_handle(value: MontyFileHandle) -> Self {
        Self::leaf(MontyNode::FileHandle(value))
    }

    /// Output-only: a value's `repr()` where no faithful representation exists.
    #[must_use]
    pub fn repr(value: impl Into<String>) -> Self {
        Self::leaf(MontyNode::Repr(value.into()))
    }

    /// Output-only: a reference back to an enclosing container, as its placeholder.
    #[must_use]
    pub fn cycle(placeholder: impl Into<String>) -> Self {
        Self::leaf(MontyNode::Cycle(placeholder.into()))
    }

    /// A `list`.
    #[must_use]
    pub fn list(items: impl IntoIterator<Item = Self>) -> Self {
        Self::container(items, MontyNode::List)
    }

    /// A `tuple`.
    #[must_use]
    pub fn tuple(items: impl IntoIterator<Item = Self>) -> Self {
        Self::container(items, MontyNode::Tuple)
    }

    /// A `set`.
    #[must_use]
    pub fn set(items: impl IntoIterator<Item = Self>) -> Self {
        Self::container(items, MontyNode::Set)
    }

    /// A `frozenset`.
    #[must_use]
    pub fn frozenset(items: impl IntoIterator<Item = Self>) -> Self {
        Self::container(items, MontyNode::FrozenSet)
    }

    /// A `dict` from `(key, value)` pairs, in insertion order.
    #[must_use]
    pub fn dict(pairs: impl IntoIterator<Item = (Self, Self)>) -> Self {
        let mut graph = MontyGraph::new();
        let pairs = push_pairs(pairs, &mut graph);
        let root = graph.push(MontyNode::Dict(pairs));
        Self { graph, root }
    }

    /// A namedtuple: `type_name(field=value, ...)`.
    #[must_use]
    pub fn named_tuple(
        type_name: impl Into<String>,
        field_names: impl IntoIterator<Item = impl Into<String>>,
        values: impl IntoIterator<Item = Self>,
    ) -> Self {
        let type_name = type_name.into();
        let field_names = field_names.into_iter().map(Into::into).collect();
        Self::container(values, |values| MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        })
    }

    /// A non-builtin class type object with its eager class attrs.
    ///
    /// `id` is generated by whichever side defined the class (a host uuid4,
    /// or a worker uuid for sandbox classes); the sandbox keeps one type
    /// object per id and routes instantiation and classmethod calls by it.
    #[must_use]
    pub fn class_type(
        name: impl Into<String>,
        id: MontyUuid,
        host_defined: bool,
        is_dataclass: bool,
        attrs: impl IntoIterator<Item = (Self, Self)>,
    ) -> Self {
        let mut graph = MontyGraph::new();
        let attrs = push_pairs(attrs, &mut graph);
        let root = graph.push(MontyNode::ClassType(Box::new(ClassTypeNode {
            name: name.into(),
            id,
            host_defined,
            is_dataclass,
            attrs,
        })));
        Self { graph, root }
    }

    /// An instance of `class_type` (a [`class_type`](Self::class_type) value)
    /// with its eager attrs, identified by `instance_id`.
    ///
    /// # Panics
    /// If `class_type` is not a class type object.
    #[must_use]
    pub fn class_instance(
        class_type: Self,
        instance_id: MontyUuid,
        attrs: impl IntoIterator<Item = (Self, Self)>,
    ) -> Self {
        let mut graph = MontyGraph::new();
        let class_type = class_type.push_into(&mut graph);
        let attrs = push_pairs(attrs, &mut graph);
        let root = graph.push(MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        });
        Self { graph, root }
    }

    /// Resolves a builtin function by its Python name (e.g. `"len"`), the
    /// name its `Display` renders.
    #[must_use]
    pub fn builtin_function_from_name(name: &str) -> Option<Self> {
        name.parse::<BuiltinsFunctions>().ok().map(Self::builtin_function)
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

    /// The Python `repr()` of the value.
    #[must_use]
    pub fn py_repr(&self) -> String {
        self.as_ref().py_repr()
    }

    /// Whether the value is truthy under Python's rules; see [`ValueRef::is_truthy`].
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        self.as_ref().is_truthy()
    }

    /// The Python type name of the value, e.g. `"list"`.
    #[must_use]
    pub fn type_name(&self) -> &str {
        self.graph.type_name(self.root)
    }

    /// Pushes every item, then the container node holding their ids.
    fn container(items: impl IntoIterator<Item = Self>, make: impl FnOnce(Vec<NodeId>) -> MontyNode) -> Self {
        let mut graph = MontyGraph::new();
        let ids = items.into_iter().map(|item| item.push_into(&mut graph)).collect();
        let root = graph.push(make(ids));
        Self { graph, root }
    }
}

impl PartialEq for MontyValue {
    /// Structural equality as Python values, independent of arena layout.
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl PartialEq<ValueRef<'_>> for MontyValue {
    fn eq(&self, other: &ValueRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

impl From<MontyNode> for MontyValue {
    fn from(node: MontyNode) -> Self {
        Self::leaf(node)
    }
}

impl fmt::Display for MontyValue {
    /// The Python `str()` of the value: text as is, everything else its `repr()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl TryFrom<&MontyValue> for i64 {
    type Error = ConversionError;

    fn try_from(value: &MontyValue) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyValue> for f64 {
    type Error = ConversionError;

    fn try_from(value: &MontyValue) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyValue> for String {
    type Error = ConversionError;

    fn try_from(value: &MontyValue) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyValue> for bool {
    type Error = ConversionError;

    fn try_from(value: &MontyValue) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
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

    /// The child at `id` of the same arena.
    #[must_use]
    pub fn child(&self, id: NodeId) -> Self {
        self.graph.value(id)
    }

    /// The items of a list, tuple, set, frozenset or namedtuple; `None` for
    /// any other value.
    #[must_use]
    pub fn items(&self) -> Option<Vec<Self>> {
        match self.node() {
            MontyNode::List(ids)
            | MontyNode::Tuple(ids)
            | MontyNode::Set(ids)
            | MontyNode::FrozenSet(ids)
            | MontyNode::NamedTuple { values: ids, .. } => Some(ids.iter().map(|id| self.child(*id)).collect()),
            _ => None,
        }
    }

    /// The `(key, value)` pairs of a dict, or the eager attrs of a class
    /// instance or class type object; `None` for any other value.
    #[must_use]
    pub fn pairs(&self) -> Option<Vec<(Self, Self)>> {
        let pairs = match self.node() {
            MontyNode::Dict(pairs) | MontyNode::ClassInstance { attrs: pairs, .. } => pairs,
            MontyNode::ClassType(class) => &class.attrs,
            _ => return None,
        };
        Some(
            pairs
                .iter()
                .map(|(key, value)| (self.child(*key), self.child(*value)))
                .collect(),
        )
    }

    /// The value as an `int`, if it fits in 64 bits.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self.node() {
            MontyNode::Int(value) => Some(*value),
            MontyNode::BigInt(value) => value.to_i64(),
            _ => None,
        }
    }

    /// The value as a `str`.
    #[must_use]
    pub fn as_str(&self) -> Option<&'a str> {
        match self.node() {
            MontyNode::String(value) => Some(value),
            _ => None,
        }
    }

    /// The value as a `bool`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self.node() {
            MontyNode::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a `float`; an `int` converts as Python's `float()` does.
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self.node() {
            MontyNode::Float(value) => Some(*value),
            MontyNode::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// The Python `repr()` of the value.
    ///
    /// # Panics
    /// Could panic if out of memory.
    #[must_use]
    pub fn py_repr(&self) -> String {
        let mut s = String::new();
        self.repr_fmt(&mut s).expect("Unable to format repr display value");
        s
    }

    /// Whether the value is truthy under Python's rules: `None`, `False`,
    /// zero and empty containers are falsy; everything else is truthy.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self.node() {
            MontyNode::None => false,
            MontyNode::Bool(b) => *b,
            MontyNode::Int(i) => *i != 0,
            MontyNode::BigInt(bi) => !bi.is_zero(),
            MontyNode::Float(f) => *f != 0.0,
            MontyNode::String(s) => !s.is_empty(),
            MontyNode::Bytes(b) => !b.is_empty(),
            MontyNode::List(items)
            | MontyNode::Tuple(items)
            | MontyNode::Set(items)
            | MontyNode::FrozenSet(items)
            | MontyNode::NamedTuple { values: items, .. } => !items.is_empty(),
            MontyNode::Dict(pairs) => !pairs.is_empty(),
            MontyNode::TimeDelta(delta) => delta.days != 0 || delta.seconds != 0 || delta.microseconds != 0,
            _ => true,
        }
    }

    /// Writes the comma-separated `repr()`s of the nodes at `ids`.
    fn repr_items(&self, f: &mut impl Write, ids: &[NodeId]) -> fmt::Result {
        for (i, id) in ids.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            self.child(*id).repr_fmt(f)?;
        }
        Ok(())
    }

    /// Writes the Python `repr()`, recursing into containers.
    fn repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        match self.node() {
            MontyNode::Ellipsis => f.write_str("Ellipsis"),
            MontyNode::NotImplemented => f.write_str("NotImplemented"),
            MontyNode::None => f.write_str("None"),
            MontyNode::Bool(true) => f.write_str("True"),
            MontyNode::Bool(false) => f.write_str("False"),
            MontyNode::Int(v) => write!(f, "{v}"),
            MontyNode::BigInt(v) => write!(f, "{v}"),
            MontyNode::Float(v) => write!(f, "{}", FormatFloat(*v)),
            MontyNode::String(s) => string_repr_fmt(s, f),
            MontyNode::Bytes(b) => bytes_repr_fmt(b, f),
            MontyNode::List(ids) => {
                f.write_char('[')?;
                self.repr_items(f, ids)?;
                f.write_char(']')
            }
            MontyNode::Tuple(ids) => {
                f.write_char('(')?;
                self.repr_items(f, ids)?;
                f.write_char(')')
            }
            MontyNode::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                // type_name(field1=value1, field2=value2, ...)
                f.write_str(type_name)?;
                f.write_char('(')?;
                for (i, (name, id)) in field_names.iter().zip(values).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(name)?;
                    f.write_char('=')?;
                    self.child(*id).repr_fmt(f)?;
                }
                f.write_char(')')
            }
            MontyNode::Dict(pairs) => {
                f.write_char('{')?;
                for (i, (key, value)) in pairs.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    self.child(*key).repr_fmt(f)?;
                    f.write_str(": ")?;
                    self.child(*value).repr_fmt(f)?;
                }
                f.write_char('}')
            }
            MontyNode::Set(ids) => {
                if ids.is_empty() {
                    f.write_str("set()")
                } else {
                    f.write_char('{')?;
                    self.repr_items(f, ids)?;
                    f.write_char('}')
                }
            }
            MontyNode::FrozenSet(ids) => {
                f.write_str("frozenset(")?;
                if !ids.is_empty() {
                    f.write_char('{')?;
                    self.repr_items(f, ids)?;
                    f.write_char('}')?;
                }
                f.write_char(')')
            }
            MontyNode::Date(date) => write!(f, "datetime.date({}, {}, {})", date.year, date.month, date.day),
            MontyNode::DateTime(datetime) => {
                write!(
                    f,
                    "datetime.datetime({}, {}, {}, {}, {}",
                    datetime.year, datetime.month, datetime.day, datetime.hour, datetime.minute
                )?;
                if datetime.second != 0 || datetime.microsecond != 0 {
                    write!(f, ", {}", datetime.second)?;
                }
                if datetime.microsecond != 0 {
                    write!(f, ", {}", datetime.microsecond)?;
                }
                if let Some(offset) = datetime.offset_seconds {
                    tzinfo_repr_fmt(f, offset, datetime.timezone_name.as_deref())?;
                }
                f.write_char(')')
            }
            MontyNode::Time(time) => {
                write!(f, "datetime.time({}, {}", time.hour, time.minute)?;
                // CPython prints `second` whenever either sub-minute field is
                // set, so `time(1, 2, 0, 4)` reprs as `(1, 2, 0, 4)`.
                if time.second != 0 || time.microsecond != 0 {
                    write!(f, ", {}", time.second)?;
                }
                if time.microsecond != 0 {
                    write!(f, ", {}", time.microsecond)?;
                }
                if let Some(offset) = time.offset_seconds {
                    tzinfo_repr_fmt(f, offset, time.timezone_name.as_deref())?;
                }
                if time.fold != 0 {
                    write!(f, ", fold={}", time.fold)?;
                }
                f.write_char(')')
            }
            MontyNode::TimeDelta(delta) => {
                if delta.days == 0 && delta.seconds == 0 && delta.microseconds == 0 {
                    return f.write_str("datetime.timedelta(0)");
                }
                f.write_str("datetime.timedelta(")?;
                let mut first = true;
                if delta.days != 0 {
                    write!(f, "days={}", delta.days)?;
                    first = false;
                }
                if delta.seconds != 0 {
                    if !first {
                        f.write_str(", ")?;
                    }
                    write!(f, "seconds={}", delta.seconds)?;
                    first = false;
                }
                if delta.microseconds != 0 {
                    if !first {
                        f.write_str(", ")?;
                    }
                    write!(f, "microseconds={}", delta.microseconds)?;
                }
                f.write_char(')')
            }
            MontyNode::TimeZone(tz) => {
                if tz.offset_seconds == 0 && tz.name.is_none() {
                    return f.write_str("datetime.timezone.utc");
                }
                let timedelta_repr = format_offset_timedelta_repr(tz.offset_seconds);
                write!(f, "datetime.timezone({timedelta_repr}")?;
                if let Some(name) = &tz.name {
                    write!(f, ", {}", StringRepr(name))?;
                }
                f.write_char(')')
            }
            MontyNode::Exception { exc_type, arg } => {
                let type_str: &'static str = exc_type.into();
                write!(f, "{type_str}(")?;
                if let Some(arg) = arg {
                    string_repr_fmt(arg, f)?;
                }
                f.write_char(')')
            }
            MontyNode::ClassInstance { attrs, .. } => {
                // ClassName(attr1=value1, attr2=value2, ...) over the eager
                // attrs in order; a non-string key renders via repr rather
                // than panicking, since inputs are host-built
                f.write_str(self.type_name())?;
                f.write_char('(')?;
                for (i, (key, value)) in attrs.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    match self.graph.node(*key) {
                        MontyNode::String(key) => f.write_str(key)?,
                        _ => self.child(*key).repr_fmt(f)?,
                    }
                    f.write_char('=')?;
                    self.child(*value).repr_fmt(f)?;
                }
                f.write_char(')')
            }
            MontyNode::Path(p) => write!(f, "PosixPath('{p}')"),
            MontyNode::FileHandle(handle) => write!(f, "{handle}"),
            MontyNode::Type(t) => write!(f, "<class '{t}'>"),
            MontyNode::ClassType(class) => write!(f, "<class '{}'>", class.name),
            MontyNode::BuiltinFunction(func) => write!(f, "<built-in function {func}>"),
            MontyNode::Function { name, .. } => write!(f, "<function '{name}' external>"),
            MontyNode::Repr(s) => write!(f, "Repr({})", StringRepr(s)),
            MontyNode::Cycle(placeholder) => f.write_str(placeholder),
        }
    }
}

impl PartialEq for ValueRef<'_> {
    /// Structural equality as Python values: an `int` equals the same
    /// `BigInt`, a namedtuple equals a tuple of its values, floats compare
    /// bit-for-bit (so `NaN` round-trips equal), and a sub-object shared in
    /// one arena equals its copies in another. Linear in the arenas: each
    /// pair of nodes is compared once.
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self.id, other.id)];
        let mut seen = HashSet::new();
        while let Some((a, b)) = pending.pop() {
            if !seen.insert((a, b)) {
                continue;
            }
            if !nodes_eq(self.graph.node(a), other.graph.node(b), &mut pending) {
                return false;
            }
        }
        true
    }
}

impl PartialEq<MontyValue> for ValueRef<'_> {
    fn eq(&self, other: &MontyValue) -> bool {
        *self == other.as_ref()
    }
}

impl fmt::Display for ValueRef<'_> {
    /// The Python `str()` of the value: text as is, everything else its `repr()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node() {
            MontyNode::String(s) | MontyNode::Cycle(s) => f.write_str(s),
            _ => self.repr_fmt(f),
        }
    }
}

impl TryFrom<ValueRef<'_>> for i64 {
    type Error = ConversionError;

    fn try_from(value: ValueRef<'_>) -> Result<Self, ConversionError> {
        match value.node() {
            MontyNode::Int(i) => Ok(*i),
            _ => Err(ConversionError::new("int", value.type_name())),
        }
    }
}

/// An `int` converts as Python's `float()` does.
impl TryFrom<ValueRef<'_>> for f64 {
    type Error = ConversionError;

    fn try_from(value: ValueRef<'_>) -> Result<Self, ConversionError> {
        value
            .as_float()
            .ok_or_else(|| ConversionError::new("float", value.type_name()))
    }
}

impl TryFrom<ValueRef<'_>> for String {
    type Error = ConversionError;

    fn try_from(value: ValueRef<'_>) -> Result<Self, ConversionError> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ConversionError::new("str", value.type_name()))
    }
}

/// Only `True`/`False` convert; this is not Python truthiness (see [`ValueRef::is_truthy`]).
impl TryFrom<ValueRef<'_>> for bool {
    type Error = ConversionError;

    fn try_from(value: ValueRef<'_>) -> Result<Self, ConversionError> {
        value
            .as_bool()
            .ok_or_else(|| ConversionError::new("bool", value.type_name()))
    }
}

/// Writes the `, tzinfo=...` part of an aware datetime or time repr.
fn tzinfo_repr_fmt(f: &mut impl Write, offset: i32, name: Option<&str>) -> fmt::Result {
    if offset == 0 && name.is_none() {
        f.write_str(", tzinfo=datetime.timezone.utc")
    } else {
        let timedelta_repr = format_offset_timedelta_repr(offset);
        write!(f, ", tzinfo=datetime.timezone({timedelta_repr}")?;
        if let Some(name) = name {
            write!(f, ", {}", StringRepr(name))?;
        }
        f.write_char(')')
    }
}

/// Compares two nodes' own payloads and queues their children pairwise;
/// `false` when the nodes differ in kind, payload or child count.
fn nodes_eq(a: &MontyNode, b: &MontyNode, pending: &mut Vec<(NodeId, NodeId)>) -> bool {
    match (a, b) {
        // Cross-compare Int and BigInt without allocating a temporary BigInt.
        (MontyNode::Int(x), MontyNode::BigInt(y)) | (MontyNode::BigInt(y), MontyNode::Int(x)) => y.to_i64() == Some(*x),
        // NamedTuple compares with Tuple by values only (matching Python semantics)
        (MontyNode::NamedTuple { values: xs, .. }, MontyNode::Tuple(ys))
        | (MontyNode::Tuple(xs), MontyNode::NamedTuple { values: ys, .. }) => queue_items(xs, ys, pending),
        (MontyNode::List(xs), MontyNode::List(ys))
        | (MontyNode::Tuple(xs), MontyNode::Tuple(ys))
        | (MontyNode::Set(xs), MontyNode::Set(ys))
        | (MontyNode::FrozenSet(xs), MontyNode::FrozenSet(ys)) => queue_items(xs, ys, pending),
        (
            MontyNode::NamedTuple {
                type_name: x_type,
                field_names: x_fields,
                values: xs,
            },
            MontyNode::NamedTuple {
                type_name: y_type,
                field_names: y_fields,
                values: ys,
            },
        ) => x_type == y_type && x_fields == y_fields && queue_items(xs, ys, pending),
        (MontyNode::Dict(xs), MontyNode::Dict(ys)) => queue_pairs(xs, ys, pending),
        (MontyNode::ClassType(x), MontyNode::ClassType(y)) => {
            x.name == y.name
                && x.id == y.id
                && x.host_defined == y.host_defined
                && x.is_dataclass == y.is_dataclass
                && queue_pairs(&x.attrs, &y.attrs, pending)
        }
        (
            MontyNode::ClassInstance {
                class_type: x_class,
                instance_id: x_id,
                attrs: xs,
            },
            MontyNode::ClassInstance {
                class_type: y_class,
                instance_id: y_id,
                attrs: ys,
            },
        ) => {
            x_id == y_id && {
                pending.push((*x_class, *y_class));
                queue_pairs(xs, ys, pending)
            }
        }
        // every other pairing is leaf against leaf, or a kind mismatch
        _ => a.is_leaf() && b.is_leaf() && a == b,
    }
}

/// Queues two child lists pairwise; `false` when their lengths differ.
fn queue_items(xs: &[NodeId], ys: &[NodeId], pending: &mut Vec<(NodeId, NodeId)>) -> bool {
    xs.len() == ys.len() && {
        pending.extend(xs.iter().copied().zip(ys.iter().copied()));
        true
    }
}

/// Queues two pair lists key-for-key and value-for-value; `false` when
/// their lengths differ.
fn queue_pairs(xs: &[(NodeId, NodeId)], ys: &[(NodeId, NodeId)], pending: &mut Vec<(NodeId, NodeId)>) -> bool {
    xs.len() == ys.len() && {
        for ((xk, xv), (yk, yv)) in xs.iter().zip(ys) {
            pending.push((*xk, *yk));
            pending.push((*xv, *yv));
        }
        true
    }
}

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

    /// The keyword argument named `name`, if present.
    #[must_use]
    pub fn kwarg(&self, name: &str) -> Option<ValueRef<'_>> {
        self.kwargs()
            .find(|(key, _)| key.as_str() == Some(name))
            .map(|(_, value)| value)
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
}

/// Positional-only arguments; concrete so an empty `vec![]` infers.
impl From<Vec<MontyValue>> for CallArgs {
    fn from(args: Vec<MontyValue>) -> Self {
        Self::from((args, Vec::new()))
    }
}

/// Positional and keyword arguments, in order.
impl From<(Vec<MontyValue>, Vec<(MontyValue, MontyValue)>)> for CallArgs {
    fn from((args, kwargs): (Vec<MontyValue>, Vec<(MontyValue, MontyValue)>)) -> Self {
        let mut call = Self::new();
        for arg in args {
            call.push_arg(arg);
        }
        call.kwargs = push_pairs(kwargs, &mut call.values);
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
impl From<Vec<(String, MontyValue)>> for NamedValues {
    fn from(pairs: Vec<(String, MontyValue)>) -> Self {
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

/// Pushes each key then value and collects the id pairs.
fn push_pairs(
    pairs: impl IntoIterator<Item = (MontyValue, MontyValue)>,
    graph: &mut MontyGraph,
) -> Vec<(NodeId, NodeId)> {
    pairs
        .into_iter()
        .map(|(key, value)| {
            let key = key.push_into(graph);
            let value = value.push_into(graph);
            (key, value)
        })
        .collect()
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
