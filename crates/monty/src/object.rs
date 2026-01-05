use std::{
    fmt::{self, Write},
    hash::{Hash, Hasher},
};

use ahash::AHashSet;
use indexmap::IndexMap;

use crate::{
    builtins::Builtins,
    exception_private::{ExcType, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{
        bytes::{bytes_repr, Bytes},
        dict::Dict,
        list::List,
        set::{FrozenSet, Set},
        str::{string_repr, Str},
        tuple::Tuple,
        PyTrait, Type,
    },
    value::Value,
};

/// A Python value that can be passed to or returned from the interpreter.
///
/// This is the public-facing type for Python values. It owns all its data and can be
/// freely cloned, serialized, or stored. Unlike the internal `Value` type, `MontyObject`
/// does not require a heap for operations.
///
/// # Input vs Output Variants
///
/// Most variants can be used both as inputs (passed to `Executor::run()`) and outputs
/// (returned from execution). However:
/// - `Repr` is output-only: represents values that have no direct `MontyObject` mapping
/// - `Exception` can be used as input (to raise) or output (when code raises)
///
/// # Hashability
///
/// Only immutable variants (`None`, `Ellipsis`, `Bool`, `Int`, `Float`, `String`, `Bytes`)
/// implement `Hash`. Attempting to hash mutable variants (`List`, `Dict`) will panic.
///
/// # JSON Serialization
///
/// `MontyObject` supports JSON serialization with natural mappings:
///
/// **Bidirectional (can serialize and deserialize):**
/// - `None` ↔ JSON `null`
/// - `Bool` ↔ JSON `true`/`false`
/// - `Int` ↔ JSON integer
/// - `Float` ↔ JSON float
/// - `String` ↔ JSON string
/// - `List` ↔ JSON array
/// - `Dict` ↔ JSON object (keys must be interns)
///
/// **Output-only (serialize only, cannot deserialize from JSON):**
/// - `Ellipsis` → `{"$ellipsis": true}`
/// - `Tuple` → `{"$tuple": [...]}`
/// - `Bytes` → `{"$bytes": [...]}`
/// - `Exception` → `{"$exception": {"type": "...", "arg": "..."}}`
/// - `Repr` → `{"$repr": "..."}`
///
/// # Binary Serialization
///
/// For binary serialization (e.g., with postcard), `MontyObject` uses derived serde
/// with internally tagged format. This differs from the natural JSON format.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MontyObject {
    /// Python's `Ellipsis` singleton (`...`).
    Ellipsis,
    /// Python's `None` singleton.
    None,
    /// Python boolean (`True` or `False`).
    Bool(bool),
    /// Python integer (64-bit signed).
    Int(i64),
    /// Python float (64-bit IEEE 754).
    Float(f64),
    /// Python string (UTF-8).
    String(String),
    /// Python bytes object.
    Bytes(Vec<u8>),
    /// Python list (mutable sequence).
    List(Vec<MontyObject>),
    /// Python tuple (immutable sequence).
    Tuple(Vec<MontyObject>),
    /// Python dictionary (insertion-ordered mapping).
    Dict(DictPairs),
    /// Python set (mutable, unordered collection of unique elements).
    Set(Vec<MontyObject>),
    /// Python frozenset (immutable, unordered collection of unique elements).
    FrozenSet(Vec<MontyObject>),
    /// Python exception with type and optional message argument.
    Exception {
        /// The exception type (e.g., `ValueError`, `TypeError`).
        exc_type: ExcType,
        /// Optional string argument passed to the exception constructor.
        arg: Option<String>,
    },
    /// A Python type object (e.g., `int`, `str`, `list`).
    ///
    /// Returned by the `type()` builtin and can be compared with other types.
    Type(Type),
    /// A dataclass instance with class name, fields, method names, and mutability.
    Dataclass {
        /// The class name (e.g., "Point", "User").
        name: String,
        /// Field name -> value mapping.
        fields: DictPairs,
        /// Method names that trigger external function calls.
        methods: Vec<String>,
        /// Whether this dataclass instance is mutable.
        mutable: bool,
    },
    /// Fallback for values that cannot be represented as other variants.
    ///
    /// Contains the `repr()` string of the original value.
    ///
    /// This is output-only and cannot be used as an input to `Executor::run()`.
    Repr(String),
    /// Represents a cycle detected during Value-to-MontyObject conversion.
    ///
    /// When converting cyclic structures (e.g., `a = []; a.append(a)`), this variant
    /// is used to break the infinite recursion. Contains the heap ID and the type-specific
    /// placeholder string (e.g., `"[...]"` for lists, `"{...}"` for dicts).
    ///
    /// This is output-only and cannot be used as an input to `Executor::run()`.
    Cycle(HeapId, String),
}

impl fmt::Display for MontyObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Cycle(_, placeholder) => f.write_str(placeholder),
            Self::Type(t) => write!(f, "<class '{t}'>"),
            _ => self.repr_fmt(f),
        }
    }
}

impl MontyObject {
    /// Converts a `Value` into a `MontyObject`, properly handling reference counting.
    ///
    /// Takes ownership of the `Value`, extracts its content to create a MontyObject,
    /// then properly drops the Value via `drop_with_heap` to maintain reference counting.
    ///
    /// The `interns` parameter is used to look up interned string/bytes content.
    pub(crate) fn new(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Self {
        let py_obj = Self::from_value(&value, heap, interns);
        value.drop_with_heap(heap);
        py_obj
    }

    /// Creates a new `MontyObject` from something that can be converted into a `DictPairs`.
    pub fn dict(dict: impl Into<DictPairs>) -> Self {
        Self::Dict(dict.into())
    }

    /// Converts this `MontyObject` into an `Value`, allocating on the heap if needed.
    ///
    /// Immediate values (None, Bool, Int, Float, Ellipsis, Exception) are created directly.
    /// Heap-allocated values (String, Bytes, List, Tuple, Dict) are allocated
    /// via the heap and wrapped in `Value::Ref`.
    ///
    /// # Errors
    /// Returns `InvalidInputError` if called on the `Repr` variant,
    /// as it is only valid as an output from code execution, not as an input.
    pub(crate) fn to_value(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<Value, InvalidInputError> {
        match self {
            Self::Ellipsis => Ok(Value::Ellipsis),
            Self::None => Ok(Value::None),
            Self::Bool(b) => Ok(Value::Bool(b)),
            Self::Int(i) => Ok(Value::Int(i)),
            Self::Float(f) => Ok(Value::Float(f)),
            Self::String(s) => Ok(Value::Ref(heap.allocate(HeapData::Str(Str::new(s)))?)),
            Self::Bytes(b) => Ok(Value::Ref(heap.allocate(HeapData::Bytes(Bytes::new(b)))?)),
            Self::List(items) => {
                let values: Vec<Value> = items
                    .into_iter()
                    .map(|item| item.to_value(heap, interns))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Ref(heap.allocate(HeapData::List(List::new(values)))?))
            }
            Self::Tuple(items) => {
                let values: Vec<Value> = items
                    .into_iter()
                    .map(|item| item.to_value(heap, interns))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Ref(heap.allocate(HeapData::Tuple(Tuple::new(values)))?))
            }
            Self::Dict(map) => {
                let pairs: Result<Vec<(Value, Value)>, InvalidInputError> = map
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_value(heap, interns)?, v.to_value(heap, interns)?)))
                    .collect();
                let dict = Dict::from_pairs(pairs?, heap, interns)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable dict keys"))?;
                Ok(Value::Ref(heap.allocate(HeapData::Dict(dict))?))
            }
            Self::Set(items) => {
                let mut set = Set::new();
                for item in items {
                    let value = item.to_value(heap, interns)?;
                    set.add(value, heap, interns)
                        .map_err(|_| InvalidInputError::invalid_type("unhashable set element"))?;
                }
                Ok(Value::Ref(heap.allocate(HeapData::Set(set))?))
            }
            Self::FrozenSet(items) => {
                let mut set = Set::new();
                for item in items {
                    let value = item.to_value(heap, interns)?;
                    set.add(value, heap, interns)
                        .map_err(|_| InvalidInputError::invalid_type("unhashable frozenset element"))?;
                }
                // Convert to frozenset by extracting storage
                let frozenset = FrozenSet::from_set(set);
                Ok(Value::Ref(heap.allocate(HeapData::FrozenSet(frozenset))?))
            }
            Self::Exception { exc_type, arg } => {
                let exc = SimpleException::new(exc_type, arg);
                Ok(Value::Ref(heap.allocate(HeapData::Exception(exc))?))
            }
            Self::Dataclass {
                name,
                fields,
                methods,
                mutable,
            } => {
                use crate::types::Dataclass;
                // Convert fields to Dict
                let pairs: Result<Vec<(Value, Value)>, InvalidInputError> = fields
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_value(heap, interns)?, v.to_value(heap, interns)?)))
                    .collect();
                let dict = Dict::from_pairs(pairs?, heap, interns)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable dataclass field keys"))?;
                // Convert methods Vec to AHashSet
                let methods_set: ahash::AHashSet<String> = methods.into_iter().collect();
                let dc = Dataclass::new(name, dict, methods_set, mutable);
                Ok(Value::Ref(heap.allocate(HeapData::Dataclass(dc))?))
            }
            Self::Repr(_) => Err(InvalidInputError::invalid_type("Repr")),
            Self::Cycle(_, _) => Err(InvalidInputError::invalid_type("Cycle")),
            Self::Type(t) => Ok(Value::Builtin(Builtins::Type(t))),
        }
    }

    fn from_value(object: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Self {
        let mut visited = AHashSet::new();
        Self::from_value_inner(object, heap, &mut visited, interns)
    }

    /// Internal helper for converting Value to MontyObject with cycle detection.
    ///
    /// The `visited` set tracks HeapIds we're currently processing. When we encounter
    /// a HeapId already in the set, we've found a cycle and return `MontyObject::Cycle`
    /// with an appropriate placeholder string.
    fn from_value_inner(
        object: &Value,
        heap: &Heap<impl ResourceTracker>,
        visited: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> Self {
        match object {
            Value::Undefined => panic!("Undefined found while converting to MontyObject"),
            Value::Ellipsis => Self::Ellipsis,
            Value::None => Self::None,
            Value::Bool(b) => Self::Bool(*b),
            Value::Int(i) => Self::Int(*i),
            Value::Float(f) => Self::Float(*f),
            Value::InternString(string_id) => Self::String(interns.get_str(*string_id).to_owned()),
            Value::InternBytes(bytes_id) => Self::Bytes(interns.get_bytes(*bytes_id).to_owned()),
            Value::Ref(id) => {
                // Check for cycle
                if visited.contains(id) {
                    // Cycle detected - return appropriate placeholder
                    return match heap.get(*id) {
                        HeapData::List(_) => Self::Cycle(*id, "[...]".to_owned()),
                        HeapData::Tuple(_) => Self::Cycle(*id, "(...)".to_owned()),
                        HeapData::Dict(_) => Self::Cycle(*id, "{...}".to_owned()),
                        _ => Self::Cycle(*id, "...".to_owned()),
                    };
                }

                // Mark this id as being visited
                visited.insert(*id);

                let result = match heap.get(*id) {
                    HeapData::Str(s) => Self::String(s.as_str().to_owned()),
                    HeapData::Bytes(b) => Self::Bytes(b.as_slice().to_owned()),
                    HeapData::List(list) => Self::List(
                        list.as_vec()
                            .iter()
                            .map(|obj| MontyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    HeapData::Tuple(tuple) => Self::Tuple(
                        tuple
                            .as_vec()
                            .iter()
                            .map(|obj| MontyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    HeapData::Dict(dict) => Self::Dict(DictPairs(
                        dict.into_iter()
                            .map(|(k, v)| {
                                (
                                    MontyObject::from_value_inner(k, heap, visited, interns),
                                    MontyObject::from_value_inner(v, heap, visited, interns),
                                )
                            })
                            .collect(),
                    )),
                    HeapData::Set(set) => Self::Set(
                        set.storage()
                            .iter()
                            .map(|obj| MontyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    HeapData::FrozenSet(frozenset) => Self::FrozenSet(
                        frozenset
                            .storage()
                            .iter()
                            .map(|obj| MontyObject::from_value_inner(obj, heap, visited, interns))
                            .collect(),
                    ),
                    // Cells are internal closure implementation details
                    HeapData::Cell(inner) => {
                        // Show the cell's contents
                        MontyObject::from_value_inner(inner, heap, visited, interns)
                    }
                    HeapData::Closure(..) | HeapData::FunctionDefaults(..) => {
                        Self::Repr(object.py_repr(heap, interns).into_owned())
                    }
                    HeapData::Range(range) => {
                        // Represent Range as a repr string since MontyObject doesn't have a Range variant
                        let mut s = String::new();
                        let _ = range.py_repr_fmt(&mut s, heap, visited, interns);
                        Self::Repr(s)
                    }
                    HeapData::Exception(exc) => Self::Exception {
                        exc_type: exc.exc_type(),
                        arg: exc.arg().map(ToString::to_string),
                    },
                    HeapData::Dataclass(dc) => {
                        // Convert fields to DictPairs
                        let fields = DictPairs(
                            dc.fields()
                                .into_iter()
                                .map(|(k, v)| {
                                    (
                                        MontyObject::from_value_inner(k, heap, visited, interns),
                                        MontyObject::from_value_inner(v, heap, visited, interns),
                                    )
                                })
                                .collect(),
                        );
                        // Convert methods set to sorted Vec for determinism
                        let mut methods: Vec<String> = dc.methods().iter().cloned().collect();
                        methods.sort();
                        Self::Dataclass {
                            name: dc.name().to_owned(),
                            fields,
                            methods,
                            mutable: dc.is_mutable(),
                        }
                    }
                };

                // Remove from visited set after processing
                visited.remove(id);
                result
            }
            Value::Builtin(Builtins::Type(t)) => Self::Type(*t),
            #[cfg(feature = "ref-count-panic")]
            Value::Dereferenced => panic!("Dereferenced found while converting to MontyObject"),
            _ => Self::Repr(object.py_repr(heap, interns).into_owned()),
        }
    }

    /// Returns the Python `repr()` string for this value.
    ///
    /// # Panics
    /// Could panic if out of memory.
    #[must_use]
    pub fn py_repr(&self) -> String {
        let mut s = String::new();
        self.repr_fmt(&mut s).expect("Unable to format repr display value");
        s
    }

    fn repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        match self {
            Self::Ellipsis => f.write_str("Ellipsis"),
            Self::None => f.write_str("None"),
            Self::Bool(true) => f.write_str("True"),
            Self::Bool(false) => f.write_str("False"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => {
                let s = v.to_string();
                f.write_str(&s)?;
                if !s.contains('.') {
                    f.write_str(".0")?;
                }
                Ok(())
            }
            Self::String(s) => f.write_str(&string_repr(s)),
            Self::Bytes(b) => f.write_str(&bytes_repr(b)),
            Self::List(l) => {
                f.write_char('[')?;
                let mut iter = l.iter();
                if let Some(first) = iter.next() {
                    first.repr_fmt(f)?;
                    for item in iter {
                        f.write_str(", ")?;
                        item.repr_fmt(f)?;
                    }
                }
                f.write_char(']')
            }
            Self::Tuple(t) => {
                f.write_char('(')?;
                let mut iter = t.iter();
                if let Some(first) = iter.next() {
                    first.repr_fmt(f)?;
                    for item in iter {
                        f.write_str(", ")?;
                        item.repr_fmt(f)?;
                    }
                }
                f.write_char(')')
            }
            Self::Dict(d) => {
                f.write_char('{')?;
                let mut iter = d.iter();
                if let Some((k, v)) = iter.next() {
                    k.repr_fmt(f)?;
                    f.write_str(": ")?;
                    v.repr_fmt(f)?;
                    for (k, v) in iter {
                        f.write_str(", ")?;
                        k.repr_fmt(f)?;
                        f.write_str(": ")?;
                        v.repr_fmt(f)?;
                    }
                }
                f.write_char('}')
            }
            Self::Set(s) => {
                if s.is_empty() {
                    f.write_str("set()")
                } else {
                    f.write_char('{')?;
                    let mut iter = s.iter();
                    if let Some(first) = iter.next() {
                        first.repr_fmt(f)?;
                        for item in iter {
                            f.write_str(", ")?;
                            item.repr_fmt(f)?;
                        }
                    }
                    f.write_char('}')
                }
            }
            Self::FrozenSet(fs) => {
                f.write_str("frozenset(")?;
                if !fs.is_empty() {
                    f.write_char('{')?;
                    let mut iter = fs.iter();
                    if let Some(first) = iter.next() {
                        first.repr_fmt(f)?;
                        for item in iter {
                            f.write_str(", ")?;
                            item.repr_fmt(f)?;
                        }
                    }
                    f.write_char('}')?;
                }
                f.write_char(')')
            }
            Self::Exception { exc_type, arg } => {
                let type_str: &'static str = exc_type.into();
                write!(f, "{type_str}(")?;

                if let Some(arg) = &arg {
                    f.write_str(&string_repr(arg))?;
                }
                f.write_char(')')
            }
            Self::Dataclass { name, fields, .. } => {
                // Format: ClassName(field1=value1, field2=value2, ...)
                f.write_str(name)?;
                f.write_char('(')?;
                let mut first = true;
                for (k, v) in fields.iter() {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    // For string keys, write without quotes
                    if let Self::String(s) = k {
                        f.write_str(s)?;
                    } else {
                        k.repr_fmt(f)?;
                    }
                    f.write_char('=')?;
                    v.repr_fmt(f)?;
                }
                f.write_char(')')
            }
            Self::Repr(s) => write!(f, "Repr({})", string_repr(s)),
            Self::Cycle(_, placeholder) => f.write_str(placeholder),
            Self::Type(t) => write!(f, "<class '{t}'>"),
        }
    }

    /// Returns `true` if this value is "truthy" according to Python's truth testing rules.
    ///
    /// In Python, the following values are considered falsy:
    /// - `None` and `Ellipsis`
    /// - `False`
    /// - Zero numeric values (`0`, `0.0`)
    /// - Empty sequences and collections (`""`, `b""`, `[]`, `()`, `{}`)
    ///
    /// All other values are truthy, including `Exception` and `Repr` variants.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::None => false,
            Self::Ellipsis => true,
            Self::Bool(b) => *b,
            Self::Int(i) => *i != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Bytes(b) => !b.is_empty(),
            Self::List(l) => !l.is_empty(),
            Self::Tuple(t) => !t.is_empty(),
            Self::Dict(d) => !d.is_empty(),
            Self::Set(s) => !s.is_empty(),
            Self::FrozenSet(fs) => !fs.is_empty(),
            Self::Exception { .. } => true,
            Self::Dataclass { .. } => true, // Dataclass instances are always truthy
            Self::Repr(_) => true,
            Self::Cycle(_, _) => true,
            Self::Type(_) => true,
        }
    }

    /// Returns the Python type name for this value (e.g., `"int"`, `"str"`, `"list"`).
    ///
    /// These are the same names returned by Python's `type(x).__name__`.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Ellipsis => "ellipsis",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Dict(_) => "dict",
            Self::Set(_) => "set",
            Self::FrozenSet(_) => "frozenset",
            Self::Exception { .. } => "Exception",
            Self::Dataclass { .. } => "dataclass",
            Self::Repr(_) => "repr",
            Self::Cycle(_, _) => "cycle",
            Self::Type(_) => "type",
        }
    }
}

impl Hash for MontyObject {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash the discriminant first
        std::mem::discriminant(self).hash(state);

        match self {
            Self::Ellipsis => {}
            Self::None => {}
            Self::Bool(bool) => bool.hash(state),
            Self::Int(i64) => i64.hash(state),
            Self::Float(f64) => f64.to_bits().hash(state),
            Self::String(string) => string.hash(state),
            Self::Bytes(bytes) => bytes.hash(state),
            Self::Type(t) => {
                let type_str: &'static str = t.into();
                type_str.hash(state);
            }
            Self::Cycle(_, _) => panic!("cycle values are not hashable"),
            _ => panic!("{} python values are not hashable", self.type_name()),
        }
    }
}

impl PartialEq for MontyObject {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ellipsis, Self::Ellipsis) => true,
            (Self::None, Self::None) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            // Use to_bits() for float comparison to be consistent with Hash
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            (Self::Set(a), Self::Set(b)) => a == b,
            (Self::FrozenSet(a), Self::FrozenSet(b)) => a == b,
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
            (
                Self::Dataclass {
                    name: a_name,
                    fields: a_fields,
                    methods: a_methods,
                    mutable: a_mutable,
                },
                Self::Dataclass {
                    name: b_name,
                    fields: b_fields,
                    methods: b_methods,
                    mutable: b_mutable,
                },
            ) => a_name == b_name && a_fields == b_fields && a_methods == b_methods && a_mutable == b_mutable,
            (Self::Repr(a), Self::Repr(b)) => a == b,
            (Self::Cycle(a, _), Self::Cycle(b, _)) => a == b,
            (Self::Type(a), Self::Type(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for MontyObject {}

impl AsRef<MontyObject> for MontyObject {
    fn as_ref(&self) -> &MontyObject {
        self
    }
}

/// Error returned when a `MontyObject` cannot be converted to the requested Rust type.
///
/// This error is returned by the `TryFrom` implementations when attempting to extract
/// a specific type from a `MontyObject` that holds a different variant.
#[derive(Debug)]
pub struct ConversionError {
    /// The type name that was expected (e.g., "int", "str").
    pub expected: &'static str,
    /// The actual type name of the `MontyObject` (e.g., "list", "NoneType").
    pub actual: &'static str,
}

impl ConversionError {
    /// Creates a new `ConversionError` with the expected and actual type names.
    #[must_use]
    pub fn new(expected: &'static str, actual: &'static str) -> Self {
        Self { expected, actual }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got {}", self.expected, self.actual)
    }
}

impl std::error::Error for ConversionError {}

/// Error returned when a `MontyObject` cannot be used as an input to code execution.
///
/// This can occur when:
/// - A `MontyObject` variant (like `Repr`) is only valid as an output, not an input
/// - A resource limit (memory, allocations) is exceeded during conversion
#[derive(Debug, Clone)]
pub enum InvalidInputError {
    /// The input type is not valid for conversion to a runtime Value.
    /// The type name of the invalid input value
    InvalidType(&'static str),
    /// A resource limit was exceeded during conversion.
    Resource(ResourceError),
}

impl InvalidInputError {
    /// Creates a new `InvalidInputError` for the given type name.
    #[must_use]
    pub fn invalid_type(type_name: &'static str) -> Self {
        Self::InvalidType(type_name)
    }
}

impl fmt::Display for InvalidInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType(type_name) => write!(f, "'{type_name}' is not a valid input value"),
            Self::Resource(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InvalidInputError {}

impl From<crate::resource::ResourceError> for InvalidInputError {
    fn from(err: crate::resource::ResourceError) -> Self {
        Self::Resource(err)
    }
}

/// Attempts to convert a MontyObject to an i64 integer.
/// Returns an error if the object is not an Int variant.
impl TryFrom<&MontyObject> for i64 {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, Self::Error> {
        match value {
            MontyObject::Int(i) => Ok(*i),
            _ => Err(ConversionError::new("int", value.type_name())),
        }
    }
}

/// Attempts to convert a MontyObject to an f64 float.
/// Returns an error if the object is not a Float or Int variant.
/// Int values are automatically converted to f64 to match python's behavior.
impl TryFrom<&MontyObject> for f64 {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, Self::Error> {
        match value {
            MontyObject::Float(f) => Ok(*f),
            MontyObject::Int(i) => Ok(*i as f64),
            _ => Err(ConversionError::new("float", value.type_name())),
        }
    }
}

/// Attempts to convert a MontyObject to a String.
/// Returns an error if the object is not a heap-allocated Str variant.
impl TryFrom<&MontyObject> for String {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, Self::Error> {
        if let MontyObject::String(s) = value {
            Ok(s.clone())
        } else {
            Err(ConversionError::new("str", value.type_name()))
        }
    }
}

/// Attempts to convert a `MontyObject` to a bool.
/// Returns an error if the object is not a True or False variant.
/// Note: This does NOT use Python's truthiness rules (use MontyObject::bool for that).
impl TryFrom<&MontyObject> for bool {
    type Error = ConversionError;

    fn try_from(value: &MontyObject) -> Result<Self, Self::Error> {
        match value {
            MontyObject::Bool(b) => Ok(*b),
            _ => Err(ConversionError::new("bool", value.type_name())),
        }
    }
}

/// A collection of key-value pairs representing Python dictionary contents.
///
/// Used internally by `MontyObject::Dict` to store dictionary entries while preserving
/// insertion order. Keys and values are both `MontyObject` instances.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DictPairs(Vec<(MontyObject, MontyObject)>);

impl From<Vec<(MontyObject, MontyObject)>> for DictPairs {
    fn from(pairs: Vec<(MontyObject, MontyObject)>) -> Self {
        DictPairs(pairs)
    }
}

impl From<IndexMap<MontyObject, MontyObject>> for DictPairs {
    fn from(map: IndexMap<MontyObject, MontyObject>) -> Self {
        DictPairs(map.into_iter().collect())
    }
}

impl From<DictPairs> for IndexMap<MontyObject, MontyObject> {
    fn from(pairs: DictPairs) -> Self {
        pairs.into_iter().collect()
    }
}

impl IntoIterator for DictPairs {
    type Item = (MontyObject, MontyObject);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
impl<'a> IntoIterator for &'a DictPairs {
    type Item = &'a (MontyObject, MontyObject);
    type IntoIter = std::slice::Iter<'a, (MontyObject, MontyObject)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl FromIterator<(MontyObject, MontyObject)> for DictPairs {
    fn from_iter<T: IntoIterator<Item = (MontyObject, MontyObject)>>(iter: T) -> Self {
        DictPairs(iter.into_iter().collect())
    }
}

impl DictPairs {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = &(MontyObject, MontyObject)> {
        self.0.iter()
    }
}
