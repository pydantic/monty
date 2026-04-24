use std::{
    borrow::Cow,
    error::Error,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    mem, slice,
    vec::IntoIter,
};

use ahash::AHashSet;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta};
use indexmap::IndexMap;
use num_bigint::BigInt;
use num_traits::Zero;

use crate::{
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::VM,
    exception_private::{ExcType, RunError, SimpleException},
    heap::{HeapData, HeapId},
    resource::{ResourceError, ResourceTracker},
    types::{
        Dataclass, LongInt, NamedTuple, Path, PyTrait, TimeZone, Type, allocate_tuple,
        bytes::{Bytes, bytes_repr},
        date as date_type, datetime as datetime_type,
        dict::Dict,
        list::List,
        set::{FrozenSet, Set},
        str::{Str, StringRepr, string_repr_fmt},
        timedelta as timedelta_type, timezone as timezone_type,
    },
    value::{EitherStr, Value},
};

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
    pub offset_seconds: Option<i32>,
    /// Optional explicit timezone name for aware datetimes.
    ///
    /// Must be `None` when `offset_seconds` is `None`.
    pub timezone_name: Option<String>,
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

/// A Python `datetime.timezone` fixed-offset timezone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyTimeZone {
    /// Fixed UTC offset in seconds.
    pub offset_seconds: i32,
    /// Optional display name.
    pub name: Option<String>,
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
/// Only immutable variants implement `Hash`, including the datetime family
/// (`Date`, `DateTime`, `TimeDelta`, `TimeZone`). Attempting to hash mutable
/// variants (`List`, `Dict`) will panic.
///
/// # Serialization
///
/// `MontyObject` has two distinct serialization paths:
///
/// 1. **Derived serde (round-trippable)** — the default `Serialize` /
///    `Deserialize` impls use an externally tagged format
///    (`{"Int": 42}`, `{"String": "hi"}`, ...). This is what `postcard` and
///    `serde_json::to_string(&obj)` produce. It is lossless and designed for
///    snapshots and binary transport, not for human-facing JSON.
///
/// 2. **Natural JSON (output-only)** — wrap the value in
///    [`JsonMontyObject`](crate::JsonMontyObject) for a much more ergonomic
///    shape where JSON-native Python values serialize bare
///    (`42`, `"hi"`, `[...]`, `{"a": 1}`) and non-JSON-native values use a
///    `{"$<tag>": ...}` convention (`{"$tuple": [...]}`, `{"$bytes": [...]}`,
///    `{"$ellipsis": "..."}`, `{"$float": "nan"}`, ...). See the
///    `object_json` module docs for the full mapping. This form is
///    intentionally not round-trippable — use the derived format if you
///    need to reconstruct a `MontyObject` from JSON.
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
    /// Python arbitrary-precision integer (larger than i64).
    BigInt(BigInt),
    /// Python float (64-bit IEEE 754).
    Float(f64),
    /// Python string (UTF-8).
    String(String),
    /// Python bytes object.
    Bytes(Vec<u8>),
    /// Python list (mutable sequence).
    List(Vec<Self>),
    /// Python tuple (immutable sequence).
    Tuple(Vec<Self>),
    /// Python named tuple (immutable sequence with named fields).
    ///
    /// Named tuples behave like tuples but also support attribute access by field name.
    /// The type_name is used in repr (e.g., "os.stat_result"), and field_names provides
    /// the attribute names for each position.
    NamedTuple {
        /// Type name for repr (e.g., "os.stat_result").
        type_name: String,
        /// Field names in order.
        field_names: Vec<String>,
        /// Values in order (same length as field_names).
        values: Vec<Self>,
    },
    /// Python dictionary (insertion-ordered mapping).
    Dict(DictPairs),
    /// Python set (mutable, unordered collection of unique elements).
    Set(Vec<Self>),
    /// Python frozenset (immutable, unordered collection of unique elements).
    FrozenSet(Vec<Self>),
    /// Python `datetime.date`.
    Date(MontyDate),
    /// Python `datetime.datetime`.
    DateTime(MontyDateTime),
    /// Python `datetime.timedelta`.
    TimeDelta(MontyTimeDelta),
    /// Python `datetime.timezone` fixed-offset timezone.
    TimeZone(MontyTimeZone),
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
    BuiltinFunction(BuiltinsFunctions),
    /// Python `pathlib.Path` object (or technically a `PurePosixPath`).
    ///
    /// Represents a filesystem path. Can be used both as input (from host) and output.
    Path(String),
    /// A dataclass instance with class name, field names, attributes, and mutability.
    ///
    /// Method calls are detected lazily at runtime: when `call_attr` is invoked
    /// on a dataclass and the attribute name is not found in `attrs`, it is
    /// dispatched as a `MethodCall` to the host (provided the name is public).
    Dataclass {
        /// The class name (e.g., "Point", "User").
        name: String,
        /// Identifier of the type, from `id(type(dc))` in python.
        type_id: u64,
        /// Declared field names in definition order (for repr).
        field_names: Vec<String>,
        /// All attribute name -> value mapping (includes fields and extra attrs).
        attrs: DictPairs,
        /// Whether this dataclass instance is immutable.
        frozen: bool,
    },
    /// An external function provided by the host.
    ///
    /// Returned by the host in response to a `NameLookup` to provide a callable
    /// that the VM can invoke. When called, the VM yields `FunctionCall` to the host.
    Function {
        /// The function name (used for repr, error messages, and function call identification).
        name: String,
        /// Optional docstring for the function.
        docstring: Option<String>,
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
            Self::Function { name, .. } => write!(f, "<function '{name}' external>"),
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
    pub(crate) fn new(value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Self {
        let py_obj = Self::from_value(&value, vm);
        value.drop_with_heap(vm);
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
    pub(crate) fn to_value(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<Value, InvalidInputError> {
        match self {
            Self::Ellipsis => Ok(Value::Ellipsis),
            Self::None => Ok(Value::None),
            Self::Bool(b) => Ok(Value::Bool(b)),
            Self::Int(i) => Ok(Value::Int(i)),
            Self::BigInt(bi) => Ok(LongInt::new(bi).into_value(vm.heap)?),
            Self::Float(f) => Ok(Value::Float(f)),
            Self::String(s) => Ok(Value::Ref(vm.heap.allocate(HeapData::Str(Str::new(s)))?)),
            Self::Bytes(b) => Ok(Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(b)))?)),
            Self::List(items) => {
                let values: Vec<Value> = items
                    .into_iter()
                    .map(|item| item.to_value(vm))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(values)))?))
            }
            Self::Tuple(items) => {
                let values = items
                    .into_iter()
                    .map(|item| item.to_value(vm))
                    .collect::<Result<_, _>>()?;
                allocate_tuple(values, vm.heap).map_err(InvalidInputError::Resource)
            }
            Self::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                let values: Vec<Value> = values
                    .into_iter()
                    .map(|item| item.to_value(vm))
                    .collect::<Result<_, _>>()?;
                let field_name_strs: Vec<EitherStr> = field_names.into_iter().map(Into::into).collect();
                let nt = NamedTuple::new(type_name, field_name_strs, values);
                Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(nt))?))
            }
            Self::Dict(map) => {
                let pairs: Result<Vec<(Value, Value)>, InvalidInputError> = map
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_value(vm)?, v.to_value(vm)?)))
                    .collect();
                let dict = Dict::from_pairs(pairs?, vm)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable dict keys"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))?))
            }
            Self::Set(items) => {
                let mut set = Set::new();
                for item in items {
                    let value = item.to_value(vm)?;
                    set.add(value, vm)
                        .map_err(|_| InvalidInputError::invalid_type("unhashable set element"))?;
                }
                Ok(Value::Ref(vm.heap.allocate(HeapData::Set(set))?))
            }
            Self::FrozenSet(items) => {
                let mut set = Set::new();
                for item in items {
                    let value = item.to_value(vm)?;
                    set.add(value, vm)
                        .map_err(|_| InvalidInputError::invalid_type("unhashable frozenset element"))?;
                }
                // Convert to frozenset by extracting storage
                let frozenset = FrozenSet::from_set(set);
                Ok(Value::Ref(vm.heap.allocate(HeapData::FrozenSet(frozenset))?))
            }
            Self::Date(date) => {
                let value = date_type::from_ymd(date.year, i32::from(date.month), i32::from(date.day))
                    .map_err(|_| InvalidInputError::invalid_type("date"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Date(value))?))
            }
            Self::DateTime(datetime) => {
                let MontyDateTime {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    microsecond,
                    offset_seconds,
                    timezone_name,
                } = datetime;
                if offset_seconds.is_none() && timezone_name.is_some() {
                    return Err(InvalidInputError::invalid_type("datetime"));
                }
                let tzinfo = offset_seconds
                    .map(|offset| TimeZone::new(offset, timezone_name))
                    .transpose()
                    .map_err(|_| InvalidInputError::invalid_type("datetime"))?;
                let value = datetime_type::from_components(
                    year,
                    i32::from(month),
                    i32::from(day),
                    i32::from(hour),
                    i32::from(minute),
                    i32::from(second),
                    i32::try_from(microsecond).map_err(|_| InvalidInputError::invalid_type("datetime"))?,
                    tzinfo,
                    None,
                    vm.heap,
                )
                .map_err(|_| InvalidInputError::invalid_type("datetime"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(value))?))
            }
            Self::TimeDelta(delta) => {
                let delta = timedelta_type::new(delta.days, delta.seconds, delta.microseconds)
                    .map_err(|_| InvalidInputError::invalid_type("timedelta"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?))
            }
            Self::TimeZone(tz) => {
                if tz.offset_seconds == 0 && tz.name.is_none() {
                    vm.heap
                        .get_timezone_utc()
                        .map_err(|_| InvalidInputError::invalid_type("timezone"))
                } else {
                    let tz = TimeZone::new(tz.offset_seconds, tz.name)
                        .map_err(|_| InvalidInputError::invalid_type("timezone"))?;
                    Ok(Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))?))
                }
            }
            Self::Exception { exc_type, arg } => {
                let exc = SimpleException::new(exc_type, arg);
                Ok(Value::Ref(vm.heap.allocate(HeapData::Exception(exc))?))
            }
            Self::Dataclass {
                name,
                type_id,
                field_names,
                attrs,
                frozen,
            } => {
                // Convert attrs to Dict
                let pairs: Result<Vec<(Value, Value)>, InvalidInputError> = attrs
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_value(vm)?, v.to_value(vm)?)))
                    .collect();
                let dict = Dict::from_pairs(pairs?, vm)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable dataclass attr keys"))?;
                let dc = Dataclass::new(name, type_id, field_names, dict, frozen);
                Ok(Value::Ref(vm.heap.allocate(HeapData::Dataclass(dc))?))
            }
            Self::Path(s) => Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(s)))?)),
            Self::Type(t) => Ok(Value::Builtin(Builtins::Type(t))),
            Self::BuiltinFunction(f) => Ok(Value::Builtin(Builtins::Function(f))),
            Self::Function { name, .. } => {
                // Try to intern the function name. If the name is already interned
                // (common case: the function has the same name as the variable it was
                // assigned to), use the lightweight `Value::ExtFunction(StringId)`.
                // Otherwise, allocate a `HeapData::ExtFunction(String)` on the heap.
                if let Some(string_id) = vm.interns.get_string_id_by_name(&name) {
                    Ok(Value::ExtFunction(string_id))
                } else {
                    Ok(Value::Ref(vm.heap.allocate(HeapData::ExtFunction(name))?))
                }
            }
            Self::Repr(_) => Err(InvalidInputError::invalid_type("'Repr' is not a valid input value")),
            Self::Cycle(_, _) => Err(InvalidInputError::invalid_type("'Cycle' is not a valid input value")),
        }
    }

    fn from_value(object: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> Self {
        let mut visited = AHashSet::new();
        Self::from_value_inner(object, vm, &mut visited)
    }

    /// Internal helper for converting Value to MontyObject with cycle detection.
    ///
    /// The `visited` set tracks HeapIds we're currently processing. When we encounter
    /// a HeapId already in the set, we've found a cycle and return `MontyObject::Cycle`
    /// with an appropriate placeholder string.
    ///
    /// Recursion depth is tracked via `heap.incr_recursion_depth_for_repr()`.
    fn from_value_inner(object: &Value, vm: &VM<'_, '_, impl ResourceTracker>, visited: &mut AHashSet<HeapId>) -> Self {
        // Check depth limit before processing
        let Some(token) = vm.heap.incr_recursion_depth_for_repr() else {
            return Self::Repr("<deeply nested>".to_owned());
        };
        crate::defer_drop_immutable_heap!(token, vm);
        match object {
            Value::Undefined => panic!("Undefined found while converting to MontyObject"),
            Value::Ellipsis => Self::Ellipsis,
            Value::None => Self::None,
            Value::Bool(b) => Self::Bool(*b),
            Value::Int(i) => Self::Int(*i),
            Value::Float(f) => Self::Float(*f),
            Value::InternString(string_id) => Self::String(vm.interns.get_str(*string_id).to_owned()),
            Value::InternBytes(bytes_id) => Self::Bytes(vm.interns.get_bytes(*bytes_id).to_owned()),
            Value::Ref(id) => {
                // Check for cycle
                if visited.contains(id) {
                    // Cycle detected - return appropriate placeholder
                    return match vm.heap.get(*id) {
                        HeapData::List(_) => Self::Cycle(*id, "[...]".to_owned()),
                        HeapData::Tuple(_) | HeapData::NamedTuple(_) => Self::Cycle(*id, "(...)".to_owned()),
                        HeapData::Dict(_) => Self::Cycle(*id, "{...}".to_owned()),
                        _ => Self::Cycle(*id, "...".to_owned()),
                    };
                }

                // Mark this id as being visited
                visited.insert(*id);

                let result = match vm.heap.get(*id) {
                    HeapData::Str(s) => Self::String(s.as_str().to_owned()),
                    HeapData::Bytes(b) => Self::Bytes(b.as_slice().to_owned()),
                    HeapData::List(list) => Self::List(
                        list.as_slice()
                            .iter()
                            .map(|obj| Self::from_value_inner(obj, vm, visited))
                            .collect(),
                    ),
                    HeapData::Tuple(tuple) => Self::Tuple(
                        tuple
                            .as_slice()
                            .iter()
                            .map(|obj| Self::from_value_inner(obj, vm, visited))
                            .collect(),
                    ),
                    HeapData::NamedTuple(nt) => Self::NamedTuple {
                        type_name: nt.name(vm.interns).to_owned(),
                        field_names: nt
                            .field_names()
                            .iter()
                            .map(|field_name| field_name.as_str(vm.interns).to_owned())
                            .collect(),
                        values: nt
                            .as_vec()
                            .iter()
                            .map(|obj| Self::from_value_inner(obj, vm, visited))
                            .collect(),
                    },
                    HeapData::Dict(dict) => Self::Dict(DictPairs(
                        dict.into_iter()
                            .map(|(k, v)| {
                                (
                                    Self::from_value_inner(k, vm, visited),
                                    Self::from_value_inner(v, vm, visited),
                                )
                            })
                            .collect(),
                    )),
                    HeapData::Set(set) => Self::Set(
                        set.storage()
                            .iter()
                            .map(|obj| Self::from_value_inner(obj, vm, visited))
                            .collect(),
                    ),
                    HeapData::FrozenSet(frozenset) => Self::FrozenSet(
                        frozenset
                            .storage()
                            .iter()
                            .map(|obj| Self::from_value_inner(obj, vm, visited))
                            .collect(),
                    ),
                    HeapData::Date(date) => {
                        let (year, month, day) = date_type::to_ymd(*date);
                        Self::Date(MontyDate {
                            year,
                            month: u8::try_from(month).expect("month is always 1..=12"),
                            day: u8::try_from(day).expect("day is always 1..=31"),
                        })
                    }
                    HeapData::DateTime(datetime) => {
                        if let Some((year, month, day, hour, minute, second, microsecond)) =
                            datetime_type::to_components(datetime)
                        {
                            Self::DateTime(MontyDateTime {
                                year,
                                month,
                                day,
                                hour,
                                minute,
                                second,
                                microsecond,
                                offset_seconds: datetime_type::offset_seconds(datetime),
                                timezone_name: datetime_type::timezone_info(datetime).and_then(|tz| tz.name),
                            })
                        } else {
                            repr_or_error(object, vm)
                        }
                    }
                    HeapData::TimeDelta(delta) => {
                        let (days, seconds, microseconds) = timedelta_type::components(delta);
                        Self::TimeDelta(MontyTimeDelta {
                            days,
                            seconds,
                            microseconds,
                        })
                    }
                    HeapData::TimeZone(tz) => Self::TimeZone(MontyTimeZone {
                        offset_seconds: tz.offset_seconds,
                        name: tz.name.clone(),
                    }),
                    // Cells are internal closure implementation details
                    HeapData::Cell(cell) => {
                        // Show the cell's contents
                        Self::from_value_inner(&cell.0, vm, visited)
                    }
                    HeapData::Closure(..) | HeapData::FunctionDefaults(..) => repr_or_error(object, vm),
                    HeapData::Range(_) => repr_or_error(object, vm),
                    HeapData::Exception(exc) => Self::Exception {
                        exc_type: exc.exc_type(),
                        arg: exc.arg().map(ToString::to_string),
                    },
                    HeapData::Dataclass(dc) => {
                        // Convert attrs to DictPairs
                        let attrs = DictPairs(
                            dc.attrs()
                                .into_iter()
                                .map(|(k, v)| {
                                    (
                                        Self::from_value_inner(k, vm, visited),
                                        Self::from_value_inner(v, vm, visited),
                                    )
                                })
                                .collect(),
                        );
                        Self::Dataclass {
                            name: dc.name(vm.interns).to_owned(),
                            type_id: dc.type_id(),
                            field_names: dc.field_names().to_vec(),
                            attrs,
                            frozen: dc.is_frozen(),
                        }
                    }
                    HeapData::Iter(_) => {
                        // Iterators are internal objects - represent as a type string
                        Self::Repr("<iterator>".to_owned())
                    }
                    HeapData::DictKeysView(_) | HeapData::DictItemsView(_) | HeapData::DictValuesView(_) => {
                        repr_or_error(object, vm)
                    }
                    HeapData::LongInt(li) => Self::BigInt(li.inner().clone()),
                    HeapData::Module(m) => {
                        // Modules are represented as a repr string
                        Self::Repr(format!("<module '{}'>", vm.interns.get_str(m.name())))
                    }
                    HeapData::Slice(_) => repr_or_error(object, vm),
                    HeapData::Coroutine(coro) => {
                        // Coroutines are represented as a repr string
                        let func = vm.interns.get_function(coro.func_id);
                        let name = vm.interns.get_str(func.name.name_id);
                        Self::Repr(format!("<coroutine object {name}>"))
                    }
                    HeapData::GatherFuture(gather) => {
                        // GatherFutures are represented as a repr string
                        Self::Repr(format!("<gather({})>", gather.item_count()))
                    }
                    HeapData::Path(path) => Self::Path(path.as_str().to_owned()),
                    HeapData::RePattern(_) | HeapData::ReMatch(_) => repr_or_error(object, vm),
                    HeapData::ExtFunction(name) => Self::Function {
                        name: name.clone(),
                        docstring: None,
                    },
                };

                // Remove from visited set after processing
                visited.remove(id);
                result
            }
            Value::Builtin(Builtins::Type(t)) => Self::Type(*t),
            Value::Builtin(Builtins::ExcType(e)) => Self::Type(Type::Exception(*e)),
            Value::Builtin(Builtins::Function(f)) => Self::BuiltinFunction(*f),
            #[cfg(feature = "memory-model-checks")]
            Value::Dereferenced => panic!("Dereferenced found while converting to MontyObject"),
            _ => repr_or_error(object, vm),
        }
    }
}

/// Converts a value to its repr string for `MontyObject`, falling back to a
/// descriptive error message if `py_repr` fails (e.g. INT_MAX_STR_DIGITS).
fn repr_or_error(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> MontyObject {
    match value.py_repr(vm) {
        Ok(s) => MontyObject::Repr(s.into_owned()),
        Err(e) => {
            let ty = value.py_type(vm);
            let msg = match &e {
                RunError::Internal(s) => s.to_string(),
                RunError::Exc(exc) | RunError::UncatchableExc(exc) => exc.exc.to_string(),
            };
            MontyObject::Repr(format!("<{ty} object, error on repr(): {msg}>"))
        }
    }
}

impl MontyObject {
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
            Self::BigInt(v) => write!(f, "{v}"),
            Self::Float(v) => {
                let s = v.to_string();
                f.write_str(&s)?;
                if !s.contains('.') {
                    f.write_str(".0")?;
                }
                Ok(())
            }
            Self::String(s) => string_repr_fmt(s, f),
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
            Self::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                // Format: type_name(field1=value1, field2=value2, ...)
                f.write_str(type_name)?;
                f.write_char('(')?;
                let mut first = true;
                for (name, value) in field_names.iter().zip(values) {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    f.write_str(name)?;
                    f.write_char('=')?;
                    value.repr_fmt(f)?;
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
            Self::Date(date) => write!(f, "datetime.date({}, {}, {})", date.year, date.month, date.day),
            Self::DateTime(datetime) => {
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
                    if offset == 0 && datetime.timezone_name.is_none() {
                        f.write_str(", tzinfo=datetime.timezone.utc")?;
                    } else {
                        let timedelta_repr = timezone_type::format_offset_timedelta_repr(offset);
                        write!(f, ", tzinfo=datetime.timezone({timedelta_repr}")?;
                        if let Some(name) = &datetime.timezone_name {
                            write!(f, ", {}", StringRepr(name))?;
                        }
                        f.write_char(')')?;
                    }
                }
                f.write_char(')')
            }
            Self::TimeDelta(delta) => {
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
            Self::TimeZone(tz) => {
                if tz.offset_seconds == 0 && tz.name.is_none() {
                    return f.write_str("datetime.timezone.utc");
                }
                let timedelta_repr = timezone_type::format_offset_timedelta_repr(tz.offset_seconds);
                write!(f, "datetime.timezone({timedelta_repr}")?;
                if let Some(name) = &tz.name {
                    write!(f, ", {}", StringRepr(name))?;
                }
                f.write_char(')')
            }
            Self::Exception { exc_type, arg } => {
                let type_str: &'static str = exc_type.into();
                write!(f, "{type_str}(")?;

                if let Some(arg) = &arg {
                    string_repr_fmt(arg, f)?;
                }
                f.write_char(')')
            }
            Self::Dataclass {
                name,
                field_names,
                attrs,
                ..
            } => {
                // Format: ClassName(field1=value1, field2=value2, ...)
                // Only declared fields are shown, not extra attributes
                f.write_str(name)?;
                f.write_char('(')?;
                let mut first = true;
                for field_name in field_names {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    f.write_str(field_name)?;
                    f.write_char('=')?;
                    // Look up value in attrs
                    let key = Self::String(field_name.clone());
                    if let Some(value) = attrs.iter().find(|(k, _)| k == &key).map(|(_, v)| v) {
                        value.repr_fmt(f)?;
                    } else {
                        f.write_str("<?>")?;
                    }
                }
                f.write_char(')')
            }
            Self::Path(p) => write!(f, "PosixPath('{p}')"),
            Self::Type(t) => write!(f, "<class '{t}'>"),
            Self::BuiltinFunction(func) => write!(f, "<built-in function {func}>"),
            Self::Function { name, .. } => write!(f, "<function '{name}' external>"),
            Self::Repr(s) => write!(f, "Repr({})", StringRepr(s)),
            Self::Cycle(_, placeholder) => f.write_str(placeholder),
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
            Self::BigInt(bi) => !bi.is_zero(),
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Bytes(b) => !b.is_empty(),
            Self::List(l) => !l.is_empty(),
            Self::Tuple(t) => !t.is_empty(),
            Self::NamedTuple { values, .. } => !values.is_empty(),
            Self::Dict(d) => !d.is_empty(),
            Self::Set(s) => !s.is_empty(),
            Self::FrozenSet(fs) => !fs.is_empty(),
            Self::Date(_) => true,
            Self::DateTime(_) => true,
            Self::TimeDelta(delta) => delta.days != 0 || delta.seconds != 0 || delta.microseconds != 0,
            Self::TimeZone(_) => true,
            Self::Exception { .. } => true,
            Self::Path(_) => true,          // Path instances are always truthy
            Self::Dataclass { .. } => true, // Dataclass instances are always truthy
            Self::Type(_) | Self::BuiltinFunction(_) | Self::Function { .. } | Self::Repr(_) | Self::Cycle(_, _) => {
                true
            }
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
            Self::Int(_) | Self::BigInt(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::NamedTuple { .. } => "namedtuple",
            Self::Dict(_) => "dict",
            Self::Set(_) => "set",
            Self::FrozenSet(_) => "frozenset",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
            Self::TimeDelta(_) => "timedelta",
            Self::TimeZone(_) => "timezone",
            Self::Exception { .. } => "Exception",
            Self::Path(_) => "PosixPath",
            Self::Dataclass { .. } => "dataclass",
            Self::Type(_) => "type",
            Self::BuiltinFunction(_) => "builtin_function_or_method",
            Self::Function { .. } => "function",
            Self::Repr(_) => "repr",
            Self::Cycle(_, _) => "cycle",
        }
    }
}

impl Hash for MontyObject {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash the discriminant first (but Int and BigInt share discriminant for consistency)
        match self {
            Self::Int(_) | Self::BigInt(_) => {
                // Use Int discriminant for both to maintain hash consistency
                mem::discriminant(&Self::Int(0)).hash(state);
            }
            _ => mem::discriminant(self).hash(state),
        }

        match self {
            Self::Ellipsis | Self::None => {}
            Self::Bool(bool) => bool.hash(state),
            Self::Int(i) => i.hash(state),
            Self::BigInt(bi) => {
                // For hash consistency, if BigInt fits in i64, hash as i64
                if let Ok(i) = i64::try_from(bi) {
                    i.hash(state);
                } else {
                    // For large BigInts, hash the signed bytes
                    bi.to_signed_bytes_le().hash(state);
                }
            }
            Self::Float(f) => f.to_bits().hash(state),
            Self::String(string) => string.hash(state),
            Self::Bytes(bytes) => bytes.hash(state),
            Self::Date(date) => date.hash(state),
            Self::DateTime(datetime) => datetime.hash(state),
            Self::TimeDelta(delta) => delta.hash(state),
            Self::TimeZone(timezone) => timezone.hash(state),
            Self::Path(path) => path.hash(state),
            Self::Type(t) => t.to_string().hash(state),
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
            (Self::BigInt(a), Self::BigInt(b)) => a == b,
            // Cross-compare Int and BigInt
            (Self::Int(a), Self::BigInt(b)) | (Self::BigInt(b), Self::Int(a)) => BigInt::from(*a) == *b,
            // Use to_bits() for float comparison to be consistent with Hash
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            (Self::TimeDelta(a), Self::TimeDelta(b)) => a == b,
            (Self::TimeZone(a), Self::TimeZone(b)) => a == b,
            (
                Self::NamedTuple {
                    type_name: a_type,
                    field_names: a_fields,
                    values: a_values,
                },
                Self::NamedTuple {
                    type_name: b_type,
                    field_names: b_fields,
                    values: b_values,
                },
            ) => a_type == b_type && a_fields == b_fields && a_values == b_values,
            // NamedTuple can compare with Tuple by values only (matching Python semantics)
            (Self::NamedTuple { values, .. }, Self::Tuple(t)) | (Self::Tuple(t), Self::NamedTuple { values, .. }) => {
                values == t
            }
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
                    type_id: a_type_id,
                    field_names: a_field_names,
                    attrs: a_attrs,
                    frozen: a_frozen,
                },
                Self::Dataclass {
                    name: b_name,
                    type_id: b_type_id,
                    field_names: b_field_names,
                    attrs: b_attrs,
                    frozen: b_frozen,
                },
            ) => {
                a_name == b_name
                    && a_type_id == b_type_id
                    && a_field_names == b_field_names
                    && a_attrs == b_attrs
                    && a_frozen == b_frozen
            }
            (Self::Path(a), Self::Path(b)) => a == b,
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
            (Self::Repr(a), Self::Repr(b)) => a == b,
            (Self::Cycle(a, _), Self::Cycle(b, _)) => a == b,
            (Self::Type(a), Self::Type(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for MontyObject {}

impl AsRef<Self> for MontyObject {
    fn as_ref(&self) -> &Self {
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

impl Error for ConversionError {}

/// Error returned when a `MontyObject` cannot be used as an input to code execution.
///
/// This can occur when:
/// - A `MontyObject` variant (like `Repr`) is only valid as an output, not an input
/// - A resource limit (memory, allocations) is exceeded during conversion
#[derive(Debug, Clone)]
pub enum InvalidInputError {
    /// The input type is not valid for conversion to a runtime Value.
    /// Message explaining why the type is invalid.
    InvalidType(Cow<'static, str>),
    /// A resource limit was exceeded during conversion.
    Resource(ResourceError),
}

impl InvalidInputError {
    /// Creates a new `InvalidInputError` for the given type name.
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
            MontyObject::Int(i) => Ok(*i as Self),
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
        Self(pairs)
    }
}

impl From<IndexMap<MontyObject, MontyObject>> for DictPairs {
    fn from(map: IndexMap<MontyObject, MontyObject>) -> Self {
        Self(map.into_iter().collect())
    }
}

impl From<DictPairs> for IndexMap<MontyObject, MontyObject> {
    fn from(pairs: DictPairs) -> Self {
        pairs.into_iter().collect()
    }
}

impl IntoIterator for DictPairs {
    type Item = (MontyObject, MontyObject);
    type IntoIter = IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
impl<'a> IntoIterator for &'a DictPairs {
    type Item = &'a (MontyObject, MontyObject);
    type IntoIter = slice::Iter<'a, (MontyObject, MontyObject)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl FromIterator<(MontyObject, MontyObject)> for DictPairs {
    fn from_iter<T: IntoIterator<Item = (MontyObject, MontyObject)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl DictPairs {
    /// Number of (key, value) pairs held by this dict.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether this dict has no pairs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = &(MontyObject, MontyObject)> {
        self.0.iter()
    }
}
