//! Conversion between component-model node arenas and [`MontyGraph`].
//!
//! WIT cannot express recursive value types, so the component boundary uses
//! the same flat post-order arena the wire does: one per message, container
//! nodes holding the indexes of their (lower-numbered) children. Conversion
//! is node for node; the arena's invariants are checked by
//! [`MontyGraph::from_nodes`]. Protobuf remains an internal detail of
//! `monty-proto`; no wire bytes cross into JavaScript.

use std::borrow::Cow;

use monty_proto::DEFAULT_MAX_DECODE_BYTES;
use monty_types::{
    BuiltinsFunctions, ClassTypeNode as MontyClassTypeNode, FileMode, MontyDate, MontyDateTime, MontyFileHandle,
    MontyGraph, MontyNode, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NodeId,
};

use crate::bindings::exports::pydantic::monty::worker::{
    Arena, ClassInstanceNode, ClassTypeNode, DateNode, DatetimeNode, ExceptionValueNode, FileHandleNode, FunctionNode,
    NamedTupleNode, NodePair, TimeNode, TimedeltaNode, TimezoneNode, ValueNode,
};

/// Remaining expanded-value allowance for one request's arena.
pub struct DecodeBudget {
    remaining: usize,
}

impl Default for DecodeBudget {
    fn default() -> Self {
        Self {
            remaining: DEFAULT_MAX_DECODE_BYTES,
        }
    }
}

impl DecodeBudget {
    /// Charges an arena before conversion allocates its nodes.
    fn charge(&mut self, nodes: &[ValueNode]) -> Result<usize, String> {
        let bytes = nodes
            .iter()
            .fold(0usize, |total, node| total.saturating_add(node_host_size(node)));
        if let Some(remaining) = self.remaining.checked_sub(bytes) {
            self.remaining = remaining;
            Ok(bytes)
        } else {
            Err("component request values exceed the host-memory budget".to_owned())
        }
    }
}

/// Converts one component arena into a validated Monty arena.
pub fn from_component(arena: Arena, budget: &mut DecodeBudget) -> Result<MontyGraph, String> {
    let estimated_size = budget.charge(&arena.nodes)?;
    let nodes = arena
        .nodes
        .into_iter()
        .map(node_from_component)
        .collect::<Result<Vec<_>, _>>()?;
    let graph = MontyGraph::from_nodes(nodes).map_err(|err| err.to_string())?;
    if graph.host_size() > estimated_size {
        Err("component value host-memory estimate is smaller than its decoded value".to_owned())
    } else {
        Ok(graph)
    }
}

/// Conservatively estimates one node using `MontyNode::host_size` accounting.
fn node_host_size(node: &ValueNode) -> usize {
    let strings_size = |strings: &[String]| {
        strings.iter().fold(0usize, |size, value| {
            size.saturating_add(MontyNode::metadata_string_size(value))
        })
    };
    let indexes = |count: usize| count.saturating_mul(size_of::<NodeId>());
    let pairs = |count: usize| count.saturating_mul(size_of::<(NodeId, NodeId)>());
    let payload = match node {
        // Two decimal digits per byte is a conservative bound for the parsed
        // binary integer without allocating it merely to measure its bits.
        ValueNode::Bigint(value) => value.len().div_ceil(2),
        ValueNode::Text(value) | ValueNode::Path(value) | ValueNode::Repr(value) | ValueNode::Cycle(value) => {
            value.len()
        }
        ValueNode::Bytes(value) => value.len(),
        ValueNode::ListValue(items)
        | ValueNode::TupleValue(items)
        | ValueNode::Set(items)
        | ValueNode::FrozenSet(items) => indexes(items.len()),
        ValueNode::NamedTuple(value) => value
            .type_name
            .len()
            .saturating_add(strings_size(&value.field_names))
            .saturating_add(indexes(value.items.len())),
        ValueNode::Dict(value) => pairs(value.len()),
        ValueNode::Datetime(value) => value.timezone_name.as_ref().map_or(0, String::len),
        ValueNode::Time(value) => value.timezone_name.as_ref().map_or(0, String::len),
        ValueNode::Timezone(value) => value.name.as_ref().map_or(0, String::len),
        ValueNode::Exception(value) => value.message.as_ref().map_or(0, String::len),
        ValueNode::FileHandle(value) => value.path.len(),
        ValueNode::ClassInstance(value) => pairs(value.attrs.len()),
        ValueNode::ClassType(value) => size_of::<MontyClassTypeNode>()
            .saturating_add(value.name.len())
            .saturating_add(pairs(value.attrs.len())),
        ValueNode::Function(value) => value
            .name
            .len()
            .saturating_add(value.docstring.as_ref().map_or(0, String::len)),
        ValueNode::Ellipsis
        | ValueNode::NotImplemented
        | ValueNode::None
        | ValueNode::Boolean(_)
        | ValueNode::Integer(_)
        | ValueNode::Float(_)
        | ValueNode::Date(_)
        | ValueNode::Timedelta(_)
        | ValueNode::TypeName(_)
        | ValueNode::BuiltinFunction(_) => 0,
    };
    size_of::<MontyNode>().saturating_add(payload)
}

/// Converts one Monty arena's nodes into a component arena.
pub fn into_component(nodes: impl IntoIterator<Item = MontyNode>) -> Arena {
    Arena {
        nodes: nodes.into_iter().map(node_into_component).collect(),
    }
}

/// Converts one component node, validating its leaf payloads; child indexes
/// are checked once the whole arena is assembled.
fn node_from_component(node: ValueNode) -> Result<MontyNode, String> {
    Ok(match node {
        ValueNode::Ellipsis => MontyNode::Ellipsis,
        ValueNode::NotImplemented => MontyNode::NotImplemented,
        ValueNode::None => MontyNode::None,
        ValueNode::Boolean(value) => MontyNode::Bool(value),
        ValueNode::Integer(value) => MontyNode::Int(value),
        ValueNode::Bigint(value) => MontyNode::BigInt(
            value
                .parse()
                .map_err(|_| format!("invalid arbitrary-precision integer {value:?}"))?,
        ),
        ValueNode::Float(value) => MontyNode::Float(value),
        ValueNode::Text(value) => MontyNode::String(value),
        ValueNode::Bytes(value) => MontyNode::Bytes(value),
        ValueNode::ListValue(items) => MontyNode::List(ids(items)),
        ValueNode::TupleValue(items) => MontyNode::Tuple(ids(items)),
        ValueNode::NamedTuple(value) => MontyNode::NamedTuple {
            type_name: value.type_name,
            field_names: value.field_names,
            values: ids(value.items),
        },
        ValueNode::Dict(pairs) => MontyNode::Dict(id_pairs(pairs)),
        ValueNode::Set(items) => MontyNode::Set(ids(items)),
        ValueNode::FrozenSet(items) => MontyNode::FrozenSet(ids(items)),
        ValueNode::Date(value) => {
            validate_date(value.year, value.month, value.day, "Date")?;
            MontyNode::Date(MontyDate {
                year: value.year,
                month: value.month,
                day: value.day,
            })
        }
        ValueNode::Datetime(value) => {
            validate_datetime(&value)?;
            MontyNode::DateTime(MontyDateTime {
                year: value.year,
                month: value.month,
                day: value.day,
                hour: value.hour,
                minute: value.minute,
                second: value.second,
                microsecond: value.microsecond,
                offset_seconds: value.offset_seconds,
                timezone_name: value.timezone_name,
            })
        }
        ValueNode::Time(value) => {
            validate_time(&value)?;
            MontyNode::Time(MontyTime {
                hour: value.hour,
                minute: value.minute,
                second: value.second,
                microsecond: value.microsecond,
                offset_seconds: value.offset_seconds,
                timezone_name: value.timezone_name,
                fold: value.fold,
            })
        }
        ValueNode::Timedelta(value) => {
            validate_timedelta(&value)?;
            MontyNode::TimeDelta(MontyTimeDelta {
                days: value.days,
                seconds: value.seconds,
                microseconds: value.microseconds,
            })
        }
        ValueNode::Timezone(value) => MontyNode::TimeZone(MontyTimeZone {
            offset_seconds: value.offset_seconds,
            name: value.name,
        }),
        ValueNode::Exception(value) => MontyNode::Exception {
            exc_type: value
                .exc_type
                .parse()
                .map_err(|_| format!("unknown exception type {:?}", value.exc_type))?,
            arg: value.message,
        },
        ValueNode::TypeName(value) => {
            MontyNode::Type(MontyType::from_type_name(&value).ok_or_else(|| format!("unknown type name {value:?}"))?)
        }
        ValueNode::ClassType(value) => MontyNode::ClassType(Box::new(MontyClassTypeNode {
            name: value.name,
            id: parse_uuid(&value.id)?,
            host_defined: value.host_defined,
            is_dataclass: value.is_dataclass,
            attrs: id_pairs(value.attrs),
        })),
        ValueNode::BuiltinFunction(value) => MontyNode::BuiltinFunction(
            value
                .parse::<BuiltinsFunctions>()
                .map_err(|_| format!("unknown builtin function {value:?}"))?,
        ),
        ValueNode::Path(value) => MontyNode::Path(value),
        ValueNode::FileHandle(value) => MontyNode::FileHandle(MontyFileHandle {
            path: value.path,
            mode: value.mode.parse::<FileMode>().map_err(Cow::into_owned)?,
            position: value.position,
        }),
        ValueNode::ClassInstance(value) => MontyNode::ClassInstance {
            class_type: NodeId(value.class_type),
            instance_id: parse_uuid(&value.instance_id)?,
            attrs: id_pairs(value.attrs),
        },
        ValueNode::Function(value) => MontyNode::Function {
            name: value.name,
            docstring: value.docstring,
        },
        ValueNode::Repr(value) => MontyNode::Repr(value),
        ValueNode::Cycle(value) => MontyNode::Cycle(value),
    })
}

/// Converts one Monty node into its component twin.
fn node_into_component(node: MontyNode) -> ValueNode {
    match node {
        MontyNode::Ellipsis => ValueNode::Ellipsis,
        MontyNode::NotImplemented => ValueNode::NotImplemented,
        MontyNode::None => ValueNode::None,
        MontyNode::Bool(value) => ValueNode::Boolean(value),
        MontyNode::Int(value) => ValueNode::Integer(value),
        MontyNode::BigInt(value) => ValueNode::Bigint(value.to_string()),
        MontyNode::Float(value) => ValueNode::Float(value),
        MontyNode::String(value) => ValueNode::Text(value),
        MontyNode::Bytes(value) => ValueNode::Bytes(value),
        MontyNode::List(items) => ValueNode::ListValue(raw_ids(items)),
        MontyNode::Tuple(items) => ValueNode::TupleValue(raw_ids(items)),
        MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        } => ValueNode::NamedTuple(NamedTupleNode {
            type_name,
            field_names,
            items: raw_ids(values),
        }),
        MontyNode::Dict(pairs) => ValueNode::Dict(raw_pairs(pairs)),
        MontyNode::Set(items) => ValueNode::Set(raw_ids(items)),
        MontyNode::FrozenSet(items) => ValueNode::FrozenSet(raw_ids(items)),
        MontyNode::Date(value) => ValueNode::Date(DateNode {
            year: value.year,
            month: value.month,
            day: value.day,
        }),
        MontyNode::DateTime(value) => ValueNode::Datetime(DatetimeNode {
            year: value.year,
            month: value.month,
            day: value.day,
            hour: value.hour,
            minute: value.minute,
            second: value.second,
            microsecond: value.microsecond,
            offset_seconds: value.offset_seconds,
            timezone_name: value.timezone_name,
        }),
        MontyNode::Time(value) => ValueNode::Time(TimeNode {
            hour: value.hour,
            minute: value.minute,
            second: value.second,
            microsecond: value.microsecond,
            offset_seconds: value.offset_seconds,
            timezone_name: value.timezone_name,
            fold: value.fold,
        }),
        MontyNode::TimeDelta(value) => ValueNode::Timedelta(TimedeltaNode {
            days: value.days,
            seconds: value.seconds,
            microseconds: value.microseconds,
        }),
        MontyNode::TimeZone(value) => ValueNode::Timezone(TimezoneNode {
            offset_seconds: value.offset_seconds,
            name: value.name,
        }),
        MontyNode::Exception { exc_type, arg } => ValueNode::Exception(ExceptionValueNode {
            exc_type: exc_type.to_string(),
            message: arg,
        }),
        MontyNode::Type(value) => ValueNode::TypeName(value.to_string()),
        MontyNode::ClassType(class) => ValueNode::ClassType(ClassTypeNode {
            name: class.name,
            id: class.id.to_string(),
            host_defined: class.host_defined,
            is_dataclass: class.is_dataclass,
            attrs: raw_pairs(class.attrs),
        }),
        MontyNode::BuiltinFunction(value) => ValueNode::BuiltinFunction(value.to_string()),
        MontyNode::Path(value) => ValueNode::Path(value),
        MontyNode::FileHandle(value) => ValueNode::FileHandle(FileHandleNode {
            path: value.path,
            mode: value.mode.as_str().to_owned(),
            position: value.position,
        }),
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => ValueNode::ClassInstance(ClassInstanceNode {
            class_type: class_type.0,
            instance_id: instance_id.to_string(),
            attrs: raw_pairs(attrs),
        }),
        MontyNode::Function { name, docstring } => ValueNode::Function(FunctionNode { name, docstring }),
        MontyNode::Repr(value) => ValueNode::Repr(value),
        MontyNode::Cycle(placeholder) => ValueNode::Cycle(placeholder),
    }
}

/// Wraps raw child indexes.
fn ids(items: Vec<u32>) -> Vec<NodeId> {
    items.into_iter().map(NodeId).collect()
}

/// Wraps raw key/value index pairs.
fn id_pairs(pairs: Vec<NodePair>) -> Vec<(NodeId, NodeId)> {
    pairs
        .into_iter()
        .map(|pair| (NodeId(pair.key), NodeId(pair.value)))
        .collect()
}

/// Unwraps child indexes.
pub(crate) fn raw_ids(items: Vec<NodeId>) -> Vec<u32> {
    items.into_iter().map(|id| id.0).collect()
}

/// Unwraps key/value index pairs.
pub(crate) fn raw_pairs(pairs: Vec<(NodeId, NodeId)>) -> Vec<NodePair> {
    pairs
        .into_iter()
        .map(|(key, value)| NodePair {
            key: key.0,
            value: value.0,
        })
        .collect()
}

/// Parses a canonical uuid string from the component boundary.
fn parse_uuid(value: &str) -> Result<MontyUuid, String> {
    MontyUuid::parse(value).ok_or_else(|| format!("invalid uuid {value:?}"))
}

/// Validates date components before they enter a node.
fn validate_date(year: i32, month: u8, day: u8, type_name: &str) -> Result<(), String> {
    if !(1..=9999).contains(&year) {
        Err(format!("{type_name}.year {year} is outside the range 1..=9999"))
    } else if !(1..=12).contains(&month) {
        Err(format!("{type_name}.month {month} is outside the range 1..=12"))
    } else {
        let max_day = days_in_month(year, month);
        if (1..=max_day).contains(&day) {
            Ok(())
        } else {
            Err(format!("{type_name}.day {day} is outside the range 1..={max_day}"))
        }
    }
}

/// Validates date/time ranges and timezone-name presence.
fn validate_datetime(value: &DatetimeNode) -> Result<(), String> {
    validate_date(value.year, value.month, value.day, "DateTime")?;
    if value.hour > 23 {
        Err(format!("DateTime.hour {} is outside the range 0..=23", value.hour))
    } else if value.minute > 59 {
        Err(format!("DateTime.minute {} is outside the range 0..=59", value.minute))
    } else if value.second > 59 {
        Err(format!("DateTime.second {} is outside the range 0..=59", value.second))
    } else if value.microsecond > 999_999 {
        Err(format!(
            "DateTime.microsecond {} exceeds maximum 999999",
            value.microsecond
        ))
    } else if value.offset_seconds.is_none() && value.timezone_name.is_some() {
        Err("DateTime.timezone_name requires offset_seconds".to_owned())
    } else {
        Ok(())
    }
}

/// Validates time ranges and timezone-name presence.
fn validate_time(value: &TimeNode) -> Result<(), String> {
    if value.hour > 23 {
        Err(format!("Time.hour {} is outside the range 0..=23", value.hour))
    } else if value.minute > 59 {
        Err(format!("Time.minute {} is outside the range 0..=59", value.minute))
    } else if value.second > 59 {
        Err(format!("Time.second {} is outside the range 0..=59", value.second))
    } else if value.microsecond > 999_999 {
        Err(format!("Time.microsecond {} exceeds maximum 999999", value.microsecond))
    } else if value.offset_seconds.is_none() && value.timezone_name.is_some() {
        Err("Time.timezone_name requires offset_seconds".to_owned())
    } else if value.fold > 1 {
        Err(format!("Time.fold {} is outside the range 0..=1", value.fold))
    } else {
        Ok(())
    }
}

/// Validates normalized timedelta components.
fn validate_timedelta(value: &TimedeltaNode) -> Result<(), String> {
    if !(0..86_400).contains(&value.seconds) {
        Err(format!(
            "TimeDelta.seconds {} is outside the normalized range 0..86400",
            value.seconds
        ))
    } else if !(0..1_000_000).contains(&value.microseconds) {
        Err(format!(
            "TimeDelta.microseconds {} is outside the normalized range 0..1000000",
            value.microseconds
        ))
    } else {
        Ok(())
    }
}

/// Returns the number of days in a validated Gregorian month.
fn days_in_month(year: i32, month: u8) -> u8 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
