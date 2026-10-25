//! Conversion of a parsed TOML tree into Monty values.
//!
//! Runs only after [`super::parser`] accepted the whole document, so every
//! allocation here is for a value that will be returned. The walk is iterative
//! with an explicit stack of half-built containers: `[a.b.c…]` headers nest
//! tables as deep as the key-part limit allows, far deeper than Rust recursion
//! could safely follow on a small thread stack. The stack is guarded while it
//! fills, so a memory limit tripped halfway releases what was built, and the
//! per-element poll keeps a huge document from allocating past the soft limit
//! inside this single native call.

use std::vec;

use indexmap::map;

use super::parser::{TomlDate, TomlInt, TomlTable, TomlTime, TomlValue};
use crate::{
    bytecode::VM,
    defer_drop_mut,
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, HeapData},
    types::{Dict, List, LongInt, TimeZone, date, datetime, str::allocate_string, time},
    value::{VALUE_SIZE, Value},
};

/// Converts the document's root table into the dict `tomllib.loads()` returns.
pub(super) fn table_into_value(table: TomlTable, vm: &mut VM<'_>) -> RunResult<Value> {
    let stack: Vec<Frame> = Vec::new();
    defer_drop_mut!(stack, vm);
    let mut next = Step::Convert(TomlValue::Table(table));
    loop {
        let value = match next {
            Step::Convert(node) => match Frame::open(node, vm)? {
                Ok(frame) => {
                    stack.push(frame);
                    next = Step::Descend;
                    continue;
                }
                Err(scalar) => scalar,
            },
            Step::Descend => match stack.last_mut().expect("Descend follows a push") {
                Frame::List { items, values } => match items.next() {
                    Some(item) => {
                        vm.heap.tracker.check_memory_time_every(values.len())?;
                        next = Step::Convert(item);
                        continue;
                    }
                    None => Frame::finish(stack.pop(), vm),
                },
                Frame::Dict {
                    entries,
                    dict,
                    pending_key,
                } => match entries.next() {
                    Some((key, item)) => {
                        vm.heap.tracker.check_memory_time_every(dict.len())?;
                        *pending_key = Some(allocate_string(key, vm.heap));
                        next = Step::Convert(item);
                        continue;
                    }
                    None => Frame::finish(stack.pop(), vm),
                },
            },
        };
        // `value` is complete: hand it to the enclosing container, or return it.
        let Some(parent) = stack.last_mut() else {
            return Ok(value);
        };
        parent.store(value, vm)?;
        next = Step::Descend;
    }
}

/// What the conversion loop does next.
enum Step {
    /// Turn `node` into a value, opening a frame if it is a container.
    Convert(TomlValue),
    /// Continue with the next child of the innermost open container.
    Descend,
}

/// A container whose children are still being converted.
enum Frame {
    List {
        items: vec::IntoIter<TomlValue>,
        values: Vec<Value>,
    },
    Dict {
        entries: map::IntoIter<String, TomlValue>,
        dict: Dict,
        /// The key allocated for the child currently being converted.
        pending_key: Option<Value>,
    },
}

impl Frame {
    /// Opens a frame for a container node, or converts a scalar in place.
    fn open(node: TomlValue, vm: &mut VM<'_>) -> RunResult<Result<Self, Value>> {
        let scalar = match node {
            TomlValue::Array(items) => {
                vm.heap.tracker.check_allocation(items.len() * VALUE_SIZE)?;
                let values = Vec::with_capacity(items.len());
                return Ok(Ok(Self::List {
                    items: items.into_iter(),
                    values,
                }));
            }
            TomlValue::Table(table) => {
                let entries = table.into_entries();
                // No bulk reservation: `set_json_string_key` preflights each
                // capacity doubling against the memory limit as the dict fills.
                let dict = Dict::new();
                return Ok(Ok(Self::Dict {
                    entries: entries.into_iter(),
                    dict,
                    pending_key: None,
                }));
            }
            TomlValue::Str(text) => allocate_string(text, vm.heap),
            TomlValue::Int(TomlInt::Small(value)) => Value::Int(value),
            TomlValue::Int(TomlInt::Big(value)) => LongInt::new(value).into_value(vm.heap),
            TomlValue::Float(value) => Value::Float(value),
            TomlValue::Bool(value) => Value::Bool(value),
            TomlValue::Date(date) => date::allocate_ymd(date.year, date.month, date.day, vm.heap),
            TomlValue::Time(time) => {
                time::allocate_naive(time.hour, time.minute, time.second, time.microsecond, vm.heap)
            }
            TomlValue::DateTime {
                date,
                time,
                offset_seconds,
            } => allocate_datetime(date, time, offset_seconds, vm)?,
        };
        Ok(Err(scalar))
    }

    /// Stores a converted child. Table keys are unique, so no entry is replaced.
    fn store(&mut self, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
        match self {
            Self::List { values, .. } => values.push(value),
            Self::Dict { dict, pending_key, .. } => {
                let key = pending_key
                    .take()
                    .expect("a key is allocated before its value is converted");
                if let Some(replaced) = dict.set_json_string_key(key, value, vm)? {
                    replaced.drop_with(vm);
                }
            }
        }
        Ok(())
    }

    /// Allocates the heap object for a frame whose children are all stored.
    fn finish(frame: Option<Self>, vm: &mut VM<'_>) -> Value {
        match frame.expect("Descend follows a push") {
            Self::List { values, .. } => Value::Ref(vm.heap.allocate(HeapData::List(List::new(values)))),
            Self::Dict { dict, .. } => Value::Ref(vm.heap.allocate(HeapData::Dict(dict))),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Frame {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::List { values, .. } => values.drop_with(ctx),
            Self::Dict { dict, pending_key, .. } => {
                dict.drop_with(ctx);
                pending_key.drop_with(ctx);
            }
        }
    }
}

/// A `datetime`, aware when the document gave an offset (`Z` is UTC).
fn allocate_datetime(date: TomlDate, time: TomlTime, offset_seconds: Option<i32>, vm: &mut VM<'_>) -> RunResult<Value> {
    // The regex bounds offsets to ±23:59, inside `timezone()`'s open ±24h range.
    let tzinfo = offset_seconds.map(|seconds| TimeZone::new(seconds, None)).transpose()?;
    let datetime = datetime::from_components(
        date.year,
        date.month,
        date.day,
        time.hour,
        time.minute,
        time.second,
        time.microsecond,
        tzinfo,
        None,
        vm.heap,
    )?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(datetime))))
}
