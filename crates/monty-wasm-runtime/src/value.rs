//! Conversion between component-model value arenas and [`MontyObject`].
//!
//! WIT cannot express recursive value types, so the component boundary uses a
//! flat node arena whose container nodes hold indexes. Protobuf remains an
//! internal detail of `monty-proto`; no wire bytes cross into JavaScript.

use std::borrow::Cow;

use monty_proto::{DEFAULT_MAX_DECODE_BYTES, MAX_VALUE_DEPTH, exceeds_max_value_depth};
use monty_types::{
    DictPairs, FileMode, MontyClassInstance, MontyClassType, MontyDate, MontyDateTime, MontyFileHandle, MontyObject,
    MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid,
};

use crate::bindings::exports::pydantic::monty::worker::{
    ClassInstanceNode, ClassTypeNode, CycleNode, DateNode, DatetimeNode, ExceptionValueNode, FileHandleNode,
    FunctionNode, NamedTupleNode, NodePair, TimeNode, TimedeltaNode, TimezoneNode, Value, ValueNode,
};

/// Remaining expanded-value allowance shared by every arena in one request.
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
    /// Charges an arena before conversion allocates its `MontyObject` tree.
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

/// Converts one component value arena into an owned Monty boundary value.
pub fn from_component(value: Value, budget: &mut DecodeBudget) -> Result<MontyObject, String> {
    let Value { root, nodes } = value;
    let estimated_size = budget.charge(&nodes)?;
    let mut nodes = nodes.into_iter().map(Some).collect::<Vec<_>>();
    let object = read_node(root, &mut nodes, 0)?;
    if let Some(index) = nodes.iter().position(Option::is_some) {
        Err(format!("value node index {index} is unreachable from the root"))
    } else if exceeds_max_value_depth(&object) {
        Err("value exceeds the maximum nesting depth".to_owned())
    } else if object.deep_host_size() > estimated_size {
        Err("component value host-memory estimate is smaller than its decoded value".to_owned())
    } else {
        Ok(object)
    }
}

/// Conservatively estimates one node using `MontyObject::host_size` accounting.
fn node_host_size(node: &ValueNode) -> usize {
    let strings_size = |strings: &[String]| {
        strings.iter().fold(0usize, |size, value| {
            size.saturating_add(MontyObject::host_metadata_string_size(value))
        })
    };
    let payload = match node {
        // Two decimal digits per byte is a conservative bound for the parsed
        // binary integer without allocating it merely to measure its bits.
        ValueNode::Bigint(value) => value.len().div_ceil(2),
        ValueNode::Text(value) | ValueNode::Path(value) | ValueNode::Repr(value) => value.len(),
        ValueNode::Bytes(value) => value.len(),
        ValueNode::NamedTuple(value) => value.type_name.len().saturating_add(strings_size(&value.field_names)),
        ValueNode::Datetime(value) => value.timezone_name.as_ref().map_or(0, String::len),
        ValueNode::Time(value) => value.timezone_name.as_ref().map_or(0, String::len),
        ValueNode::Timezone(value) => value.name.as_ref().map_or(0, String::len),
        ValueNode::Exception(value) => value.message.as_ref().map_or(0, String::len),
        ValueNode::FileHandle(value) => value.path.len(),
        ValueNode::ClassInstance(value) => value.instance_id.len(),
        ValueNode::ClassType(value) => value.name.len().saturating_add(value.id.len()),
        ValueNode::Function(value) => value
            .name
            .len()
            .saturating_add(value.docstring.as_ref().map_or(0, String::len)),
        ValueNode::Cycle(value) => value.placeholder.len(),
        ValueNode::Ellipsis
        | ValueNode::NotImplemented
        | ValueNode::None
        | ValueNode::Boolean(_)
        | ValueNode::Integer(_)
        | ValueNode::Float(_)
        | ValueNode::ListValue(_)
        | ValueNode::TupleValue(_)
        | ValueNode::Dict(_)
        | ValueNode::Set(_)
        | ValueNode::FrozenSet(_)
        | ValueNode::Date(_)
        | ValueNode::Timedelta(_)
        | ValueNode::TypeName(_)
        | ValueNode::BuiltinFunction(_) => 0,
    };
    MontyObject::host_base_size().saturating_add(payload)
}

/// Converts one owned Monty boundary value into a component value arena.
pub fn into_component(object: MontyObject) -> Value {
    let mut nodes = Vec::new();
    let root = push_node(object, &mut nodes);
    Value { root, nodes }
}

/// Reads one arena node, rejecting bad indexes, cycles, and excessive nesting.
fn read_node(index: u32, nodes: &mut [Option<ValueNode>], depth: usize) -> Result<MontyObject, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err("value exceeds the maximum nesting depth".to_owned());
    }
    let index = usize::try_from(index).map_err(|_| "value node index does not fit in usize")?;
    let node = nodes
        .get_mut(index)
        .ok_or_else(|| format!("value node index {index} is out of bounds"))?
        .take()
        .ok_or_else(|| format!("value node index {index} is referenced more than once"))?;
    let object = match node {
        ValueNode::Ellipsis => MontyObject::Ellipsis,
        ValueNode::NotImplemented => MontyObject::NotImplemented,
        ValueNode::None => MontyObject::None,
        ValueNode::Boolean(value) => MontyObject::Bool(value),
        ValueNode::Integer(value) => MontyObject::Int(value),
        ValueNode::Bigint(value) => MontyObject::BigInt(
            value
                .parse()
                .map_err(|_| format!("invalid arbitrary-precision integer {value:?}"))?,
        ),
        ValueNode::Float(value) => MontyObject::Float(value),
        ValueNode::Text(value) => MontyObject::String(value),
        ValueNode::Bytes(value) => MontyObject::Bytes(value),
        ValueNode::ListValue(items) => MontyObject::List(read_items(items, nodes, depth)?),
        ValueNode::TupleValue(items) => MontyObject::Tuple(read_items(items, nodes, depth)?),
        ValueNode::NamedTuple(value) => MontyObject::NamedTuple {
            type_name: value.type_name,
            field_names: value.field_names,
            values: read_items(value.items, nodes, depth)?,
        },
        ValueNode::Dict(pairs) => MontyObject::Dict(read_pairs(pairs, nodes, depth)?.into()),
        ValueNode::Set(items) => MontyObject::Set(read_items(items, nodes, depth)?),
        ValueNode::FrozenSet(items) => MontyObject::FrozenSet(read_items(items, nodes, depth)?),
        ValueNode::Date(value) => {
            validate_date(value.year, value.month, value.day, "Date")?;
            MontyObject::Date(MontyDate {
                year: value.year,
                month: value.month,
                day: value.day,
            })
        }
        ValueNode::Datetime(value) => {
            validate_datetime(&value)?;
            MontyObject::DateTime(MontyDateTime {
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
            MontyObject::Time(MontyTime {
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
            MontyObject::TimeDelta(MontyTimeDelta {
                days: value.days,
                seconds: value.seconds,
                microseconds: value.microseconds,
            })
        }
        ValueNode::Timezone(value) => MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: value.offset_seconds,
            name: value.name,
        }),
        ValueNode::Exception(value) => MontyObject::Exception {
            exc_type: value
                .exc_type
                .parse()
                .map_err(|_| format!("unknown exception type {:?}", value.exc_type))?,
            arg: value.message,
        },
        ValueNode::TypeName(value) => {
            MontyObject::Type(MontyType::from_type_name(&value).ok_or_else(|| format!("unknown type name {value:?}"))?)
        }
        ValueNode::ClassType(value) => MontyObject::Type(read_class_type(value, nodes, depth)?),
        ValueNode::BuiltinFunction(value) => MontyObject::builtin_function_from_name(&value)
            .ok_or_else(|| format!("unknown builtin function {value:?}"))?,
        ValueNode::Path(value) => MontyObject::Path(value),
        ValueNode::FileHandle(value) => MontyObject::FileHandle(MontyFileHandle {
            path: value.path,
            mode: value.mode.parse::<FileMode>().map_err(Cow::into_owned)?,
            position: value.position,
        }),
        ValueNode::ClassInstance(value) => {
            // The class node is read like any other child, so the
            // reachable-exactly-once arena invariant covers it too.
            let class_node = take_node(value.class_type, nodes)?;
            let ValueNode::ClassType(class_node) = class_node else {
                return Err("class-instance node's class-type index is not a class-type node".to_owned());
            };
            let MontyType::Instance(class_type) = read_class_type(class_node, nodes, depth)? else {
                unreachable!("read_class_type on a MontyClassType node always yields Instance");
            };
            MontyObject::ClassInstance(MontyClassInstance {
                class_type: *class_type,
                instance_id: parse_uuid(&value.instance_id)?,
                attrs: read_pairs(value.attrs, nodes, depth)?.into(),
            })
        }
        ValueNode::Function(value) => MontyObject::Function {
            name: value.name,
            docstring: value.docstring,
        },
        ValueNode::Repr(value) => MontyObject::Repr(value),
        ValueNode::Cycle(value) => MontyObject::Cycle(
            usize::try_from(value.identity).map_err(|_| "cycle identity does not fit in usize")?,
            value.placeholder,
        ),
    };
    Ok(object)
}

/// Takes one node out of the arena by index, enforcing the
/// reachable-exactly-once invariant (same rules as `read_node`).
fn take_node(index: u32, nodes: &mut [Option<ValueNode>]) -> Result<ValueNode, String> {
    let index = usize::try_from(index).map_err(|_| "value node index does not fit in usize")?;
    nodes
        .get_mut(index)
        .ok_or_else(|| format!("value node index {index} is out of bounds"))?
        .take()
        .ok_or_else(|| format!("value node index {index} is referenced more than once"))
}

/// Parses a canonical uuid string from the component boundary.
fn parse_uuid(value: &str) -> Result<MontyUuid, String> {
    MontyUuid::parse(value).ok_or_else(|| format!("invalid uuid {value:?}"))
}

/// Reads a class-type node into `MontyType::Instance`, resolving the eager
/// class attrs recursively.
fn read_class_type(node: ClassTypeNode, nodes: &mut [Option<ValueNode>], depth: usize) -> Result<MontyType, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err("value exceeds the maximum nesting depth".to_owned());
    }
    let attrs = read_pairs(node.attrs, nodes, depth)?;
    Ok(MontyType::Instance(Box::new(MontyClassType {
        name: node.name,
        id: parse_uuid(&node.id)?,
        host_defined: node.host_defined,
        is_dataclass: node.is_dataclass,
        attrs: attrs.into(),
    })))
}

/// Reads a list of child indexes from an arena.
fn read_items(items: Vec<u32>, nodes: &mut [Option<ValueNode>], depth: usize) -> Result<Vec<MontyObject>, String> {
    items
        .into_iter()
        .map(|index| read_node(index, nodes, depth + 1))
        .collect()
}

/// Reads key/value indexes from an arena while preserving pair order.
fn read_pairs(
    pairs: Vec<NodePair>,
    nodes: &mut [Option<ValueNode>],
    depth: usize,
) -> Result<Vec<(MontyObject, MontyObject)>, String> {
    pairs
        .into_iter()
        .map(|pair| {
            Ok((
                read_node(pair.key, nodes, depth + 1)?,
                read_node(pair.value, nodes, depth + 1)?,
            ))
        })
        .collect()
}

/// Validates date components before they enter a `MontyObject`.
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

/// Appends one object and all its children to an arena, returning its index.
fn push_node(object: MontyObject, nodes: &mut Vec<ValueNode>) -> u32 {
    let node = match object {
        MontyObject::Ellipsis => ValueNode::Ellipsis,
        MontyObject::NotImplemented => ValueNode::NotImplemented,
        MontyObject::None => ValueNode::None,
        MontyObject::Bool(value) => ValueNode::Boolean(value),
        MontyObject::Int(value) => ValueNode::Integer(value),
        MontyObject::BigInt(value) => ValueNode::Bigint(value.to_string()),
        MontyObject::Float(value) => ValueNode::Float(value),
        MontyObject::String(value) => ValueNode::Text(value),
        MontyObject::Bytes(value) => ValueNode::Bytes(value),
        MontyObject::List(items) => ValueNode::ListValue(push_items(items, nodes)),
        MontyObject::Tuple(items) => ValueNode::TupleValue(push_items(items, nodes)),
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => ValueNode::NamedTuple(NamedTupleNode {
            type_name,
            field_names,
            items: push_items(values, nodes),
        }),
        MontyObject::Dict(pairs) => ValueNode::Dict(push_pairs(pairs, nodes)),
        MontyObject::Set(items) => ValueNode::Set(push_items(items, nodes)),
        MontyObject::FrozenSet(items) => ValueNode::FrozenSet(push_items(items, nodes)),
        MontyObject::Date(value) => ValueNode::Date(DateNode {
            year: value.year,
            month: value.month,
            day: value.day,
        }),
        MontyObject::DateTime(value) => ValueNode::Datetime(DatetimeNode {
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
        MontyObject::Time(value) => ValueNode::Time(TimeNode {
            hour: value.hour,
            minute: value.minute,
            second: value.second,
            microsecond: value.microsecond,
            offset_seconds: value.offset_seconds,
            timezone_name: value.timezone_name,
            fold: value.fold,
        }),
        MontyObject::TimeDelta(value) => ValueNode::Timedelta(TimedeltaNode {
            days: value.days,
            seconds: value.seconds,
            microseconds: value.microseconds,
        }),
        MontyObject::TimeZone(value) => ValueNode::Timezone(TimezoneNode {
            offset_seconds: value.offset_seconds,
            name: value.name,
        }),
        MontyObject::Exception { exc_type, arg } => ValueNode::Exception(ExceptionValueNode {
            exc_type: exc_type.to_string(),
            message: arg,
        }),
        MontyObject::Type(MontyType::Instance(class_type)) => ValueNode::ClassType(push_class_type(*class_type, nodes)),
        MontyObject::Type(value) => ValueNode::TypeName(value.to_string()),
        MontyObject::BuiltinFunction(value) => ValueNode::BuiltinFunction(value.to_string()),
        MontyObject::Path(value) => ValueNode::Path(value),
        MontyObject::FileHandle(value) => ValueNode::FileHandle(FileHandleNode {
            path: value.path,
            mode: value.mode.as_str().to_owned(),
            position: value.position,
        }),
        MontyObject::ClassInstance(instance) => {
            let class_node = push_class_type(instance.class_type, nodes);
            let class_index = u32::try_from(nodes.len()).expect("component value arena exceeds u32::MAX nodes");
            nodes.push(ValueNode::ClassType(class_node));
            ValueNode::ClassInstance(ClassInstanceNode {
                class_type: class_index,
                instance_id: instance.instance_id.to_string(),
                attrs: push_pairs(instance.attrs, nodes),
            })
        }
        MontyObject::Function { name, docstring } => ValueNode::Function(FunctionNode { name, docstring }),
        MontyObject::Repr(value) => ValueNode::Repr(value),
        MontyObject::Cycle(identity, placeholder) => ValueNode::Cycle(CycleNode {
            identity: u64::try_from(identity).expect("usize always fits in u64"),
            placeholder,
        }),
    };
    let index = u32::try_from(nodes.len()).expect("component value arena exceeds u32::MAX nodes");
    nodes.push(node);
    index
}

/// Builds a class-type node, appending its eager attr nodes to the arena.
fn push_class_type(class_type: MontyClassType, nodes: &mut Vec<ValueNode>) -> ClassTypeNode {
    let attrs = push_pairs(class_type.attrs, nodes);
    ClassTypeNode {
        name: class_type.name,
        id: class_type.id.to_string(),
        host_defined: class_type.host_defined,
        is_dataclass: class_type.is_dataclass,
        attrs,
    }
}

/// Appends a sequence's child values and returns their indexes.
fn push_items(items: Vec<MontyObject>, nodes: &mut Vec<ValueNode>) -> Vec<u32> {
    items.into_iter().map(|item| push_node(item, nodes)).collect()
}

/// Appends a mapping's keys and values and returns their index pairs.
fn push_pairs(pairs: DictPairs, nodes: &mut Vec<ValueNode>) -> Vec<NodePair> {
    pairs
        .into_iter()
        .map(|(key, value)| NodePair {
            key: push_node(key, nodes),
            value: push_node(value, nodes),
        })
        .collect()
}
