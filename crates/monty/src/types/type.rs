use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    args::ArgValues,
    exception_private::{exc_fmt, ExcType, RunResult},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{Bytes, Dict, FrozenSet, List, PyTrait, Range, Set, Str, Tuple},
    value::Value,
};

/// Represents the Python type of a value.
///
/// This enum is used both for type checking and as a callable constructor.
/// When parsed from a string (e.g., "list", "dict"), it can be used to create
/// new instances of that type.
#[derive(
    Debug, Clone, Copy, Display, EnumString, IntoStaticStr, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[strum(serialize_all = "lowercase")]
#[allow(clippy::enum_variant_names)]
pub enum Type {
    Ellipsis,
    Type,
    #[strum(serialize = "NoneType")]
    NoneType,
    Bool,
    Int,
    Float,
    Range,
    Str,
    Bytes,
    List,
    Tuple,
    Dict,
    Set,
    #[strum(serialize = "frozenset")]
    FrozenSet,
    /// User-defined dataclass instance type.
    #[strum(disabled)]
    Dataclass,
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    #[strum(serialize = "builtin_function_or_method")]
    BuiltinFunction,
    Cell,
    #[strum(serialize = "iterator")]
    Iterator,
    /// used when we can't infer the type, this should be removed or very rare
    Unknown,
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

    /// Calls this type as a constructor (e.g., `list(x)`, `int(x)`).
    ///
    /// Dispatches to the appropriate type's init method for container types,
    /// or handles primitive type conversions inline.
    pub fn call(self, heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
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

            // Primitive types - inline implementation
            Self::Int => {
                let value = args.get_zero_one_arg("int")?;
                match value {
                    None => Ok(Value::Int(0)),
                    Some(v) => {
                        let result = match &v {
                            Value::Int(i) => Ok(Value::Int(*i)),
                            Value::Float(f) => Ok(Value::Int(f64_to_i64_truncate(*f))),
                            Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
                            _ => Err(ExcType::type_error_int_conversion(v.py_type(Some(heap)))),
                        };
                        v.drop_with_heap(heap);
                        result
                    }
                }
            }
            Self::Float => {
                let value = args.get_zero_one_arg("float")?;
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
                                _ => Err(ExcType::type_error_float_conversion(v.py_type(Some(heap)))),
                            },
                            _ => Err(ExcType::type_error_float_conversion(v.py_type(Some(heap)))),
                        };
                        v.drop_with_heap(heap);
                        result
                    }
                }
            }
            Self::Bool => {
                let value = args.get_zero_one_arg("bool")?;
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
fn value_error_could_not_convert_string_to_float(value: &str) -> crate::exception_private::RunError {
    exc_fmt!(ExcType::ValueError; "could not convert string to float: '{value}'").into()
}
