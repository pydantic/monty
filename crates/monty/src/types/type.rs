use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
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
                            Value::Float(f) => Ok(Value::Int(*f as i64)),
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
