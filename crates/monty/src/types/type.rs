use std::fmt;

use strum::EnumString;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{Bytes, Dict, FrozenSet, List, MontyIter, PyTrait, Range, Set, Slice, Str, Tuple, str::StringRepr},
    value::Value,
};

/// Represents the Python type of a value.
///
/// This enum is used both for type checking and as a callable constructor.
/// When parsed from a string (e.g., "list", "dict"), it can be used to create
/// new instances of that type.
///
/// Note: `Exception` variants is disabled for strum's `EnumString` (they can't be parsed from strings).
#[derive(Debug, Clone, Copy, EnumString, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
#[expect(clippy::enum_variant_names)]
pub enum Type {
    Ellipsis,
    Type,
    NoneType,
    Bool,
    Int,
    Float,
    Range,
    Slice,
    Str,
    Bytes,
    List,
    Tuple,
    NamedTuple,
    Dict,
    Set,
    FrozenSet,
    Dataclass,
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    BuiltinFunction,
    Cell,
    #[strum(serialize = "iter")]
    Iterator,
    Module,
    /// Marker types like stdout/stderr - displays as "TextIOWrapper"
    #[strum(serialize = "TextIOWrapper")]
    TextIOWrapper,
    /// typing module special forms (Any, Optional, Union, etc.) - displays as "typing._SpecialForm"
    #[strum(serialize = "typing._SpecialForm")]
    SpecialForm,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ellipsis => f.write_str("ellipsis"),
            Self::Type => f.write_str("type"),
            Self::NoneType => f.write_str("NoneType"),
            Self::Bool => f.write_str("bool"),
            Self::Int => f.write_str("int"),
            Self::Float => f.write_str("float"),
            Self::Range => f.write_str("range"),
            Self::Slice => f.write_str("slice"),
            Self::Str => f.write_str("str"),
            Self::Bytes => f.write_str("bytes"),
            Self::List => f.write_str("list"),
            Self::Tuple => f.write_str("tuple"),
            Self::NamedTuple => f.write_str("namedtuple"),
            Self::Dict => f.write_str("dict"),
            Self::Set => f.write_str("set"),
            Self::FrozenSet => f.write_str("frozenset"),
            Self::Dataclass => f.write_str("dataclass"),
            Self::Exception(exc_type) => write!(f, "{exc_type}"),
            Self::Function => f.write_str("function"),
            Self::BuiltinFunction => f.write_str("builtin_function_or_method"),
            Self::Cell => f.write_str("cell"),
            Self::Iterator => f.write_str("iterator"),
            Self::Module => f.write_str("module"),
            Self::TextIOWrapper => f.write_str("_io.TextIOWrapper"),
            Self::SpecialForm => f.write_str("typing._SpecialForm"),
        }
    }
}

impl Type {
    /// Checks if a value of type `self` is an instance of `other`.
    ///
    /// This handles Python's subtype relationships:
    /// - `bool` is a subtype of `int` (so `isinstance(True, int)` returns True)
    #[must_use]
    pub fn is_instance_of(self, other: Self) -> bool {
        if self == other {
            true
        } else if self == Self::Bool && other == Self::Int {
            // bool is a subtype of int in Python
            true
        } else {
            false
        }
    }

    /// Converts a callable type to a u8 for the `CallBuiltinType` opcode.
    ///
    /// Returns `Some(u8)` for types that can be called as constructors,
    /// `None` for non-callable types.
    #[must_use]
    pub fn callable_to_u8(self) -> Option<u8> {
        match self {
            Self::Bool => Some(0),
            Self::Int => Some(1),
            Self::Float => Some(2),
            Self::Str => Some(3),
            Self::Bytes => Some(4),
            Self::List => Some(5),
            Self::Tuple => Some(6),
            Self::Dict => Some(7),
            Self::Set => Some(8),
            Self::FrozenSet => Some(9),
            Self::Range => Some(10),
            Self::Slice => Some(11),
            Self::Iterator => Some(12),
            _ => None,
        }
    }

    /// Converts a u8 back to a callable `Type` for the `CallBuiltinType` opcode.
    ///
    /// Returns `Some(Type)` for valid callable type IDs, `None` otherwise.
    #[must_use]
    pub fn callable_from_u8(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Bool),
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::Str),
            4 => Some(Self::Bytes),
            5 => Some(Self::List),
            6 => Some(Self::Tuple),
            7 => Some(Self::Dict),
            8 => Some(Self::Set),
            9 => Some(Self::FrozenSet),
            10 => Some(Self::Range),
            11 => Some(Self::Slice),
            12 => Some(Self::Iterator),
            _ => None,
        }
    }

    /// Calls this type as a constructor (e.g., `list(x)`, `int(x)`).
    ///
    /// Dispatches to the appropriate type's init method for container types,
    /// or handles primitive type conversions inline.
    pub(crate) fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match self {
            // Container types - delegate to init methods
            Self::List => List::init(heap, args, interns),
            Self::Tuple => Tuple::init(heap, args, interns),
            Self::Dict => Dict::init(heap, args, interns),
            Self::Set => Set::init(heap, args, interns),
            Self::FrozenSet => FrozenSet::init(heap, args, interns),
            Self::Str => Str::init(heap, args, interns),
            Self::Bytes => Bytes::init(heap, args, interns),
            Self::Range => Range::init(heap, args),
            Self::Slice => Slice::init(heap, args),
            Self::Iterator => MontyIter::init(heap, args, interns),

            // Primitive types - inline implementation
            Self::Int => {
                let value = args.get_zero_one_arg("int", heap)?;
                match value {
                    None => Ok(Value::Int(0)),
                    Some(v) => {
                        let result = match &v {
                            Value::Int(i) => Ok(Value::Int(*i)),
                            Value::Float(f) => Ok(Value::Int(f64_to_i64_truncate(*f))),
                            Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
                            Value::InternString(string_id) => parse_i64_from_str(interns.get_str(*string_id)),
                            Value::Ref(heap_id) => match heap.get(*heap_id) {
                                HeapData::Str(s) => parse_i64_from_str(s.as_str()),
                                _ => Err(ExcType::type_error_int_conversion(v.py_type(heap))),
                            },
                            _ => Err(ExcType::type_error_int_conversion(v.py_type(heap))),
                        };
                        v.drop_with_heap(heap);
                        result
                    }
                }
            }
            Self::Float => {
                let value = args.get_zero_one_arg("float", heap)?;
                match value {
                    None => Ok(Value::Float(0.0)),
                    Some(v) => {
                        let result = match &v {
                            Value::Float(f) => Ok(Value::Float(*f)),
                            Value::Int(i) => Ok(Value::Float(*i as f64)),
                            Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                            Value::InternString(string_id) => {
                                Ok(Value::Float(parse_f64_from_str(interns.get_str(*string_id))?))
                            }
                            Value::Ref(heap_id) => match heap.get(*heap_id) {
                                HeapData::Str(s) => Ok(Value::Float(parse_f64_from_str(s.as_str())?)),
                                _ => Err(ExcType::type_error_float_conversion(v.py_type(heap))),
                            },
                            _ => Err(ExcType::type_error_float_conversion(v.py_type(heap))),
                        };
                        v.drop_with_heap(heap);
                        result
                    }
                }
            }
            Self::Bool => {
                let value = args.get_zero_one_arg("bool", heap)?;
                match value {
                    None => Ok(Value::Bool(false)),
                    Some(v) => {
                        let result = v.py_bool(heap, interns);
                        v.drop_with_heap(heap);
                        Ok(Value::Bool(result))
                    }
                }
            }

            // Non-callable types - raise TypeError
            _ => Err(ExcType::type_error_not_callable(self)),
        }
    }
}

/// Truncates f64 to i64 with clamping for out-of-range values.
///
/// Python's `int(float)` truncates toward zero. For values outside i64 range,
/// we clamp to i64::MAX/MIN (Python would use arbitrary precision ints, which
/// we don't support).
fn f64_to_i64_truncate(value: f64) -> i64 {
    // trunc() rounds toward zero, matching Python's int(float) behavior
    let truncated = value.trunc();
    if truncated >= i64::MAX as f64 {
        i64::MAX
    } else if truncated <= i64::MIN as f64 {
        i64::MIN
    } else {
        // SAFETY for clippy: truncated is guaranteed to be in (i64::MIN, i64::MAX)
        // after the bounds checks above, so truncation cannot overflow
        #[expect(clippy::cast_possible_truncation, reason = "bounds checked above")]
        let result = truncated as i64;
        result
    }
}

/// Parses a Python `float()` string argument into an `f64`.
///
/// This supports:
/// - Leading/trailing whitespace (e.g. `"  1.5  "`)
/// - The special values `inf`, `-inf`, `infinity`, and `nan` (case-insensitive)
///
/// Underscore digit separators are not currently supported.
fn parse_f64_from_str(value: &str) -> RunResult<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(value_error_could_not_convert_string_to_float(value));
    }

    let lower = trimmed.to_ascii_lowercase();
    let parsed = match lower.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => f64::INFINITY,
        "-inf" | "-infinity" => f64::NEG_INFINITY,
        "nan" | "+nan" => f64::NAN,
        "-nan" => -f64::NAN,
        _ => trimmed
            .parse::<f64>()
            .map_err(|_| value_error_could_not_convert_string_to_float(value))?,
    };

    Ok(parsed)
}

/// Creates the `ValueError` raised by `float()` when a string cannot be parsed.
///
/// Matches CPython's message format: `could not convert string to float: '...'`.
fn value_error_could_not_convert_string_to_float(value: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("could not convert string to float: {}", StringRepr(value)),
    )
    .into()
}

/// Parses a Python `int()` string argument into an `i64`.
///
/// Handles whitespace stripping and removing _ and returns appropriate `ValueError` on failure.
fn parse_i64_from_str(value: &str) -> RunResult<Value> {
    if let Ok(int) = value.parse::<i64>() {
        return Ok(Value::Int(int));
    }
    let trimmed = value.trim();

    if let Ok(int) = trimmed.parse::<i64>() {
        Ok(Value::Int(int))
    } else if let Ok(int) = trimmed.replace('_', "").parse::<i64>() {
        Ok(Value::Int(int))
    } else {
        Err(value_error_invalid_literal_for_int(value))
    }
}

/// Creates the `ValueError` raised by `int()` when a string cannot be parsed.
///
/// Matches CPython's message format: `invalid literal for int() with base 10: '...'`.
fn value_error_invalid_literal_for_int(value: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("invalid literal for int() with base 10: {}", StringRepr(value)),
    )
    .into()
}
