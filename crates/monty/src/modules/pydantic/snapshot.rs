//! Snapshots of sandbox values for `ValidationError` payloads.
//!
//! An exception is plain data: it is cloned on every catch and re-raise and
//! travels out of the sandbox, so it cannot hold heap references. A line
//! error's input therefore crosses into the exception as a [`MontyObject`]
//! built here, and comes back out as a fresh value when `errors()` reads it.
//! Scalars, containers, temporal values and decimals survive the round trip;
//! anything else (an instance, a function) is kept as its repr, as is any
//! value past the node budget, so one failing input of any size cannot pin
//! unbounded memory in an exception.

use std::mem;

use monty_types::{
    MontyDate, MontyObject, MontyTime, MontyTimeDelta,
    unstable::{self, MontyNode},
};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapReadOutput},
    object_bridge::MontyObjectExt,
    types::{PyTrait, date as date_type, datetime as datetime_type, time as time_type, timedelta as timedelta_type},
    value::Value,
};

/// Nodes one snapshot may hold before the rest degrades to a repr.
const NODE_BUDGET: usize = 4096;

/// Snapshots `value`, spending from a fresh [`NODE_BUDGET`].
pub(crate) fn snapshot_value(value: &Value, vm: &mut VM<'_>) -> MontyObject {
    let mut budget = NODE_BUDGET;
    snapshot(value, &mut budget, vm)
}

/// One level of the snapshot walk; `budget` counts the nodes left.
fn snapshot(value: &Value, budget: &mut usize, vm: &mut VM<'_>) -> MontyObject {
    if *budget == 0 {
        return repr_object(value, vm);
    }
    *budget -= 1;
    let Ok(mut guard) = vm.recursion_guard() else {
        return repr_object(value, vm);
    };
    let vm = &mut *guard;
    match value {
        Value::None => MontyObject::none(),
        Value::Bool(b) => MontyObject::bool(*b),
        Value::Int(i) => MontyObject::int(*i),
        Value::Float(f) => MontyObject::float(*f),
        Value::InternString(id) => MontyObject::string(vm.interns.get_str(*id)),
        Value::InlineString(text) => MontyObject::string(text.as_str()),
        Value::InternBytes(id) => MontyObject::bytes(vm.interns.get_bytes(*id).to_owned()),
        Value::InternLongInt(id) => MontyObject::bigint(vm.interns.get_long_int(*id).clone()),
        Value::Ref(id) => match vm.heap.read(*id) {
            HeapReadOutput::Str(s) => MontyObject::string(s.get(vm.heap).as_str()),
            HeapReadOutput::Bytes(b) => MontyObject::bytes(b.get(vm.heap).as_slice().to_owned()),
            HeapReadOutput::LongInt(li) => MontyObject::bigint(li.get(vm.heap).inner().clone()),
            HeapReadOutput::List(list) => {
                let children: Vec<Value> = list
                    .get(vm.heap)
                    .as_slice()
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyObject::list(snapshot_all(children, budget, vm))
            }
            HeapReadOutput::Tuple(tuple) => {
                let children: Vec<Value> = tuple
                    .get(vm.heap)
                    .as_slice()
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyObject::tuple(snapshot_all(children, budget, vm))
            }
            HeapReadOutput::Set(set) => {
                let children: Vec<Value> = set.get(vm.heap).iter().map(|v| v.clone_with_heap(vm.heap)).collect();
                defer_drop!(children, vm);
                MontyObject::set(snapshot_all(children, budget, vm))
            }
            HeapReadOutput::FrozenSet(set) => {
                let children: Vec<Value> = set
                    .get(vm.heap)
                    .storage()
                    .iter()
                    .map(|v| v.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyObject::frozenset(snapshot_all(children, budget, vm))
            }
            HeapReadOutput::Dict(dict) => {
                let pairs: Vec<(Value, Value)> = dict
                    .get(vm.heap)
                    .iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect();
                defer_drop!(pairs, vm);
                let pairs = pairs
                    .iter()
                    .map(|(k, v)| (snapshot(k, budget, vm), snapshot(v, budget, vm)))
                    .collect::<Vec<_>>();
                MontyObject::dict(pairs)
            }
            HeapReadOutput::NamedTuple(nt) => {
                let type_name = nt.get(vm.heap).name(vm.interns).to_owned();
                let field_names: Vec<String> = nt
                    .get(vm.heap)
                    .field_names()
                    .iter()
                    .map(|name| name.as_str(vm.interns).to_owned())
                    .collect();
                let children: Vec<Value> = nt
                    .get(vm.heap)
                    .items()
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyObject::named_tuple(type_name, field_names, snapshot_all(children, budget, vm))
            }
            HeapReadOutput::Date(d) => {
                let (year, month, day) = date_type::to_ymd(*d.get(vm.heap));
                MontyObject::date(MontyDate {
                    year,
                    month: u8::try_from(month).expect("month is always 1..=12"),
                    day: u8::try_from(day).expect("day is always 1..=31"),
                })
            }
            HeapReadOutput::DateTime(dt) => match datetime_type::to_monty_datetime(dt.get(vm.heap)) {
                Some(datetime) => MontyObject::datetime(datetime),
                None => repr_object(value, vm),
            },
            HeapReadOutput::Time(t) => {
                let time = t.get(vm.heap);
                let (hour, minute, second, microsecond, fold) = time.to_components();
                let tz = time_type::attached_timezone(time, vm.heap);
                MontyObject::time(MontyTime {
                    hour,
                    minute,
                    second,
                    microsecond,
                    offset_seconds: tz.as_ref().map(|tz| tz.offset_seconds),
                    timezone_name: tz.and_then(|tz| tz.name),
                    fold,
                })
            }
            HeapReadOutput::TimeDelta(td) => {
                let (days, seconds, microseconds) = timedelta_type::components(td.get(vm.heap));
                MontyObject::timedelta(MontyTimeDelta {
                    days,
                    seconds,
                    microseconds,
                })
            }
            HeapReadOutput::Decimal(d) => MontyObject::decimal(d.get(vm.heap).to_sci_string(false, true)),
            _ => repr_object(value, vm),
        },
        _ => repr_object(value, vm),
    }
}

/// Snapshots each of `children` in order, sharing `budget`.
fn snapshot_all(children: &[Value], budget: &mut usize, vm: &mut VM<'_>) -> Vec<MontyObject> {
    children.iter().map(|child| snapshot(child, budget, vm)).collect()
}

/// The repr fallback: a value with no snapshot form keeps the text Python
/// would print for it.
fn repr_object(value: &Value, vm: &mut VM<'_>) -> MontyObject {
    MontyObject::repr(repr_string(value, vm))
}

/// `repr(value)` as a Rust string, or the type name when the repr itself fails.
pub(crate) fn repr_string(value: &Value, vm: &mut VM<'_>) -> String {
    match value.py_repr(vm) {
        Ok(repr) => {
            defer_drop!(repr, vm);
            repr.as_str(vm).map_or_else(String::new, str::to_owned)
        }
        Err(_) => format!("<unprintable {} object>", value.py_type_name(vm)),
    }
}

/// The Python type name a line error reports as `input_type`.
pub(crate) fn type_name(value: &Value, vm: &VM<'_>) -> String {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Instance(_) | HeapData::HostClass(_) => value.py_type_name(vm).into_owned(),
            data => data.py_type().dunder_name(vm.heap, vm.interns).into_owned(),
        },
        other => other.py_type(vm).dunder_name(vm.heap, vm.interns).into_owned(),
    }
}

/// Rebuilds a snapshot as a live value, a repr fallback coming back as the
/// string it holds.
pub(crate) fn restore_value(snapshot: &MontyObject, vm: &mut VM<'_>) -> RunResult<Value> {
    let (graph, root) = unstable::graph_parts(snapshot);
    let mut graph = graph.clone();
    for node in graph.nodes_mut() {
        if let MontyNode::Repr(text) = node {
            *node = MontyNode::String(mem::take(text));
        }
    }
    let object = unstable::object_from_graph(graph, root).expect("a snapshot is a valid graph");
    object
        .to_value(vm)
        .map_err(|err| ExcType::value_error(format!("{err}")))
}
