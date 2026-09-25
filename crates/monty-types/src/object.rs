//! Boundary values: [`MontyObject`] (an owned value),
//! [`ObjectRef`] (a borrowed value), the message carriers
//! [`CallArgs`] and [`NamedValues`], and the leaf payloads they hold:
//! [`MontyType`], the datetime value types, [`MontyFileHandle`], and the
//! errors reading or importing a value raises.
//!
//! Hosts build inputs with the [`MontyObject`] constructors (`MontyObject::int`,
//! `MontyObject::list`, ...) and read results through [`ObjectRef`]'s typed
//! accessors, structural equality and Python `repr()`. Graph representation
//! access requires opting into [`unstable`].

pub mod unstable;

use std::{
    borrow::Cow,
    collections::HashSet,
    error::Error,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    vec,
};

use ReprPiece::{Child, Text};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta};
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use crate::{
    builtins::BuiltinsFunctions,
    exceptions::ExcType,
    file_mode::FileMode,
    format::{FormatComplex, FormatFloat, StringRepr, bytes_repr_fmt, format_offset_timedelta_repr, string_repr_fmt},
    graph::{ClassTypeNode, GraphError, MontyGraph, MontyNode, NodeId},
    resource::ResourceError,
    unstable::PushValue,
    uuid::MontyUuid,
};

/// One owned Python value at the host boundary.
///
/// Carried by `Complete`, resume results, name lookups and `os.getenv`
/// defaults, and the type hosts build inputs with. Equality is structural as
/// Python values, independent of storage layout.
#[derive(Debug, Clone, Eq, serde::Serialize, serde::Deserialize)]
pub struct MontyObject {
    /// The arena holding the value and everything it references.
    graph: MontyGraph,
    /// The value's node.
    root: NodeId,
}

impl MontyObject {
    /// Pairs an arena with a root, checking the root is in range.
    fn new(graph: MontyGraph, root: NodeId) -> Result<Self, GraphError> {
        graph.check_root(root)?;
        Ok(Self { graph, root })
    }

    /// A value made of one leaf node.
    ///
    /// # Panics
    /// If `node` holds child ids.
    #[must_use]
    fn leaf(node: MontyNode) -> Self {
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

    /// A `complex` from its real and imaginary parts.
    #[must_use]
    pub fn complex(real: f64, imag: f64) -> Self {
        Self::leaf(MontyNode::Complex(MontyComplex { real, imag }))
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

    /// Borrows the value for inspection without copying.
    #[must_use]
    pub fn as_ref(&self) -> ObjectRef<'_> {
        ObjectRef {
            graph: &self.graph,
            id: self.root,
        }
    }

    /// The root node.
    #[must_use]
    fn root_node(&self) -> &MontyNode {
        self.graph.node(self.root)
    }

    /// The Python `repr()` of the value.
    #[must_use]
    pub fn py_repr(&self) -> String {
        self.as_ref().py_repr()
    }

    /// Whether the value is truthy under Python's rules; see [`ObjectRef::is_truthy`].
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

impl PartialEq for MontyObject {
    /// Structural equality as Python values, independent of arena layout.
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl PartialEq<ObjectRef<'_>> for MontyObject {
    fn eq(&self, other: &ObjectRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

impl fmt::Display for MontyObject {
    /// The Python `str()` of the value: text as is, everything else its `repr()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl TryFrom<&MontyObject> for i64 {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyObject> for f64 {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyObject> for String {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

impl TryFrom<&MontyObject> for bool {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, ConversionError> {
        value.as_ref().try_into()
    }
}

/// A borrowed Python value, for inspecting results, arguments and inputs without copying.
#[derive(Debug, Clone, Copy)]
pub struct ObjectRef<'a> {
    /// The arena.
    graph: &'a MontyGraph,
    /// The value's node.
    id: NodeId,
}

impl<'a> ObjectRef<'a> {
    /// The root node.
    #[must_use]
    fn node(&self) -> &'a MontyNode {
        self.graph.node(self.id)
    }

    /// The Python type name of the value, e.g. `"list"`.
    #[must_use]
    pub fn type_name(&self) -> &'a str {
        self.graph.type_name(self.id)
    }

    /// Copies into an owned value, preserving sharing within the value.
    #[must_use]
    pub fn to_owned(&self) -> MontyObject {
        let mut graph = MontyGraph::new();
        let root = self.push_into(&mut graph);
        MontyObject { graph, root }
    }

    /// The child at `id` of the same arena.
    #[must_use]
    fn child(&self, id: NodeId) -> Self {
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

    /// The value as a `complex`; an `int` or `float` converts as Python's
    /// `complex()` does, with a zero imaginary part.
    #[must_use]
    pub fn as_complex(&self) -> Option<MontyComplex> {
        match self.node() {
            MontyNode::Complex(value) => Some(*value),
            MontyNode::Float(value) => Some(MontyComplex {
                real: *value,
                imag: 0.0,
            }),
            MontyNode::Int(value) => Some(MontyComplex {
                real: *value as f64,
                imag: 0.0,
            }),
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
            MontyNode::Complex(c) => c.real != 0.0 || c.imag != 0.0,
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

    /// Writes the Python `repr()`. Containers are walked on an explicit stack
    /// of their [`ReprPiece`]s, so a deeply nested value costs heap rather than
    /// native stack; a sub-object shared `n` times renders `n` times, as
    /// CPython's `repr()` does.
    fn repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        let mut stack: Vec<vec::IntoIter<ReprPiece<'a>>> = Vec::new();
        let mut pending = Some(self.id);
        loop {
            if let Some(id) = pending.take() {
                let value = self.child(id);
                match value.repr_pieces() {
                    Some(pieces) => stack.push(pieces.into_iter()),
                    None => value.leaf_repr_fmt(f)?,
                }
            }
            let Some(pieces) = stack.last_mut() else {
                return Ok(());
            };
            match pieces.next() {
                Some(ReprPiece::Text(text)) => f.write_str(text)?,
                Some(ReprPiece::Child(child)) => pending = Some(child),
                None => {
                    stack.pop();
                }
            }
        }
    }

    /// The pieces of a container's `repr()` in order, or `None` for a leaf.
    fn repr_pieces(&self) -> Option<Vec<ReprPiece<'a>>> {
        let mut pieces = Vec::new();
        match self.node() {
            MontyNode::List(ids) => {
                pieces.push(Text("["));
                push_repr_items(&mut pieces, ids);
                pieces.push(Text("]"));
            }
            MontyNode::Tuple(ids) => {
                pieces.push(Text("("));
                push_repr_items(&mut pieces, ids);
                pieces.push(Text(")"));
            }
            MontyNode::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                // type_name(field1=value1, field2=value2, ...)
                pieces.extend([Text(type_name), Text("(")]);
                for (i, (name, id)) in field_names.iter().zip(values).enumerate() {
                    push_repr_separator(&mut pieces, i);
                    pieces.extend([Text(name), Text("="), Child(*id)]);
                }
                pieces.push(Text(")"));
            }
            MontyNode::Dict(pairs) => {
                pieces.push(Text("{"));
                for (i, (key, value)) in pairs.iter().enumerate() {
                    push_repr_separator(&mut pieces, i);
                    pieces.extend([Child(*key), Text(": "), Child(*value)]);
                }
                pieces.push(Text("}"));
            }
            MontyNode::Set(ids) if ids.is_empty() => pieces.push(Text("set()")),
            MontyNode::Set(ids) => {
                pieces.push(Text("{"));
                push_repr_items(&mut pieces, ids);
                pieces.push(Text("}"));
            }
            MontyNode::FrozenSet(ids) => {
                pieces.push(Text("frozenset("));
                if !ids.is_empty() {
                    pieces.push(Text("{"));
                    push_repr_items(&mut pieces, ids);
                    pieces.push(Text("}"));
                }
                pieces.push(Text(")"));
            }
            MontyNode::ClassInstance { attrs, .. } => {
                // ClassName(attr1=value1, ...); a non-string key (possible in
                // host-built input) renders via its repr rather than panicking.
                pieces.extend([Text(self.type_name()), Text("(")]);
                for (i, (key, value)) in attrs.iter().enumerate() {
                    push_repr_separator(&mut pieces, i);
                    match self.graph.node(*key) {
                        MontyNode::String(key) => pieces.push(Text(key)),
                        _ => pieces.push(Child(*key)),
                    }
                    pieces.extend([Text("="), Child(*value)]);
                }
                pieces.push(Text(")"));
            }
            _ => return None,
        }
        Some(pieces)
    }

    /// Writes the `repr()` of a leaf node.
    fn leaf_repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        match self.node() {
            MontyNode::Ellipsis => f.write_str("Ellipsis"),
            MontyNode::NotImplemented => f.write_str("NotImplemented"),
            MontyNode::None => f.write_str("None"),
            MontyNode::Bool(true) => f.write_str("True"),
            MontyNode::Bool(false) => f.write_str("False"),
            MontyNode::Int(v) => write!(f, "{v}"),
            MontyNode::BigInt(v) => write!(f, "{v}"),
            MontyNode::Float(v) => write!(f, "{}", FormatFloat(*v)),
            MontyNode::Complex(c) => write!(
                f,
                "{}",
                FormatComplex {
                    real: c.real,
                    imag: c.imag
                }
            ),
            MontyNode::String(s) => string_repr_fmt(s, f),
            MontyNode::Bytes(b) => bytes_repr_fmt(b, f),
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
            MontyNode::Path(p) => write!(f, "PosixPath('{p}')"),
            MontyNode::FileHandle(handle) => write!(f, "{handle}"),
            MontyNode::Type(t) => write!(f, "<class '{t}'>"),
            MontyNode::ClassType(class) => write!(f, "<class '{}'>", class.name),
            MontyNode::BuiltinFunction(func) => write!(f, "<built-in function {func}>"),
            MontyNode::Function { name, .. } => write!(f, "<function '{name}' external>"),
            MontyNode::Repr(s) => write!(f, "Repr({})", StringRepr(s)),
            MontyNode::Cycle(placeholder) => f.write_str(placeholder),
            MontyNode::List(_)
            | MontyNode::Tuple(_)
            | MontyNode::NamedTuple { .. }
            | MontyNode::Dict(_)
            | MontyNode::Set(_)
            | MontyNode::FrozenSet(_)
            | MontyNode::ClassInstance { .. } => unreachable!("containers render through repr_pieces"),
        }
    }
}

/// One step of a container's `repr()`: literal text, or a child rendered in
/// place. Text borrows the arena (a type or field name) or is static.
enum ReprPiece<'a> {
    Text(&'a str),
    Child(NodeId),
}

/// Pushes the children at `ids`, comma-separated.
fn push_repr_items(pieces: &mut Vec<ReprPiece<'_>>, ids: &[NodeId]) {
    for (i, id) in ids.iter().enumerate() {
        push_repr_separator(pieces, i);
        pieces.push(ReprPiece::Child(*id));
    }
}

/// Pushes the `, ` that precedes every entry but the first.
fn push_repr_separator(pieces: &mut Vec<ReprPiece<'_>>, index: usize) {
    if index > 0 {
        pieces.push(ReprPiece::Text(", "));
    }
}

impl PartialEq for ObjectRef<'_> {
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

impl PartialEq<MontyObject> for ObjectRef<'_> {
    fn eq(&self, other: &MontyObject) -> bool {
        *self == other.as_ref()
    }
}

impl fmt::Display for ObjectRef<'_> {
    /// The Python `str()` of the value: text as is, everything else its `repr()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node() {
            MontyNode::String(s) | MontyNode::Cycle(s) => f.write_str(s),
            _ => self.repr_fmt(f),
        }
    }
}

impl TryFrom<ObjectRef<'_>> for i64 {
    type Error = ConversionError;

    fn try_from(value: ObjectRef<'_>) -> Result<Self, ConversionError> {
        match value.node() {
            MontyNode::Int(i) => Ok(*i),
            _ => Err(ConversionError::new("int", value.type_name())),
        }
    }
}

/// An `int` converts as Python's `float()` does.
impl TryFrom<ObjectRef<'_>> for f64 {
    type Error = ConversionError;

    fn try_from(value: ObjectRef<'_>) -> Result<Self, ConversionError> {
        value
            .as_float()
            .ok_or_else(|| ConversionError::new("float", value.type_name()))
    }
}

impl TryFrom<ObjectRef<'_>> for String {
    type Error = ConversionError;

    fn try_from(value: ObjectRef<'_>) -> Result<Self, ConversionError> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ConversionError::new("str", value.type_name()))
    }
}

/// Only `True`/`False` convert; this is not Python truthiness (see [`ObjectRef::is_truthy`]).
impl TryFrom<ObjectRef<'_>> for bool {
    type Error = ConversionError;

    fn try_from(value: ObjectRef<'_>) -> Result<Self, ConversionError> {
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

/// The positional and keyword arguments of one function or OS call.
/// A sub-object shared within an exported call stays shared at the host boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallArgs {
    /// The arena holding every argument.
    graph: MontyGraph,
    /// Ids of the positional arguments, in order.
    arg_ids: Vec<NodeId>,
    /// Ids of the keyword arguments as `(key, value)` pairs, in order; keys are usually strings.
    kwarg_ids: Vec<(NodeId, NodeId)>,
}

impl CallArgs {
    /// No arguments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a positional argument.
    pub fn push_arg(&mut self, value: MontyObject) {
        unstable::push_arg(self, value);
    }

    /// Appends a keyword argument with a string key.
    pub fn push_kwarg(&mut self, name: &str, value: MontyObject) {
        unstable::push_kwarg(self, name, value);
    }

    /// The `index`th positional argument.
    #[must_use]
    pub fn arg(&self, index: usize) -> Option<ObjectRef<'_>> {
        self.arg_ids.get(index).map(|id| self.graph.value(*id))
    }

    /// The positional arguments, in order.
    #[must_use]
    pub fn args(&self) -> impl ExactSizeIterator<Item = ObjectRef<'_>> {
        self.arg_ids.iter().map(|id| self.graph.value(*id))
    }

    /// The keyword arguments as `(key, value)` views, in order.
    #[must_use]
    pub fn kwargs(&self) -> impl ExactSizeIterator<Item = (ObjectRef<'_>, ObjectRef<'_>)> {
        self.kwarg_ids
            .iter()
            .map(|(key, value)| (self.graph.value(*key), self.graph.value(*value)))
    }

    /// The keyword argument named `name`, if present.
    #[must_use]
    pub fn kwarg(&self, name: &str) -> Option<ObjectRef<'_>> {
        self.kwargs()
            .find(|(key, _)| key.as_str() == Some(name))
            .map(|(_, value)| value)
    }

    /// Checks every argument id is inside the arena; run on decoded messages.
    fn check_roots(&self) -> Result<(), GraphError> {
        self.arg_ids.iter().try_for_each(|id| self.graph.check_root(*id))?;
        self.kwarg_ids
            .iter()
            .try_for_each(|(key, value)| self.graph.check_root(*key).and_then(|()| self.graph.check_root(*value)))
    }
}

/// Positional-only arguments; concrete so an empty `vec![]` infers.
impl From<Vec<MontyObject>> for CallArgs {
    fn from(args: Vec<MontyObject>) -> Self {
        Self::from((args, Vec::new()))
    }
}

/// Positional and keyword arguments, in order.
impl From<(Vec<MontyObject>, Vec<(MontyObject, MontyObject)>)> for CallArgs {
    fn from((args, kwargs): (Vec<MontyObject>, Vec<(MontyObject, MontyObject)>)) -> Self {
        let mut call = Self::new();
        for arg in args {
            call.push_arg(arg);
        }
        call.kwarg_ids = push_pairs(kwargs, &mut call.graph);
        call
    }
}

/// The named inputs of one feed, preserving sharing between exported values.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamedValues {
    /// The arena holding every value.
    graph: MontyGraph,
    /// `(name, id)` pairs, in order.
    names: Vec<(String, NodeId)>,
}

impl NamedValues {
    /// No values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a named value.
    pub fn push(&mut self, name: impl Into<String>, value: MontyObject) {
        unstable::push_named(self, name, value);
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
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, ObjectRef<'_>)> {
        self.names
            .iter()
            .map(|(name, id)| (name.as_str(), self.graph.value(*id)))
    }

    /// Checks every id is inside the arena; run on decoded messages.
    fn check_roots(&self) -> Result<(), GraphError> {
        self.names.iter().try_for_each(|(_, id)| self.graph.check_root(*id))
    }
}

/// Named values in order; concrete so an empty `vec![]` infers.
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
    pub fn value(&self, id: NodeId) -> ObjectRef<'_> {
        assert!(id.index() < self.len(), "node id {id} is out of range");
        ObjectRef { graph: self, id }
    }
}

impl PushValue for MontyObject {
    /// Merges the value's arena in and returns its rebased root.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let offset = graph.merge(self.graph);
        NodeId(self.root.0 + offset)
    }
}

impl PushValue for ObjectRef<'_> {
    /// Copies the reachable nodes; a sub-object shared inside the value stays
    /// shared. Linear sweeps over the value's index span, no recursion, so a
    /// deep value from an untrusted worker costs heap rather than native stack.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let nodes = self.graph.nodes();
        let root = self.id.index();
        // Children are lower than their holder, so the value is inside
        // `lowest..=root`. A merged arena keeps each value contiguous, so the
        // sweeps cost the value rather than the whole arena before it.
        let mut lowest = root;
        let mut index = root;
        loop {
            nodes[index].for_each_child(|child| lowest = lowest.min(child.index()));
            if index == lowest {
                break;
            }
            index -= 1;
        }
        let span = &nodes[lowest..=root];
        // Sweeping downwards from the root visits each holder before its children.
        let mut reachable = vec![false; span.len()];
        reachable[root - lowest] = true;
        for (offset, node) in span.iter().enumerate().rev() {
            if reachable[offset] {
                node.for_each_child(|child| reachable[child.index() - lowest] = true);
            }
        }
        // Copying upwards then meets every child before the node holding it.
        let mut copied: Vec<Option<NodeId>> = vec![None; span.len()];
        for (offset, node) in span.iter().enumerate() {
            if reachable[offset] {
                let mut node = node.clone();
                node.for_each_child_mut(|child| {
                    *child = copied[child.index() - lowest].expect("children are copied first");
                });
                copied[offset] = Some(graph.push(node));
            }
        }
        copied[root - lowest].expect("the root is copied")
    }
}

/// Pushes each key then value and collects the id pairs.
fn push_pairs(
    pairs: impl IntoIterator<Item = (MontyObject, MontyObject)>,
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

/// The Python type of a builtin at the host boundary: the public mirror of
/// the runtime `Type` enum, minus class types, which cross as their own
/// [`class_type`](MontyObject::class_type) value. Serializable and
/// displayable without heap access.
///
/// Every runtime type is mirrored, not only those a host materialises as its
/// own class: the rest cross as a named proxy so the same type object can
/// round-trip back into the sandbox.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumIter,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::VariantNames,
)]
#[strum(serialize_all = "lowercase")]
pub enum MontyType {
    Ellipsis,
    Type,
    #[strum(serialize = "NoneType")]
    NoneType,
    Bool,
    Int,
    Float,
    Complex,
    Range,
    Slice,
    /// The four `datetime` classes carry the qualified names the runtime
    /// `Type` uses (`datetime.date`, ...) rather than bare `date`, so a type
    /// object keeps one name either side of the boundary.
    #[strum(serialize = "datetime.date")]
    Date,
    #[strum(serialize = "datetime.datetime")]
    DateTime,
    #[strum(serialize = "datetime.timedelta")]
    TimeDelta,
    #[strum(serialize = "datetime.timezone")]
    TimeZone,
    Str,
    Bytes,
    List,
    /// `collections.deque`. Qualified like `datetime.datetime` so the
    /// host-boundary name matches the runtime `Type::Deque` (`collections.deque`)
    /// rather than a bare `deque`.
    #[strum(serialize = "collections.deque")]
    Deque,
    #[strum(serialize = "list_iterator")]
    ListIterator,
    #[strum(serialize = "callable_iterator")]
    CallableIterator,
    Tuple,
    NamedTuple,
    Dict,
    #[strum(serialize = "dict_keys")]
    DictKeys,
    #[strum(serialize = "dict_items")]
    DictItems,
    #[strum(serialize = "dict_values")]
    DictValues,
    Set,
    FrozenSet,
    /// Exception types render/parse via `ExcType`'s own strum name
    /// (`"ValueError"`, `"json.JSONDecodeError"`, ...), so this variant is
    /// `#[strum(disabled)]`: [`name`](Self::name) and
    /// [`from_type_name`](Self::from_type_name) peel `Exception` off
    /// explicitly.
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    #[strum(serialize = "builtin_function_or_method")]
    BuiltinFunction,
    Cell,
    Iterator,
    Coroutine,
    Module,
    #[strum(serialize = "_io.TextIOWrapper")]
    TextIOWrapper,
    #[strum(serialize = "_io.BufferedReader")]
    BufferedReader,
    #[strum(serialize = "_io.BufferedWriter")]
    BufferedWriter,
    #[strum(serialize = "_io.BufferedRandom")]
    BufferedRandom,
    #[strum(serialize = "typing._SpecialForm")]
    SpecialForm,
    #[strum(serialize = "PosixPath")]
    Path,
    Property,
    #[strum(serialize = "re.Pattern")]
    RePattern,
    #[strum(serialize = "re.Match")]
    ReMatch,
    // Variants serialize by name into dumps: renaming one needs `#[serde(alias)]`.
    #[strum(serialize = "tuple_iterator")]
    TupleIterator,
    #[strum(serialize = "str_ascii_iterator")]
    StrAsciiIterator,
    #[strum(serialize = "str_iterator")]
    StrIterator,
    #[strum(serialize = "bytes_iterator")]
    BytesIterator,
    #[strum(serialize = "range_iterator")]
    RangeIterator,
    #[strum(serialize = "dict_keyiterator")]
    DictKeyIterator,
    #[strum(serialize = "dict_itemiterator")]
    DictItemIterator,
    #[strum(serialize = "dict_valueiterator")]
    DictValueIterator,
    #[strum(serialize = "set_iterator")]
    SetIterator,
    #[strum(serialize = "itertools.count")]
    ItertoolsCount,
    #[strum(serialize = "itertools.repeat")]
    ItertoolsRepeat,
    /// A `dataclasses.Field` describing one field of a sandbox `@dataclass`,
    /// as found in a class's `__dataclass_fields__`.
    #[strum(serialize = "Field")]
    Field,
    #[strum(serialize = "itertools.pairwise")]
    ItertoolsPairwise,
    #[strum(serialize = "itertools.compress")]
    ItertoolsCompress,
    #[strum(serialize = "itertools.islice")]
    ItertoolsIslice,
    #[strum(serialize = "itertools.chain")]
    ItertoolsChain,
    #[strum(serialize = "itertools.cycle")]
    ItertoolsCycle,
    #[strum(serialize = "NotImplementedType")]
    NotImplementedType,
    /// The `__dataclass_params__` of a sandbox `@dataclass`: the options it was
    /// decorated with, named as CPython's private class reports itself.
    #[strum(serialize = "_DataclassParams")]
    DataclassParams,
    #[strum(serialize = "itertools.takewhile")]
    ItertoolsTakeWhile,
    #[strum(serialize = "itertools.dropwhile")]
    ItertoolsDropWhile,
    #[strum(serialize = "itertools.filterfalse")]
    ItertoolsFilterFalse,
    #[strum(serialize = "itertools.starmap")]
    ItertoolsStarMap,
    /// The builtin `object`, which the sandbox exposes as a name only — it is
    /// not a base class and cannot be constructed.
    Object,
    #[strum(serialize = "datetime.time")]
    Time,
    /// `functools.partial`, qualified the way CPython's `tp_name` is.
    #[strum(serialize = "functools.partial")]
    Partial,
    #[strum(serialize = "itertools.accumulate")]
    ItertoolsAccumulate,
    #[strum(serialize = "itertools.batched")]
    ItertoolsBatched,
    #[strum(serialize = "itertools.zip_longest")]
    ItertoolsZipLongest,
    /// `types.GenericAlias`, the type of `list[int]`, qualified the way CPython's `tp_name` is.
    #[strum(serialize = "types.GenericAlias")]
    GenericAlias,
    /// `typing.Union`, the type of `int | None` (one object with `types.UnionType` since 3.14).
    #[strum(serialize = "typing.Union")]
    Union,
    #[strum(serialize = "itertools.combinations")]
    ItertoolsCombinations,
    #[strum(serialize = "itertools.combinations_with_replacement")]
    ItertoolsCombinationsWithReplacement,
    #[strum(serialize = "itertools.permutations")]
    ItertoolsPermutations,
    #[strum(serialize = "itertools.product")]
    ItertoolsProduct,
    #[strum(serialize = "itertools.groupby")]
    ItertoolsGroupBy,
    #[strum(serialize = "itertools._grouper")]
    ItertoolsGrouper,
    #[strum(serialize = "itertools._tee")]
    ItertoolsTee,
    #[strum(serialize = "itertools._tee_dataobject")]
    ItertoolsTeeDataObject,
}

impl fmt::Display for MontyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl MontyType {
    /// The Python-visible name of this type (`"int"`, `"datetime.datetime"`,
    /// `"ValueError"`).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Exception(exc_type) => (*exc_type).into(),
            // Every remaining variant is named by strum's `IntoStaticStr`
            // (`Exception` is peeled off above).
            other => other.into(),
        }
    }

    /// Parses builtin and exception type names produced by [`Display`](fmt::Display)/[`name`](Self::name).
    /// Unrecognized names return `None`; `"object"` parses to [`Object`](Self::Object).
    #[must_use]
    pub fn from_type_name(name: &str) -> Option<Self> {
        name.parse::<Self>()
            .ok()
            .or_else(|| name.parse::<ExcType>().ok().map(Self::Exception))
    }
}

/// A Python `datetime.date` value with year, month, and day components.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MontyDate {
    /// Gregorian year in range 1..=9999.
    pub year: i32,
    /// Month component in range 1..=12.
    pub month: u8,
    /// Day component valid for the given month/year.
    pub day: u8,
}

/// A Python `datetime.datetime` value with date, time, and optional timezone components.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyDateTime {
    /// Gregorian year in range 1..=9999.
    pub year: i32,
    /// Month component in range 1..=12.
    pub month: u8,
    /// Day component valid for the given month/year.
    pub day: u8,
    /// Hour in range 0..=23.
    pub hour: u8,
    /// Minute in range 0..=59.
    pub minute: u8,
    /// Second in range 0..=59.
    pub second: u8,
    /// Microsecond in range 0..=999_999.
    pub microsecond: u32,
    /// Fixed offset seconds for aware datetimes, or `None` for naive values.
    ///
    /// Within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`] when set.
    pub offset_seconds: Option<i32>,
    /// Optional explicit timezone name for aware datetimes.
    ///
    /// Must be `None` when `offset_seconds` is `None`.
    pub timezone_name: Option<String>,
}

/// A Python `datetime.time` value: a wall clock with no date attached.
///
/// `fold` is carried so the flag survives the boundary, but neither monty nor
/// this type interprets it — as in CPython it takes no part in equality.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyTime {
    /// Hour in range 0..=23.
    pub hour: u8,
    /// Minute in range 0..=59.
    pub minute: u8,
    /// Second in range 0..=59.
    pub second: u8,
    /// Microsecond in range 0..=999_999.
    pub microsecond: u32,
    /// Fixed offset seconds for aware times, or `None` for naive values.
    ///
    /// Within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`] when set.
    pub offset_seconds: Option<i32>,
    /// Optional explicit timezone name for aware times.
    ///
    /// Must be `None` when `offset_seconds` is `None`.
    pub timezone_name: Option<String>,
    /// Fold flag, 0 or 1.
    pub fold: u8,
}

/// A Python `complex` value as its two `float` parts.
///
/// Compared bit-for-bit inside a graph (see `MontyNode`'s `PartialEq`), so a
/// `NaN` part round-trips equal and `-0.0` stays distinct from `0.0`.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct MontyComplex {
    /// The real part.
    pub real: f64,
    /// The imaginary part.
    pub imag: f64,
}

impl MontyComplex {
    /// Bit-for-bit equality of both parts.
    #[must_use]
    pub fn bits_eq(&self, other: &Self) -> bool {
        self.real.to_bits() == other.real.to_bits() && self.imag.to_bits() == other.imag.to_bits()
    }
}

/// A Python `datetime.timedelta` value representing a duration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MontyTimeDelta {
    /// Day component.
    pub days: i32,
    /// Seconds component in normalized range 0..86400.
    pub seconds: i32,
    /// Microseconds component in normalized range 0..1_000_000.
    pub microseconds: i32,
}

/// Smallest UTC offset `datetime.timezone` accepts, -23:59:59.
///
/// CPython requires an offset strictly inside ±24 hours. Shared with the wire
/// decoder so a forged offset is rejected at the boundary rather than by the
/// sandbox-side constructor, which by then can only report a generic bad value.
pub const MIN_TIMEZONE_OFFSET_SECONDS: i32 = -86_399;
/// Largest UTC offset `datetime.timezone` accepts, +23:59:59.
///
/// See [`MIN_TIMEZONE_OFFSET_SECONDS`].
pub const MAX_TIMEZONE_OFFSET_SECONDS: i32 = 86_399;

/// A Python `datetime.timezone` fixed-offset timezone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyTimeZone {
    /// Fixed UTC offset in seconds, within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`].
    pub offset_seconds: i32,
    /// Optional display name.
    pub name: Option<String>,
}

/// Wall-clock microseconds since midnight, before any offset is applied.
///
/// Every field is range-bounded by its type, so this is total and cannot
/// overflow — unlike the datetime equivalent, which can fail on an invalid date.
fn monty_time_local_micros(time: &MontyTime) -> i64 {
    i64::from(time.hour) * 3_600_000_000
        + i64::from(time.minute) * 60_000_000
        + i64::from(time.second) * 1_000_000
        + i64::from(time.microsecond)
}

/// Comparison key: offset-adjusted microseconds for an aware time, wall-clock
/// microseconds for a naive one.
///
/// The adjusted value is deliberately NOT wrapped into a 24-hour day — a bare
/// time has no date to carry into, so `time(1, 0, utc)` differs from
/// `time(23, 0, minus_two)`, as in CPython.
fn monty_time_key(time: &MontyTime) -> i64 {
    monty_time_local_micros(time) - i64::from(time.offset_seconds.unwrap_or(0)) * 1_000_000
}

/// Aware and naive times never compare equal, and `fold` takes no part —
/// both matching CPython.
impl PartialEq for MontyTime {
    fn eq(&self, other: &Self) -> bool {
        self.offset_seconds.is_some() == other.offset_seconds.is_some() && monty_time_key(self) == monty_time_key(other)
    }
}

impl Eq for MontyTime {}

impl Hash for MontyTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Must agree with `PartialEq`: awareness and the adjusted key only,
        // never `fold`.
        self.offset_seconds.is_some().hash(state);
        monty_time_key(self).hash(state);
    }
}

impl PartialEq for MontyDateTime {
    fn eq(&self, other: &Self) -> bool {
        let self_aware = self.offset_seconds.is_some();
        let other_aware = other.offset_seconds.is_some();
        if self_aware != other_aware {
            return false;
        }

        if self_aware {
            return monty_datetime_utc_micros(self)
                .zip(monty_datetime_utc_micros(other))
                .is_some_and(|(lhs, rhs)| lhs == rhs)
                || monty_datetime_raw_eq(self, other);
        }

        monty_datetime_local_micros(self)
            .zip(monty_datetime_local_micros(other))
            .is_some_and(|(lhs, rhs)| lhs == rhs)
            || monty_datetime_raw_eq(self, other)
    }
}

impl Eq for MontyDateTime {}

impl Hash for MontyDateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.offset_seconds.is_some()
            && let Some(utc_micros) = monty_datetime_utc_micros(self)
        {
            utc_micros.hash(state);
            return;
        }
        if let Some(local_micros) = monty_datetime_local_micros(self) {
            local_micros.hash(state);
            return;
        }

        // Invalid carrier values should still hash deterministically instead of panicking.
        self.year.hash(state);
        self.month.hash(state);
        self.day.hash(state);
        self.hour.hash(state);
        self.minute.hash(state);
        self.second.hash(state);
        self.microsecond.hash(state);
        self.offset_seconds.hash(state);
        self.timezone_name.hash(state);
    }
}

impl PartialEq for MontyTimeZone {
    fn eq(&self, other: &Self) -> bool {
        self.offset_seconds == other.offset_seconds
    }
}

impl Eq for MontyTimeZone {}

impl Hash for MontyTimeZone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.offset_seconds.hash(state);
    }
}

/// Error returned when a [`MontyObject`] cannot be converted to the requested Rust type.
///
/// Returned by the `TryFrom` implementations when an [`ObjectRef`] holds a
/// different kind of value than the one requested.
#[derive(Debug)]
pub struct ConversionError {
    /// The type name that was expected (e.g., "int", "str").
    pub expected: &'static str,
    /// The actual type name of the value (e.g., "list", "NoneType", or a
    /// class instance's class name).
    pub actual: String,
}

impl ConversionError {
    /// Creates a new [`ConversionError`] with the expected and actual type names.
    #[must_use]
    pub fn new(expected: &'static str, actual: impl Into<String>) -> Self {
        Self {
            expected,
            actual: actual.into(),
        }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got {}", self.expected, self.actual)
    }
}

impl Error for ConversionError {}

/// Error returned when a value cannot be used as an input to code execution.
///
/// This can occur when:
/// - A value (like [`MontyObject::repr`]) is only valid as an output, not an input
/// - A resource limit is exceeded during conversion
#[derive(Debug, Clone)]
pub enum InvalidInputError {
    /// The input type is not valid for conversion to a runtime Value.
    /// Message explaining why the type is invalid.
    InvalidType(Cow<'static, str>),
    /// A resource limit was exceeded during conversion.
    Resource(ResourceError),
}

impl InvalidInputError {
    /// Creates a new [`InvalidInputError`] for the given type name.
    #[must_use]
    pub fn invalid_type(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::InvalidType(msg.into())
    }
}

impl fmt::Display for InvalidInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType(msg) => write!(f, "{msg}"),
            Self::Resource(e) => write!(f, "{e}"),
        }
    }
}

impl Error for InvalidInputError {}

impl From<ResourceError> for InvalidInputError {
    fn from(err: ResourceError) -> Self {
        Self::Resource(err)
    }
}

/// An open file object (the result of `open()`).
///
/// This is the boundary representation of Monty's heap `OpenFile`
/// wrapper. It carries everything needed to service a file operation from a
/// host that holds no live OS handle: the virtual `path`, the `mode`, and
/// the byte `position` for seek-aware reads.
///
/// The host produces a `FileHandle` as the result of an
/// [`OsFunctionCall::Open`](crate::os::OsFunctionCall::Open) call; the
/// interpreter then builds its heap file wrapper from it. Conversely, a heap file
/// object passed as an argument to a `read`/`write` OS call is converted
/// back to a `FileHandle` so the host receives this state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyFileHandle {
    /// The virtual (sandbox) path of the file. Never a host path.
    pub path: String,
    /// The parsed `open()` mode.
    pub mode: FileMode,
    /// Position for sized/line/seek operations: char index in text mode,
    /// byte index in binary mode. `0` for a freshly opened file.
    pub position: u64,
}

impl fmt::Display for MontyFileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<{} name={} mode={}>",
            self.mode.file_type_name(),
            StringRepr(&self.path),
            StringRepr(self.mode.as_str())
        )
    }
}

fn monty_datetime_local_micros(datetime: &MontyDateTime) -> Option<i64> {
    monty_datetime_naive(datetime).map(|naive| naive.and_utc().timestamp_micros())
}

fn monty_datetime_raw_eq(a: &MontyDateTime, b: &MontyDateTime) -> bool {
    a.year == b.year
        && a.month == b.month
        && a.day == b.day
        && a.hour == b.hour
        && a.minute == b.minute
        && a.second == b.second
        && a.microsecond == b.microsecond
        && a.offset_seconds == b.offset_seconds
        && a.timezone_name == b.timezone_name
}

fn monty_datetime_utc_micros(datetime: &MontyDateTime) -> Option<i64> {
    let offset_seconds = datetime.offset_seconds?;
    let offset_delta = ChronoTimeDelta::try_seconds(i64::from(offset_seconds))?;
    let utc = monty_datetime_naive(datetime)?.checked_sub_signed(offset_delta)?;
    Some(utc.and_utc().timestamp_micros())
}

fn monty_datetime_naive(datetime: &MontyDateTime) -> Option<NaiveDateTime> {
    let date = NaiveDate::from_ymd_opt(datetime.year, u32::from(datetime.month), u32::from(datetime.day))?;
    let time = NaiveTime::from_hms_micro_opt(
        u32::from(datetime.hour),
        u32::from(datetime.minute),
        u32::from(datetime.second),
        datetime.microsecond,
    )?;
    Some(date.and_time(time))
}
