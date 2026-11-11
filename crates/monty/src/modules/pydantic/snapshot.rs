//! Snapshots of sandbox values for `ValidationError` payloads.
//!
//! An exception is plain data: it is cloned on every catch and re-raise and
//! travels out of the sandbox, so it cannot hold heap references. A line
//! error's input therefore crosses into the exception as a [`MontyObject`]
//! built here, and comes back out as a fresh value when `errors()` reads it.
//! Scalars, containers, temporal values and decimals survive the round trip;
//! anything else (an instance, a function) is kept as its repr.
//! One [`Snapshotter`] serves a whole exception, so shared sub-objects and
//! many line errors cannot multiply its size past the node and text budgets.

use std::mem;

use ahash::AHashSet;
use monty_types::{
    MontyObject,
    unstable::{self, MontyNode},
};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{ContainsHeap, HeapData, HeapId, HeapReadOutput},
    object_bridge::MontyObjectExt,
    types::{PyTrait, date as date_type, datetime as datetime_type, time as time_type, timedelta as timedelta_type},
    value::Value,
};

/// Nodes one exception's snapshots may hold before the rest is cut off.
const NODE_BUDGET: usize = 32_768;

/// Bytes of string, bytes and repr text one exception's snapshots may copy;
/// the leaf that crosses it is kept whole.
const TEXT_BUDGET: usize = 4 * 1024 * 1024;

/// The marker that stands in for whatever a spent budget cut off.
const TRUNCATED: &str = "...";

/// Snapshots `value` with a fresh budget, for a one-off value such as a context entry.
pub(crate) fn snapshot_value(value: &Value, vm: &mut VM<'_>) -> MontyObject {
    Snapshotter::new().snapshot(value, vm)
}

/// The budget shared by every snapshot of one exception, plus the containers
/// being descended, so a container that holds itself is cut at the reference back.
pub(crate) struct Snapshotter {
    nodes: usize,
    text: usize,
    in_progress: AHashSet<HeapId>,
}

impl Snapshotter {
    pub(crate) fn new() -> Self {
        Self {
            nodes: NODE_BUDGET,
            text: TEXT_BUDGET,
            in_progress: AHashSet::new(),
        }
    }

    /// Snapshots `value`, or returns the [`TRUNCATED`] marker once the budget is spent.
    pub(crate) fn snapshot(&mut self, value: &Value, vm: &mut VM<'_>) -> MontyObject {
        if self.spent() {
            return truncated();
        }
        self.nodes -= 1;
        let Ok(mut guard) = vm.recursion_guard() else {
            return truncated();
        };
        let vm = &mut *guard;
        if let Value::Ref(id) = value {
            if self.in_progress.contains(id) {
                return MontyObject::repr(cycle_placeholder(vm.heap.get(*id)));
            }
            self.in_progress.insert(*id);
        }
        let object = self.snapshot_node(value, vm);
        if let Value::Ref(id) = value {
            self.in_progress.remove(id);
        }
        object
    }

    fn spent(&self) -> bool {
        self.nodes == 0 || self.text == 0
    }

    /// Charges `len` bytes of copied text against the budget.
    fn charge(&mut self, len: usize) {
        self.text = self.text.saturating_sub(len);
    }

    /// The node for `value`, its children snapshotted through `self`.
    fn snapshot_node(&mut self, value: &Value, vm: &mut VM<'_>) -> MontyObject {
        match value {
            Value::None => MontyObject::none(),
            Value::Bool(b) => MontyObject::bool(*b),
            Value::Int(i) => MontyObject::int(*i),
            Value::Float(f) => MontyObject::float(*f),
            Value::InternString(id) => self.string(vm.interns.get_str(*id)),
            Value::InlineString(text) => self.string(text.as_str()),
            Value::InternBytes(id) => self.bytes(vm.interns.get_bytes(*id)),
            Value::InternLongInt(id) => MontyObject::bigint(vm.interns.get_long_int(*id).clone()),
            Value::Ref(id) => match vm.heap.read(*id) {
                HeapReadOutput::Str(s) => self.string(s.get(vm.heap).as_str()),
                HeapReadOutput::Bytes(b) => self.bytes(b.get(vm.heap).as_slice()),
                HeapReadOutput::LongInt(li) => MontyObject::bigint(li.get(vm.heap).inner().clone()),
                HeapReadOutput::List(list) => {
                    let items = list.get(vm.heap).as_slice();
                    let (len, children) = (items.len(), self.clone_prefix(items.iter(), vm.heap));
                    MontyObject::list(self.snapshot_all(children, len, vm))
                }
                HeapReadOutput::Tuple(tuple) => {
                    let items = tuple.get(vm.heap).as_slice();
                    let (len, children) = (items.len(), self.clone_prefix(items.iter(), vm.heap));
                    MontyObject::tuple(self.snapshot_all(children, len, vm))
                }
                HeapReadOutput::Set(set) => {
                    let set = set.get(vm.heap);
                    let (len, children) = (set.len(), self.clone_prefix(set.iter(), vm.heap));
                    MontyObject::set(self.snapshot_all(children, len, vm))
                }
                HeapReadOutput::FrozenSet(set) => {
                    let set = set.get(vm.heap);
                    let (len, children) = (set.len(), self.clone_prefix(set.storage().iter(), vm.heap));
                    MontyObject::frozenset(self.snapshot_all(children, len, vm))
                }
                HeapReadOutput::Dict(dict) => {
                    let dict = dict.get(vm.heap);
                    let len = dict.len();
                    let pairs: Vec<(Value, Value)> = dict
                        .iter()
                        .take(self.nodes)
                        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                        .collect();
                    MontyObject::dict(self.snapshot_pairs(pairs, len, vm))
                }
                HeapReadOutput::NamedTuple(nt) => {
                    let nt = nt.get(vm.heap);
                    let type_name = nt.name(vm.interns).to_owned();
                    let field_names: Vec<String> = nt
                        .field_names()
                        .iter()
                        .map(|name| name.as_str(vm.interns).to_owned())
                        .collect();
                    self.charge(type_name.len() + field_names.iter().map(String::len).sum::<usize>());
                    let items = nt.items();
                    let (len, children) = (items.len(), self.clone_prefix(items.iter(), vm.heap));
                    let items = self.snapshot_all(children, len, vm);
                    if items.len() == field_names.len() {
                        MontyObject::named_tuple(type_name, field_names, items)
                    } else {
                        // Cut short, it no longer fits its fields.
                        MontyObject::tuple(items)
                    }
                }
                HeapReadOutput::Date(d) => MontyObject::date(date_type::to_monty_date(*d.get(vm.heap))),
                HeapReadOutput::DateTime(dt) => match datetime_type::to_monty_datetime(dt.get(vm.heap)) {
                    Some(datetime) => MontyObject::datetime(datetime),
                    None => self.repr(value, vm),
                },
                HeapReadOutput::Time(t) => MontyObject::time(time_type::to_monty_time(t.get(vm.heap), vm.heap)),
                HeapReadOutput::TimeDelta(td) => {
                    MontyObject::timedelta(timedelta_type::to_monty_timedelta(td.get(vm.heap)))
                }
                HeapReadOutput::Decimal(d) => {
                    let text = d.get(vm.heap).to_sci_string(false, true);
                    self.charge(text.len());
                    MontyObject::decimal(text)
                }
                _ => self.repr(value, vm),
            },
            _ => self.repr(value, vm),
        }
    }

    /// Clones the children the node budget can still reach; the rest are never visited.
    fn clone_prefix<'a>(&self, items: impl Iterator<Item = &'a Value>, heap: &impl ContainsHeap) -> Vec<Value> {
        items.take(self.nodes).map(|item| item.clone_with_heap(heap)).collect()
    }

    /// Snapshots `children`, the first of `len` items, ending with one
    /// [`TRUNCATED`] marker if the budget stops it short.
    fn snapshot_all(&mut self, children: Vec<Value>, len: usize, vm: &mut VM<'_>) -> Vec<MontyObject> {
        defer_drop!(children, vm);
        let mut out = Vec::with_capacity(children.len() + 1);
        for child in children {
            if self.spent() {
                break;
            }
            out.push(self.snapshot(child, vm));
        }
        if out.len() < len {
            out.push(truncated());
        }
        out
    }

    /// [`Self::snapshot_all`] for a dict, a cut-off tail becoming one marker pair.
    fn snapshot_pairs(
        &mut self,
        pairs: Vec<(Value, Value)>,
        len: usize,
        vm: &mut VM<'_>,
    ) -> Vec<(MontyObject, MontyObject)> {
        defer_drop!(pairs, vm);
        let mut out = Vec::with_capacity(pairs.len() + 1);
        for (key, value) in pairs {
            if self.spent() {
                break;
            }
            out.push((self.snapshot(key, vm), self.snapshot(value, vm)));
        }
        if out.len() < len {
            out.push((truncated(), truncated()));
        }
        out
    }

    fn string(&mut self, text: &str) -> MontyObject {
        self.charge(text.len());
        MontyObject::string(text)
    }

    fn bytes(&mut self, bytes: &[u8]) -> MontyObject {
        self.charge(bytes.len());
        MontyObject::bytes(bytes.to_owned())
    }

    /// The repr fallback: a value with no snapshot form keeps the text Python would print for it.
    fn repr(&mut self, value: &Value, vm: &mut VM<'_>) -> MontyObject {
        let text = repr_string(value, vm);
        self.charge(text.len());
        MontyObject::repr(text)
    }
}

/// The text Python prints for a container reached again while printing it.
fn cycle_placeholder(data: &HeapData) -> &'static str {
    match data {
        HeapData::List(_) => "[...]",
        HeapData::Tuple(_) | HeapData::NamedTuple(_) => "(...)",
        HeapData::Dict(_) => "{...}",
        _ => "...",
    }
}

fn truncated() -> MontyObject {
    MontyObject::repr(TRUNCATED)
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
