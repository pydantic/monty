//! Snapshots of sandbox values for `ValidationError` payloads.
//!
//! An exception is plain data: it is copied on every catch and re-raise and
//! travels out of the sandbox, so it cannot hold heap references. A line
//! error's input therefore crosses into the exception as a [`MontyObject`]
//! built here, and comes back out as a fresh value when `errors()` reads it.
//! Scalars, containers, temporal values, decimals, complex numbers, paths and
//! exceptions survive the round trip; anything else (an instance, a function)
//! is kept as its repr in a `Repr` node. A reference back to an enclosing
//! container and the marker for whatever a spent budget cut off are `Cycle`
//! nodes, whose text prints raw.
//! One [`Snapshotter`] serves a whole exception, so shared sub-objects and
//! many line errors cannot multiply its size past the node and text budgets.

use std::mem;

use ahash::AHashSet;
use monty_types::{
    MontyObject, ResourceTracker,
    unstable::{self, MontyNode},
};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
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

/// Copied text from which a leaf, or a restored snapshot, is first checked
/// against the memory limit, so a huge input raises `MemoryError` rather than
/// overshooting it.
const LARGE_TEXT: usize = 1024 * 1024;

/// The marker that stands in for whatever a spent budget cut off.
const TRUNCATED: &str = "...";

/// Snapshots `value` with a fresh budget, for a one-off value such as a context entry.
pub(crate) fn snapshot_value(value: &Value, vm: &mut VM<'_>) -> RunResult<MontyObject> {
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
    ///
    /// Fails only with a resource error: a large leaf over the memory limit, or
    /// one raised by a user `__repr__`.
    pub(crate) fn snapshot(&mut self, value: &Value, vm: &mut VM<'_>) -> RunResult<MontyObject> {
        if self.spent() {
            return Ok(truncated());
        }
        self.nodes -= 1;
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(truncated());
        };
        let vm = &mut *guard;
        if let Value::Ref(id) = value {
            if self.in_progress.contains(id) {
                return Ok(MontyObject::cycle(cycle_placeholder(vm.heap.get(*id))));
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
    fn snapshot_node(&mut self, value: &Value, vm: &mut VM<'_>) -> RunResult<MontyObject> {
        let tracker = &vm.heap.tracker;
        Ok(match value {
            Value::None => MontyObject::none(),
            Value::Bool(b) => MontyObject::bool(*b),
            Value::Int(i) => MontyObject::int(*i),
            Value::Float(f) => MontyObject::float(*f),
            Value::InternString(id) => self.string(vm.interns.get_str(*id), tracker)?,
            Value::InlineString(text) => self.string(text.as_str(), tracker)?,
            Value::InternBytes(id) => self.bytes(vm.interns.get_bytes(*id), tracker)?,
            Value::InternLongInt(id) => MontyObject::bigint(vm.interns.get_long_int(*id).clone()),
            Value::Ref(id) => match vm.heap.read(*id) {
                HeapReadOutput::Str(s) => self.string(s.get(vm.heap).as_str(), tracker)?,
                HeapReadOutput::Bytes(b) => self.bytes(b.get(vm.heap).as_slice(), tracker)?,
                HeapReadOutput::LongInt(li) => MontyObject::bigint(li.get(vm.heap).inner().clone()),
                HeapReadOutput::List(list) => {
                    let items = list.get(vm.heap).as_slice();
                    let (len, children) = (items.len(), self.clone_prefix(items.iter(), vm.heap));
                    MontyObject::list(self.snapshot_all(children, len, vm)?)
                }
                HeapReadOutput::Tuple(tuple) => {
                    let items = tuple.get(vm.heap).as_slice();
                    let (len, children) = (items.len(), self.clone_prefix(items.iter(), vm.heap));
                    MontyObject::tuple(self.snapshot_all(children, len, vm)?)
                }
                HeapReadOutput::Set(set) => {
                    let set = set.get(vm.heap);
                    let (len, children) = (set.len(), self.clone_prefix(set.iter(), vm.heap));
                    MontyObject::set(self.snapshot_all(children, len, vm)?)
                }
                HeapReadOutput::FrozenSet(set) => {
                    let set = set.get(vm.heap);
                    let (len, children) = (set.len(), self.clone_prefix(set.storage().iter(), vm.heap));
                    MontyObject::frozenset(self.snapshot_all(children, len, vm)?)
                }
                HeapReadOutput::Dict(dict) => {
                    let dict = dict.get(vm.heap);
                    let len = dict.len();
                    let pairs: Vec<(Value, Value)> = dict
                        .iter()
                        .take(self.nodes)
                        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                        .collect();
                    MontyObject::dict(self.snapshot_pairs(pairs, len, vm)?)
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
                    let items = self.snapshot_all(children, len, vm)?;
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
                    None => self.repr(value, vm)?,
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
                HeapReadOutput::Complex(c) => {
                    let c = c.get(vm.heap);
                    MontyObject::complex(c.real, c.imag)
                }
                HeapReadOutput::Exception(exc) => {
                    let exc = exc.get(vm.heap);
                    self.copy_text(exc.arg().map_or(0, String::len), &vm.heap.tracker)?;
                    MontyObject::exception(exc.exc_type(), exc.arg().cloned())
                }
                HeapReadOutput::Path(path) => {
                    let path = path.get(vm.heap).as_str();
                    self.copy_text(path.len(), &vm.heap.tracker)?;
                    MontyObject::path(path)
                }
                _ => self.repr(value, vm)?,
            },
            _ => self.repr(value, vm)?,
        })
    }

    /// Clones the children the node budget can still reach; the rest are never visited.
    fn clone_prefix<'a>(&self, items: impl Iterator<Item = &'a Value>, heap: &impl ContainsHeap) -> Vec<Value> {
        items.take(self.nodes).map(|item| item.clone_with_heap(heap)).collect()
    }

    /// Snapshots `children`, the first of `len` items, ending with one
    /// [`TRUNCATED`] marker if the budget stops it short.
    fn snapshot_all(&mut self, children: Vec<Value>, len: usize, vm: &mut VM<'_>) -> RunResult<Vec<MontyObject>> {
        defer_drop!(children, vm);
        let mut out = Vec::with_capacity(children.len() + 1);
        for child in children {
            if self.spent() {
                break;
            }
            out.push(self.snapshot(child, vm)?);
        }
        if out.len() < len {
            out.push(truncated());
        }
        Ok(out)
    }

    /// [`Self::snapshot_all`] for a dict, a cut-off tail becoming one marker pair.
    fn snapshot_pairs(
        &mut self,
        pairs: Vec<(Value, Value)>,
        len: usize,
        vm: &mut VM<'_>,
    ) -> RunResult<Vec<(MontyObject, MontyObject)>> {
        defer_drop!(pairs, vm);
        let mut out = Vec::with_capacity(pairs.len() + 1);
        for (key, value) in pairs {
            if self.spent() {
                break;
            }
            out.push((self.snapshot(key, vm)?, self.snapshot(value, vm)?));
        }
        if out.len() < len {
            out.push((truncated(), truncated()));
        }
        Ok(out)
    }

    fn string(&mut self, text: &str, tracker: &ResourceTracker) -> RunResult<MontyObject> {
        self.copy_text(text.len(), tracker)?;
        Ok(MontyObject::string(text))
    }

    fn bytes(&mut self, bytes: &[u8], tracker: &ResourceTracker) -> RunResult<MontyObject> {
        self.copy_text(bytes.len(), tracker)?;
        Ok(MontyObject::bytes(bytes.to_owned()))
    }

    /// Charges a leaf about to be copied, first checking a large one against the memory limit.
    fn copy_text(&mut self, len: usize, tracker: &ResourceTracker) -> RunResult<()> {
        check_large_text(len, tracker)?;
        self.charge(len);
        Ok(())
    }

    /// The repr fallback: a value with no snapshot form keeps the text Python would print for it.
    fn repr(&mut self, value: &Value, vm: &mut VM<'_>) -> RunResult<MontyObject> {
        let text = try_repr_string(value, vm)?;
        self.charge(text.len());
        Ok(MontyObject::repr(text))
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
    MontyObject::cycle(TRUNCATED)
}

/// Checks `len` bytes of text about to be copied against the memory limit once it is large.
fn check_large_text(len: usize, tracker: &ResourceTracker) -> RunResult<()> {
    if len >= LARGE_TEXT {
        tracker.check_allocation(len)?;
    }
    Ok(())
}

/// `repr(value)` as a Rust string, or the type name when the repr itself fails.
///
/// Swallows every error, including resource errors; the limit that raised one
/// is hit again at the next checkpoint. Prefer [`try_repr_string`] where the
/// caller can propagate.
pub(crate) fn repr_string(value: &Value, vm: &mut VM<'_>) -> String {
    try_repr_string(value, vm).unwrap_or_else(|_| format!("<unprintable {} object>", value.py_type_name(vm)))
}

/// `repr(value)` as a Rust string; a Python exception from `__repr__` gives
/// pydantic's `<unprintable X object>`, while a resource or internal error propagates.
pub(crate) fn try_repr_string(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    match value.py_repr(vm) {
        Ok(repr) => {
            defer_drop!(repr, vm);
            Ok(repr.as_str(vm).map_or_else(String::new, str::to_owned))
        }
        Err(RunError::Exc(_)) => Ok(format!("<unprintable {} object>", value.py_type_name(vm))),
        Err(err) => Err(err),
    }
}

/// `str(value)` as a Rust string.
pub(crate) fn str_string(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let text = value.py_str(vm)?;
    defer_drop!(text, vm);
    Ok(text.as_str(vm).map_or_else(String::new, str::to_owned))
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

/// Rebuilds a snapshot as a live value, a repr fallback, cycle or truncation
/// marker coming back as the string it holds.
pub(crate) fn restore_value(snapshot: &MontyObject, vm: &mut VM<'_>) -> RunResult<Value> {
    let (graph, root) = unstable::graph_parts(snapshot);
    // The copy below and the values built from it each hold the snapshot's payload.
    let size: usize = graph.nodes().iter().map(MontyNode::decoded_size).sum();
    check_large_text(size.saturating_mul(2), &vm.heap.tracker)?;
    let mut graph = graph.clone();
    for node in graph.nodes_mut() {
        if let MontyNode::Repr(text) | MontyNode::Cycle(text) = node {
            *node = MontyNode::String(mem::take(text));
        }
    }
    let object = unstable::object_from_graph(graph, root).expect("a snapshot is a valid graph");
    object
        .to_value(vm)
        .map_err(|err| ExcType::value_error(format!("{err}")))
}
