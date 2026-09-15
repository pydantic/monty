//! Interpreter-side bridge for the boundary value types in `monty-types`:
//! export of VM `Value`s into a [`MontyGraph`] arena and import of an arena
//! back into heap values, plus the [`MontyType`] ↔ [`Type`] mirror. The
//! types themselves (and their pure methods) live in `monty-types`.
//!
//! Export walks the heap once per message with a memo, so a sub-object
//! reachable twice is one node referenced twice and the arena is
//! O(heap objects) rather than O(paths). Import builds every node in one
//! forward pass because a node's children always precede it.

use std::mem;

use ahash::{AHashMap, AHashSet};
use monty_types::{
    CallArgs, ClassTypeNode, InvalidInputError, MontyDate, MontyDateTime, MontyFileHandle, MontyGraph, MontyNode,
    MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, MontyValue, NodeId,
};

use crate::{
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{RunError, SimpleException},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapReadOutput},
    modules::dataclasses,
    types::{
        HostClass, HostClassType, LongInt, NamedTuple, OpenFile, Path, PyTrait, TimeZone, Type, allocate_tuple,
        bytes::Bytes,
        date as date_type, datetime as datetime_type,
        dict::Dict,
        instance::class_name,
        list::List,
        set::{FrozenSet, Set},
        str::allocate_string,
        time as time_type, timedelta as timedelta_type,
    },
    value::{EitherStr, Value},
};

/// Crate-internal conversions between a single [`MontyValue`] and a VM `Value`.
///
/// `MontyValue` lives in `monty-types`; building one from a heap `Value` (and
/// back) requires the VM, so the conversions stay here as a `pub(crate)`
/// extension trait. Multi-value messages use [`GraphExporter`] and
/// [`MontyGraphExt::to_values`] directly so their values share one arena.
pub(crate) trait MontyValueExt: Sized {
    /// Exports a `Value` into its own arena, taking ownership of the `Value`
    /// and dropping it via `drop_with`.
    fn export(value: Value, vm: &mut VM<'_>) -> Self;

    /// Imports this value into the heap. Fails with `InvalidInputError` on
    /// output-only nodes (`Repr`, `Cycle`), on a sandbox class or instance
    /// whose sandbox object no longer exists, and on malformed input; memory
    /// overshoot surfaces at the next soft-limit checkpoint, not here.
    fn to_value(self, vm: &mut VM<'_>) -> Result<Value, InvalidInputError>;
}

impl MontyValueExt for MontyValue {
    fn export(value: Value, vm: &mut VM<'_>) -> Self {
        let mut exporter = GraphExporter::new();
        let root = exporter.push_owned(value, vm);
        Self {
            graph: exporter.finish(),
            root,
        }
    }

    fn to_value(self, vm: &mut VM<'_>) -> Result<Value, InvalidInputError> {
        let mut values = self.graph.to_values(vm)?;
        // `None` is an immediate, so swapping it in leaves nothing to release.
        let root = mem::replace(&mut values[self.root.index()], Value::None);
        values.drop_with(vm);
        Ok(root)
    }
}

/// Crate-internal import of a whole arena.
pub(crate) trait MontyGraphExt {
    /// Imports every node into the heap, in order, returning one owned
    /// `Value` per node (so a root is `values[id.index()]`). The caller owns
    /// the vector and must `drop_with` it. Fails as
    /// [`MontyValueExt::to_value`] does, releasing everything built so far.
    fn to_values(self, vm: &mut VM<'_>) -> Result<Vec<Value>, InvalidInputError>;
}

impl MontyGraphExt for MontyGraph {
    fn to_values(self, vm: &mut VM<'_>) -> Result<Vec<Value>, InvalidInputError> {
        let nodes = self.into_nodes();
        // A host-defined class node resolves like a type object (sandbox
        // class first); an instance of it needs the host class instead, so
        // keep the node data for the instance path.
        let host_classes: AHashMap<NodeId, Box<ClassTypeNode>> = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| match node {
                MontyNode::ClassType(class) if class.host_defined => {
                    Some((NodeId(u32::try_from(index).expect("arena ids fit u32")), class.clone()))
                }
                _ => None,
            })
            .collect();
        let mut guard = DropGuard::new(Vec::with_capacity(nodes.len()), vm);
        let (values, vm) = guard.as_parts_mut();
        for node in nodes {
            let value = import_node(node, values, &host_classes, vm)?;
            values.push(value);
        }
        Ok(guard.into_inner())
    }
}

/// Builds the arena for one outgoing message: the final value, or every
/// argument of a call. One exporter per message is what lets an object
/// reachable from two arguments cross once.
///
/// `done` memoizes finished heap objects; `in_progress` holds the ones on the
/// current descent, so a reference back to one of them becomes a
/// [`MontyNode::Cycle`] leaf rather than infinite recursion.
pub(crate) struct GraphExporter {
    graph: MontyGraph,
    done: AHashMap<HeapId, NodeId>,
    in_progress: AHashSet<HeapId>,
}

impl GraphExporter {
    pub(crate) fn new() -> Self {
        Self {
            graph: MontyGraph::new(),
            done: AHashMap::new(),
            in_progress: AHashSet::new(),
        }
    }

    /// Exports an owned value, dropping it, and returns its node.
    pub(crate) fn push_owned(&mut self, value: Value, vm: &mut VM<'_>) -> NodeId {
        let id = self.push(&value, vm);
        value.drop_with(vm);
        id
    }

    /// Appends a leaf node (e.g. an interned keyword name) directly.
    pub(crate) fn push_node(&mut self, node: MontyNode) -> NodeId {
        self.graph.push(node)
    }

    /// The finished arena.
    pub(crate) fn finish(self) -> MontyGraph {
        self.graph
    }

    /// Exports a borrowed value and returns its node.
    ///
    /// Non-`Ref` variants are produced inline using only the interner — they
    /// never recurse through the heap. `Ref` variants dispatch via
    /// `vm.heap.read(id)` so the resulting `HeapRead` keeps the heap entry
    /// alive (through its reader count) without retaining a borrow on
    /// `vm.heap`. Recursing can run a user-defined `__repr__` (via
    /// [`repr_or_error`] on nested instances), so mutable containers (list,
    /// dict, set, dataclass attrs) snapshot ALL children up front — the
    /// `inc_ref`s keep each child alive and the snapshot keeps iteration valid
    /// even if that `__repr__` mutates the container. Immutable containers
    /// (tuple, namedtuple, frozenset) clone per-item: their length and slots
    /// cannot change mid-iteration.
    pub(crate) fn push(&mut self, value: &Value, vm: &mut VM<'_>) -> NodeId {
        // Check depth limit before processing
        let Ok(mut guard) = vm.recursion_guard() else {
            return self.push_node(MontyNode::Repr("<deeply nested>".to_owned()));
        };
        let vm = &mut *guard;

        let interns = vm.interns;
        let node = match value {
            Value::Undefined => panic!("Undefined found while exporting a value"),
            Value::Ellipsis => MontyNode::Ellipsis,
            Value::NotImplemented => MontyNode::NotImplemented,
            Value::None => MontyNode::None,
            Value::Bool(b) => MontyNode::Bool(*b),
            Value::Int(i) => MontyNode::Int(*i),
            Value::Float(f) => MontyNode::Float(*f),
            Value::InternString(string_id) => MontyNode::String(interns.get_str(*string_id).to_owned()),
            Value::InternBytes(bytes_id) => MontyNode::Bytes(interns.get_bytes(*bytes_id).to_owned()),
            Value::InternLongInt(li_id) => MontyNode::BigInt(interns.get_long_int(*li_id).clone()),
            Value::Ref(id) => return self.push_ref(*id, value, vm),
            Value::Builtin(Builtins::Type(Type::Instance(class_id))) => return self.sandbox_class_node(*class_id, vm),
            Value::Builtin(Builtins::Type(t)) => match MontyType::from_internal_static(*t) {
                Some(ty) => MontyNode::Type(ty),
                None => repr_or_error(value, vm),
            },
            Value::Builtin(Builtins::ExcType(e)) => MontyNode::Type(MontyType::Exception(*e)),
            Value::Builtin(Builtins::Function(f)) => MontyNode::BuiltinFunction(*f),
            #[cfg(feature = "memory-model-checks")]
            Value::Dereferenced => panic!("Dereferenced found while exporting a value"),
            _ => repr_or_error(value, vm),
        };
        self.push_node(node)
    }

    /// Exports a heap object through the memo: a finished object is reused, one
    /// still being exported is a cycle, anything else is converted then recorded.
    fn push_ref(&mut self, id: HeapId, value: &Value, vm: &mut VM<'_>) -> NodeId {
        if let Some(node_id) = self.done.get(&id) {
            return *node_id;
        }
        // A host class's type object *is* its shared class node.
        if matches!(vm.heap.get(id), HeapData::HostClassType(_)) {
            return self.host_class_node(id, vm);
        }
        if self.in_progress.contains(&id) {
            let placeholder = match vm.heap.get(id) {
                // A deque exports as a list, so it takes a list's placeholder
                // (as its repr does too).
                HeapData::List(_) | HeapData::Deque(_) => "[...]",
                HeapData::Tuple(_) | HeapData::NamedTuple(_) => "(...)",
                HeapData::Dict(_) => "{...}",
                _ => "...",
            };
            return self.push_node(MontyNode::Cycle(placeholder.to_owned()));
        }
        self.in_progress.insert(id);
        let node_id = match vm.heap.read(id) {
            // Cells are internal closure implementation details — export the
            // contents directly without a wrapper node.
            HeapReadOutput::Cell(cell) => {
                let inner = cell.get(vm.heap).0.clone_with_heap(vm.heap);
                defer_drop!(inner, vm);
                self.push(inner, vm)
            }
            output => {
                let node = self.convert_heap_object(id, output, value, vm);
                self.push_node(node)
            }
        };
        self.in_progress.remove(&id);
        self.done.insert(id, node_id);
        node_id
    }

    /// Converts one heap object into its node; container children are pushed
    /// first so their ids exist.
    fn convert_heap_object<'h>(
        &mut self,
        id: HeapId,
        output: HeapReadOutput<'h>,
        value: &Value,
        vm: &mut VM<'h>,
    ) -> MontyNode {
        match output {
            HeapReadOutput::Str(s) => MontyNode::String(s.get(vm.heap).as_str().to_owned()),
            HeapReadOutput::Bytes(b) => MontyNode::Bytes(b.get(vm.heap).as_slice().to_owned()),
            HeapReadOutput::List(list) => {
                // Snapshot before recursing: a nested `__repr__` may mutate this list.
                let children: Vec<Value> = list
                    .get(vm.heap)
                    .as_slice()
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyNode::List(self.push_all(children, vm))
            }
            // A deque exports as a host list: there is no host-side deque
            // *value* type, so it degrades to the nearest structural one
            // rather than to a repr string, matching how defaultdict and
            // Counter degrade to `dict`. `maxlen` does not survive, and the
            // host cannot round-trip it back into a deque.
            HeapReadOutput::Deque(deque) => {
                // Snapshot before recursing: a deque is mutable, so a nested
                // `__repr__` may shorten it and invalidate index-based access.
                let children: Vec<Value> = deque
                    .get(vm.heap)
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(children, vm);
                MontyNode::List(self.push_all(children, vm))
            }
            HeapReadOutput::Tuple(tuple) => {
                let len = tuple.get(vm.heap).as_slice().len();
                let mut items = Vec::with_capacity(len);
                for i in 0..len {
                    let item = tuple.get(vm.heap).as_slice()[i].clone_with_heap(vm.heap);
                    defer_drop!(item, vm);
                    items.push(self.push(item, vm));
                }
                MontyNode::Tuple(items)
            }
            HeapReadOutput::NamedTuple(nt) => {
                let type_name = nt.get(vm.heap).name(vm.interns).to_owned();
                let field_names = nt
                    .get(vm.heap)
                    .field_names()
                    .iter()
                    .map(|fname| fname.as_str(vm.interns).to_owned())
                    .collect::<Vec<_>>();
                let len = nt.get(vm.heap).len();
                let mut values = Vec::with_capacity(len);
                for i in 0..len {
                    let item = nt.get(vm.heap).as_vec()[i].clone_with_heap(vm.heap);
                    defer_drop!(item, vm);
                    values.push(self.push(item, vm));
                }
                MontyNode::NamedTuple {
                    type_name,
                    field_names,
                    values,
                }
            }
            HeapReadOutput::Dict(dict) => {
                // Snapshot before recursing: a nested `__repr__` may mutate this dict.
                let children = snapshot_dict_pairs(dict.get(vm.heap), vm.heap);
                defer_drop!(children, vm);
                MontyNode::Dict(self.push_pairs(children, vm))
            }
            HeapReadOutput::Set(set) => {
                // Snapshot before recursing: a nested `__repr__` may mutate this set.
                let children: Vec<Value> = {
                    let set_ref = set.get(vm.heap);
                    (0..set_ref.len())
                        .map(|i| {
                            set_ref
                                .storage()
                                .value_at(i)
                                .expect("index in range")
                                .clone_with_heap(vm.heap)
                        })
                        .collect()
                };
                defer_drop!(children, vm);
                MontyNode::Set(self.push_all(children, vm))
            }
            HeapReadOutput::FrozenSet(fs) => {
                let len = fs.get(vm.heap).len();
                let mut items = Vec::with_capacity(len);
                for i in 0..len {
                    let item = fs
                        .get(vm.heap)
                        .storage()
                        .value_at(i)
                        .expect("index in range")
                        .clone_with_heap(vm.heap);
                    defer_drop!(item, vm);
                    items.push(self.push(item, vm));
                }
                MontyNode::FrozenSet(items)
            }
            HeapReadOutput::Cell(_) | HeapReadOutput::HostClassType(_) => {
                unreachable!("cells and host class types are routed by push_ref")
            }
            HeapReadOutput::Date(d) => {
                let (year, month, day) = date_type::to_ymd(*d.get(vm.heap));
                MontyNode::Date(MontyDate {
                    year,
                    month: u8::try_from(month).expect("month is always 1..=12"),
                    day: u8::try_from(day).expect("day is always 1..=31"),
                })
            }
            HeapReadOutput::DateTime(dt) => {
                if let Some((year, month, day, hour, minute, second, microsecond)) =
                    datetime_type::to_components(dt.get(vm.heap))
                {
                    MontyNode::DateTime(MontyDateTime {
                        year,
                        month,
                        day,
                        hour,
                        minute,
                        second,
                        microsecond,
                        offset_seconds: datetime_type::offset_seconds(dt.get(vm.heap)),
                        timezone_name: datetime_type::timezone_info(dt.get(vm.heap)).and_then(|tz| tz.name),
                    })
                } else {
                    repr_or_error(value, vm)
                }
            }
            HeapReadOutput::Time(t) => {
                let time = t.get(vm.heap);
                let (hour, minute, second, microsecond, fold) = time.to_components();
                let tz = time_type::attached_timezone(time, vm.heap);
                MontyNode::Time(MontyTime {
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
                MontyNode::TimeDelta(MontyTimeDelta {
                    days,
                    seconds,
                    microseconds,
                })
            }
            HeapReadOutput::TimeZone(tz) => {
                let tz_ref = tz.get(vm.heap);
                MontyNode::TimeZone(MontyTimeZone {
                    offset_seconds: tz_ref.offset_seconds,
                    name: tz_ref.name.clone(),
                })
            }
            HeapReadOutput::Exception(exc) => {
                let exc_ref = exc.get(vm.heap);
                MontyNode::Exception {
                    exc_type: exc_ref.exc_type(),
                    arg: exc_ref.arg().map(ToString::to_string),
                }
            }
            HeapReadOutput::HostClass(hc) => {
                let (class_id, instance_id) = {
                    let hc_ref = hc.get(vm.heap);
                    (hc_ref.class_id(), hc_ref.instance_id())
                };
                let class_type = self.host_class_node(class_id, vm);
                // Snapshot before recursing: attrs are mutable via `setattr`.
                let children = snapshot_dict_pairs(hc.get(vm.heap).attrs(), vm.heap);
                defer_drop!(children, vm);
                MontyNode::ClassInstance {
                    class_type,
                    instance_id,
                    attrs: self.push_pairs(children, vm),
                }
            }
            // Sandbox-defined class instances cross out structured rather
            // than as a repr string, so hosts get name + attrs + dataclass-ness,
            // plus worker-generated uuids (stored on the heap objects, so
            // stable across crossings and dump/restore).
            HeapReadOutput::Instance(inst) => {
                let class_id = inst.get(vm.heap).class();
                let instance_id = vm.heap.boundary_uuid(id);
                let class_type = self.sandbox_class_node(class_id, vm);
                // Snapshot before recursing: attrs are mutable via `setattr`.
                let children = snapshot_dict_pairs(inst.get(vm.heap).attrs(), vm.heap);
                defer_drop!(children, vm);
                MontyNode::ClassInstance {
                    class_type,
                    instance_id,
                    attrs: self.push_pairs(children, vm),
                }
            }
            // Iterators are internal objects — represent as a fixed type
            // string rather than recursing.
            HeapReadOutput::ListIterator(_) => MontyNode::Repr("<list_iterator object>".to_owned()),
            HeapReadOutput::TupleIterator(_) => MontyNode::Repr("<tuple_iterator object>".to_owned()),
            HeapReadOutput::StringIterator(iter) => {
                MontyNode::Repr(format!("<{} object>", iter.py_type(vm).name(vm.heap, vm.interns)))
            }
            HeapReadOutput::BytesIterator(_) => MontyNode::Repr("<bytes_iterator object>".to_owned()),
            HeapReadOutput::RangeIterator(_) => MontyNode::Repr("<range_iterator object>".to_owned()),
            HeapReadOutput::DictKeyIterator(_) => MontyNode::Repr("<dict_keyiterator object>".to_owned()),
            HeapReadOutput::DictItemIterator(_) => MontyNode::Repr("<dict_itemiterator object>".to_owned()),
            HeapReadOutput::DictValueIterator(_) => MontyNode::Repr("<dict_valueiterator object>".to_owned()),
            HeapReadOutput::SetIterator(_) => MontyNode::Repr("<set_iterator object>".to_owned()),
            HeapReadOutput::CallableIterator(_) => MontyNode::Repr("<callable_iterator object>".to_owned()),
            // A placeholder despite the real in-sandbox repr (`count(0)`),
            // which would recurse into `repeat`'s arbitrary object.
            HeapReadOutput::Itertools(iter) => {
                MontyNode::Repr(format!("<{} object>", iter.py_type(vm).name(vm.heap, vm.interns)))
            }
            HeapReadOutput::LongInt(li) => MontyNode::BigInt(li.get(vm.heap).inner().clone()),
            HeapReadOutput::Module(m) => {
                MontyNode::Repr(format!("<module '{}'>", vm.interns.get_str(m.get(vm.heap).name())))
            }
            HeapReadOutput::Coroutine(coro) => {
                let func_id = coro.get(vm.heap).func_id;
                let func = vm.interns.get_function(func_id);
                let name = vm.interns.get_str(func.name.name_id);
                MontyNode::Repr(format!("<coroutine object {name}>"))
            }
            HeapReadOutput::GatherFuture(gather) => {
                MontyNode::Repr(format!("<gather({})>", gather.get(vm.heap).item_count()))
            }
            HeapReadOutput::Path(path) => MontyNode::Path(path.get(vm.heap).as_str().to_owned()),
            // File objects carry no heap refs (leaf type) — no recursion.
            // This is how `file.read()`/`write()` deliver the open file
            // to the host as the first OS-call argument.
            HeapReadOutput::OpenFile(file) => {
                let file = file.get(vm.heap);
                MontyNode::FileHandle(MontyFileHandle {
                    path: file.path().to_owned(),
                    mode: *file.file_mode(),
                    position: file.position(),
                })
            }
            HeapReadOutput::ExtFunction(function) => MontyNode::Function {
                name: function.get(vm.heap).as_str().to_owned(),
                docstring: None,
            },
            _ => repr_or_error(value, vm),
        }
    }

    /// The class node of a sandbox-defined class, generating and storing its
    /// boundary uuid on first crossing so repeated crossings (and
    /// dump/restore) observe the same id. Attrs are never sent for sandbox
    /// classes, so the node has no children and is memoized on the class.
    ///
    /// # Panics
    /// If `class_id` does not refer to a `Class` heap entry — every producer of a
    /// class id guarantees it does, so this is a programmer-error tripwire.
    fn sandbox_class_node(&mut self, class_id: HeapId, vm: &mut VM<'_>) -> NodeId {
        if let Some(node_id) = self.done.get(&class_id) {
            return *node_id;
        }
        let node = MontyNode::ClassType(Box::new(ClassTypeNode {
            name: class_name(class_id, vm.heap, vm.interns).into_owned(),
            id: vm.heap.boundary_uuid(class_id),
            host_defined: false,
            is_dataclass: dataclasses::is_dataclass_class(class_id, vm),
            attrs: Vec::new(),
        }));
        let node_id = self.push_node(node);
        self.done.insert(class_id, node_id);
        node_id
    }

    /// The class node of a host-defined class (`type_id` is its `HostClassType`
    /// entry), carrying its eager class attrs so an echoed type round-trips
    /// intact. Memoized so every instance of the class, and the type object
    /// itself, share one node; a class reached again while its own attrs are
    /// still being exported gets an attr-less duplicate, since a
    /// `ClassInstance` must point at a class node, never a `Cycle` leaf.
    fn host_class_node(&mut self, type_id: HeapId, vm: &mut VM<'_>) -> NodeId {
        if let Some(node_id) = self.done.get(&type_id) {
            return *node_id;
        }
        let HeapReadOutput::HostClassType(ty) = vm.heap.read(type_id) else {
            unreachable!("host class ids always point at a HostClassType entry");
        };
        let class_type = ty.get(vm.heap).class_type(vm.interns);
        let attrs = if self.in_progress.insert(type_id) {
            let children = snapshot_dict_pairs(ty.get(vm.heap).attrs(), vm.heap);
            defer_drop!(children, vm);
            let attrs = self.push_pairs(children, vm);
            self.in_progress.remove(&type_id);
            attrs
        } else {
            Vec::new()
        };
        let node_id = self.push_node(MontyNode::ClassType(Box::new(ClassTypeNode {
            name: class_type.name,
            id: class_type.id,
            host_defined: true,
            is_dataclass: class_type.is_dataclass,
            attrs,
        })));
        if !self.in_progress.contains(&type_id) {
            self.done.insert(type_id, node_id);
        }
        node_id
    }

    /// Exports a guarded snapshot of container children.
    ///
    /// Taking a `&[Value]` snapshot (cloned and guarded by the caller) is what
    /// keeps [`push`](Self::push) safe against a nested `__repr__` mutating the
    /// source container mid-iteration.
    fn push_all(&mut self, children: &[Value], vm: &mut VM<'_>) -> Vec<NodeId> {
        children.iter().map(|child| self.push(child, vm)).collect()
    }

    /// Exports a guarded snapshot of dict entries — the pair-wise counterpart
    /// of [`push_all`](Self::push_all).
    fn push_pairs(&mut self, children: &[(Value, Value)], vm: &mut VM<'_>) -> Vec<(NodeId, NodeId)> {
        children
            .iter()
            .map(|(key, value)| {
                let key = self.push(key, vm);
                let value = self.push(value, vm);
                (key, value)
            })
            .collect()
    }
}

/// Crate-internal export of call arguments: one arena for every argument.
pub(crate) trait CallArgsExt {
    /// Appends an owned positional argument.
    fn export_arg(&mut self, exporter: &mut GraphExporter, value: Value, vm: &mut VM<'_>);
    /// Appends an owned keyword argument.
    fn export_kwarg(&mut self, exporter: &mut GraphExporter, key: Value, value: Value, vm: &mut VM<'_>);
}

impl CallArgsExt for CallArgs {
    fn export_arg(&mut self, exporter: &mut GraphExporter, value: Value, vm: &mut VM<'_>) {
        let id = exporter.push_owned(value, vm);
        self.args.push(id);
    }

    fn export_kwarg(&mut self, exporter: &mut GraphExporter, key: Value, value: Value, vm: &mut VM<'_>) {
        let key = exporter.push_owned(key, vm);
        let value = exporter.push_owned(value, vm);
        self.kwargs.push((key, value));
    }
}

/// Crate-internal bridge between [`MontyType`] and the runtime [`Type`].
///
/// `MontyType` lives in `monty-types` (it is pure data), but mapping it to and
/// from the runtime `Type` needs heap/intern access, so the conversions stay
/// here as a `pub(crate)` extension trait.
pub(crate) trait MontyTypeExt: Sized {
    fn to_internal(&self) -> Option<Type>;

    fn from_internal_static(ty: Type) -> Option<Self>;
}

impl MontyTypeExt for MontyType {
    /// The internal runtime [`Type`] this variant mirrors, or `None` for
    /// [`Instance`](Self::Instance) — a class binding cannot be reconstructed
    /// from a name, which is exactly why `Instance` inputs are rejected.
    ///
    /// Keep in lockstep with [`from_internal_static`](Self::from_internal_static);
    /// both matches are exhaustive so the compiler enforces totality.
    fn to_internal(&self) -> Option<Type> {
        match self {
            Self::Ellipsis => Some(Type::Ellipsis),
            Self::NotImplementedType => Some(Type::NotImplementedType),
            Self::Type => Some(Type::Type),
            Self::NoneType => Some(Type::NoneType),
            Self::Bool => Some(Type::Bool),
            Self::Int => Some(Type::Int),
            Self::Float => Some(Type::Float),
            Self::Range => Some(Type::Range),
            Self::Slice => Some(Type::Slice),
            Self::Date => Some(Type::Date),
            Self::DateTime => Some(Type::DateTime),
            Self::Time => Some(Type::Time),
            Self::TimeDelta => Some(Type::TimeDelta),
            Self::TimeZone => Some(Type::TimeZone),
            Self::Str => Some(Type::Str),
            Self::Bytes => Some(Type::Bytes),
            Self::List => Some(Type::List),
            Self::Deque => Some(Type::Deque),
            Self::ListIterator => Some(Type::ListIterator),
            Self::TupleIterator => Some(Type::TupleIterator),
            Self::StrAsciiIterator => Some(Type::StrAsciiIterator),
            Self::StrIterator => Some(Type::StrIterator),
            Self::BytesIterator => Some(Type::BytesIterator),
            Self::RangeIterator => Some(Type::RangeIterator),
            Self::DictKeyIterator => Some(Type::DictKeyIterator),
            Self::DictItemIterator => Some(Type::DictItemIterator),
            Self::DictValueIterator => Some(Type::DictValueIterator),
            Self::SetIterator => Some(Type::SetIterator),
            Self::CallableIterator => Some(Type::CallableIterator),
            Self::ItertoolsPairwise => Some(Type::ItertoolsPairwise),
            Self::ItertoolsCompress => Some(Type::ItertoolsCompress),
            Self::ItertoolsIslice => Some(Type::ItertoolsIslice),
            Self::ItertoolsChain => Some(Type::ItertoolsChain),
            Self::ItertoolsCycle => Some(Type::ItertoolsCycle),
            Self::ItertoolsTakeWhile => Some(Type::ItertoolsTakeWhile),
            Self::ItertoolsDropWhile => Some(Type::ItertoolsDropWhile),
            Self::ItertoolsFilterFalse => Some(Type::ItertoolsFilterFalse),
            Self::ItertoolsStarMap => Some(Type::ItertoolsStarMap),
            Self::ItertoolsAccumulate => Some(Type::ItertoolsAccumulate),
            Self::ItertoolsBatched => Some(Type::ItertoolsBatched),
            Self::ItertoolsZipLongest => Some(Type::ItertoolsZipLongest),
            Self::ItertoolsCombinations => Some(Type::ItertoolsCombinations),
            Self::ItertoolsCombinationsWithReplacement => Some(Type::ItertoolsCombinationsWithReplacement),
            Self::ItertoolsPermutations => Some(Type::ItertoolsPermutations),
            Self::ItertoolsProduct => Some(Type::ItertoolsProduct),
            Self::ItertoolsGroupBy => Some(Type::ItertoolsGroupBy),
            Self::ItertoolsGrouper => Some(Type::ItertoolsGrouper),
            Self::ItertoolsTee => Some(Type::ItertoolsTee),
            Self::ItertoolsTeeDataObject => Some(Type::ItertoolsTeeDataObject),
            Self::ItertoolsCount => Some(Type::ItertoolsCount),
            Self::ItertoolsRepeat => Some(Type::ItertoolsRepeat),
            Self::Partial => Some(Type::Partial),
            Self::GenericAlias => Some(Type::GenericAlias),
            Self::Union => Some(Type::Union),
            Self::Tuple => Some(Type::Tuple),
            Self::NamedTuple => Some(Type::NamedTuple),
            Self::Dict => Some(Type::Dict),
            Self::DictKeys => Some(Type::DictKeys),
            Self::DictItems => Some(Type::DictItems),
            Self::DictValues => Some(Type::DictValues),
            Self::Set => Some(Type::Set),
            Self::FrozenSet => Some(Type::FrozenSet),
            Self::Instance(_) => None,
            Self::Exception(exc_type) => Some(Type::Exception(*exc_type)),
            Self::Function => Some(Type::Function),
            Self::BuiltinFunction => Some(Type::BuiltinFunction),
            Self::Cell => Some(Type::Cell),
            Self::Iterator => Some(Type::Iterator),
            Self::Coroutine => Some(Type::Coroutine),
            Self::Module => Some(Type::Module),
            Self::TextIOWrapper => Some(Type::TextIOWrapper),
            Self::BufferedReader => Some(Type::BufferedReader),
            Self::BufferedWriter => Some(Type::BufferedWriter),
            Self::BufferedRandom => Some(Type::BufferedRandom),
            Self::SpecialForm => Some(Type::SpecialForm),
            Self::Path => Some(Type::Path),
            Self::Property => Some(Type::Property),
            Self::Object => Some(Type::Object),
            Self::RePattern => Some(Type::RePattern),
            Self::ReMatch => Some(Type::ReMatch),
            Self::Field => Some(Type::DataclassField),
            Self::DataclassParams => Some(Type::DataclassParams),
        }
    }

    /// Mirrors a runtime [`Type`] whose class identity is NOT needed; a
    /// sandbox class (`Type::Instance`) exports as a class node instead, and a
    /// type with no boundary form returns `None` to cross as a repr.
    ///
    /// # Panics
    /// On `Instance`, whose class name cannot be resolved without a heap; the
    /// exporter routes it to [`GraphExporter::sandbox_class_node`] first.
    fn from_internal_static(ty: Type) -> Option<Self> {
        Some(match ty {
            Type::Ellipsis => Self::Ellipsis,
            Type::NotImplementedType => Self::NotImplementedType,
            Type::Type => Self::Type,
            Type::NoneType => Self::NoneType,
            Type::Bool => Self::Bool,
            Type::Int => Self::Int,
            Type::Float => Self::Float,
            Type::Range => Self::Range,
            Type::Slice => Self::Slice,
            Type::Date => Self::Date,
            Type::DateTime => Self::DateTime,
            Type::Time => Self::Time,
            Type::TimeDelta => Self::TimeDelta,
            Type::TimeZone => Self::TimeZone,
            Type::Str => Self::Str,
            Type::Bytes => Self::Bytes,
            Type::List => Self::List,
            Type::Deque => Self::Deque,
            // No dedicated host-side deque-iterator type; it crosses as the
            // generic `iterator`, like any iterator without a MontyType variant.
            Type::DequeIterator => Self::Iterator,
            Type::ListIterator => Self::ListIterator,
            Type::TupleIterator => Self::TupleIterator,
            Type::StrAsciiIterator => Self::StrAsciiIterator,
            Type::StrIterator => Self::StrIterator,
            Type::BytesIterator => Self::BytesIterator,
            Type::RangeIterator => Self::RangeIterator,
            Type::DictKeyIterator => Self::DictKeyIterator,
            Type::DictItemIterator => Self::DictItemIterator,
            Type::DictValueIterator => Self::DictValueIterator,
            Type::SetIterator => Self::SetIterator,
            Type::CallableIterator => Self::CallableIterator,
            Type::ItertoolsPairwise => Self::ItertoolsPairwise,
            Type::ItertoolsCompress => Self::ItertoolsCompress,
            Type::ItertoolsIslice => Self::ItertoolsIslice,
            Type::ItertoolsChain => Self::ItertoolsChain,
            Type::ItertoolsCycle => Self::ItertoolsCycle,
            Type::ItertoolsTakeWhile => Self::ItertoolsTakeWhile,
            Type::ItertoolsDropWhile => Self::ItertoolsDropWhile,
            Type::ItertoolsFilterFalse => Self::ItertoolsFilterFalse,
            Type::ItertoolsStarMap => Self::ItertoolsStarMap,
            Type::ItertoolsAccumulate => Self::ItertoolsAccumulate,
            Type::ItertoolsBatched => Self::ItertoolsBatched,
            Type::ItertoolsZipLongest => Self::ItertoolsZipLongest,
            Type::ItertoolsCombinations => Self::ItertoolsCombinations,
            Type::ItertoolsCombinationsWithReplacement => Self::ItertoolsCombinationsWithReplacement,
            Type::ItertoolsPermutations => Self::ItertoolsPermutations,
            Type::ItertoolsProduct => Self::ItertoolsProduct,
            Type::ItertoolsGroupBy => Self::ItertoolsGroupBy,
            Type::ItertoolsGrouper => Self::ItertoolsGrouper,
            Type::ItertoolsTee => Self::ItertoolsTee,
            Type::ItertoolsTeeDataObject => Self::ItertoolsTeeDataObject,
            Type::ItertoolsCount => Self::ItertoolsCount,
            Type::ItertoolsRepeat => Self::ItertoolsRepeat,
            Type::Partial => Self::Partial,
            Type::GenericAlias => Self::GenericAlias,
            Type::Union => Self::Union,
            Type::Random => return None,
            Type::Tuple => Self::Tuple,
            Type::NamedTuple => Self::NamedTuple,
            Type::Dict => Self::Dict,
            // No host-side defaultdict/Counter type; both degrade to `dict`,
            // consistent with their values crossing as `MontyNode::Dict`.
            Type::DefaultDict | Type::Counter => Self::Dict,
            Type::DictKeys => Self::DictKeys,
            Type::DictItems => Self::DictItems,
            Type::DictValues => Self::DictValues,
            Type::Set => Self::Set,
            Type::FrozenSet => Self::FrozenSet,
            Type::Instance(_) => unreachable!("Type::Instance requires heap access — exported as a class node"),
            // The interpreter-internal placeholder never becomes a value:
            // `type(x)` on a host instance materializes a `HostClassType`.
            Type::HostClass => {
                unreachable!("Type::HostClass has no boundary mirror — host instances cross as class nodes")
            }
            Type::Exception(exc_type) => Self::Exception(exc_type),
            Type::Function => Self::Function,
            Type::BuiltinFunction => Self::BuiltinFunction,
            Type::Cell => Self::Cell,
            Type::Iterator => Self::Iterator,
            Type::Coroutine => Self::Coroutine,
            Type::Module => Self::Module,
            Type::TextIOWrapper => Self::TextIOWrapper,
            Type::BufferedReader => Self::BufferedReader,
            Type::BufferedWriter => Self::BufferedWriter,
            Type::BufferedRandom => Self::BufferedRandom,
            Type::SpecialForm => Self::SpecialForm,
            Type::Path => Self::Path,
            Type::Property => Self::Property,
            Type::Object => Self::Object,
            Type::RePattern => Self::RePattern,
            Type::ReMatch => Self::ReMatch,
            Type::DataclassField => Self::Field,
            Type::DataclassParams => Self::DataclassParams,
        })
    }
}

/// Imports one node whose children are already in `built` (every child id is
/// lower, so they always are). A child used twice becomes a shared heap
/// object: the sandbox sees the identity the host sent. `host_classes` holds
/// the host-defined class nodes, by id, for instances of them.
///
/// Immediate values (None, Bool, Int, Float, Ellipsis, Exception) are created
/// directly; heap values are allocated and wrapped in `Value::Ref`.
fn import_node(
    node: MontyNode,
    built: &[Value],
    host_classes: &AHashMap<NodeId, Box<ClassTypeNode>>,
    vm: &mut VM<'_>,
) -> Result<Value, InvalidInputError> {
    match node {
        MontyNode::Ellipsis => Ok(Value::Ellipsis),
        MontyNode::NotImplemented => Ok(Value::NotImplemented),
        MontyNode::None => Ok(Value::None),
        MontyNode::Bool(b) => Ok(Value::Bool(b)),
        MontyNode::Int(i) => Ok(Value::Int(i)),
        MontyNode::BigInt(bi) => Ok(LongInt::new(bi).into_value(vm.heap)),
        MontyNode::Float(f) => Ok(Value::Float(f)),
        MontyNode::String(s) => Ok(allocate_string(s, vm.heap)),
        MontyNode::Bytes(b) => Ok(Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(b))))),
        MontyNode::List(ids) => {
            let values = import_children(&ids, built, vm);
            Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(values)))))
        }
        MontyNode::Tuple(ids) => {
            let values = import_children(&ids, built, vm);
            Ok(allocate_tuple(values.into(), vm.heap))
        }
        MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        } => {
            // `NamedTuple::new` asserts equal lengths; malformed host input
            // (e.g. untrusted serialized data) must error, not panic.
            if field_names.len() != values.len() {
                return Err(InvalidInputError::invalid_type(
                    "NamedTuple field_names and values must have the same length",
                ));
            }
            let values = import_children(&values, built, vm);
            let field_name_strs: Vec<EitherStr> = field_names.into_iter().map(Into::into).collect();
            let nt = NamedTuple::new(type_name, field_name_strs, values);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(nt)))))
        }
        MontyNode::Dict(pairs) => {
            let pairs = import_pairs(&pairs, built, vm);
            let dict =
                Dict::from_pairs(pairs, vm).map_err(|_| InvalidInputError::invalid_type("unhashable dict keys"))?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
        }
        MontyNode::Set(ids) => {
            let set = import_set(&ids, built, vm, "unhashable set element")?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::Set(set))))
        }
        MontyNode::FrozenSet(ids) => {
            let set = import_set(&ids, built, vm, "unhashable frozenset element")?;
            let frozenset = FrozenSet::from_set(set);
            Ok(Value::Ref(vm.heap.allocate(HeapData::FrozenSet(frozenset))))
        }
        MontyNode::Date(date) => {
            let value = date_type::from_ymd(date.year, i32::from(date.month), i32::from(date.day))
                .map_err(|_| InvalidInputError::invalid_type("date"))?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::Date(value))))
        }
        MontyNode::DateTime(datetime) => {
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
            Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(value))))
        }
        MontyNode::Time(time) => {
            let MontyTime {
                hour,
                minute,
                second,
                microsecond,
                offset_seconds,
                timezone_name,
                fold,
            } = time;
            if offset_seconds.is_none() && timezone_name.is_some() {
                return Err(InvalidInputError::invalid_type("time"));
            }
            let tzinfo = offset_seconds
                .map(|offset| TimeZone::new(offset, timezone_name))
                .transpose()
                .map_err(|_| InvalidInputError::invalid_type("time"))?;
            let value = time_type::from_boundary_components(
                i32::from(hour),
                i32::from(minute),
                i32::from(second),
                i32::try_from(microsecond).map_err(|_| InvalidInputError::invalid_type("time"))?,
                i32::from(fold),
                tzinfo,
                vm.heap,
            )
            .map_err(|_| InvalidInputError::invalid_type("time"))?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::Time(value))))
        }
        MontyNode::TimeDelta(delta) => {
            let delta = timedelta_type::new(delta.days, delta.seconds, delta.microseconds)
                .map_err(|_| InvalidInputError::invalid_type("timedelta"))?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))))
        }
        MontyNode::TimeZone(tz) => {
            if tz.offset_seconds == 0 && tz.name.is_none() {
                Ok(vm.heap.get_timezone_utc())
            } else {
                let tz = TimeZone::new(tz.offset_seconds, tz.name)
                    .map_err(|_| InvalidInputError::invalid_type("timezone"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))))
            }
        }
        MontyNode::Exception { exc_type, arg } => {
            let exc = SimpleException::new(exc_type, arg);
            Ok(Value::Ref(vm.heap.allocate(HeapData::Exception(exc))))
        }
        // A sandbox class the host hands back resolves to the class object
        // itself; a host class resolves to its single `HostClassType` entry,
        // and calling it (or a classmethod on it) suspends to the host, whose
        // own policy decides.
        MontyNode::ClassType(class) => match vm.heap.resolve_boundary_uuid(&class.id) {
            Some(class_id) if matches!(vm.heap.get(class_id), HeapData::Class(_)) => {
                vm.heap.inc_ref(class_id);
                Ok(Value::Ref(class_id))
            }
            _ if class.host_defined => {
                let attrs = import_pairs(&class.attrs, built, vm);
                intern_host_class_type(class.name, class.id, class.is_dataclass, attrs, vm).map(Value::Ref)
            }
            _ => Err(InvalidInputError::invalid_type(format!(
                "sandbox class '{}' (id {}) no longer exists",
                class.name, class.id
            ))),
        },
        // A sandbox instance the host hands back resolves to the original
        // object by uuid (identity survives the round trip); anything else
        // is host-backed, whatever its class id resolved to as a type object.
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => match vm.heap.resolve_boundary_uuid(&instance_id) {
            Some(id) if matches!(vm.heap.get(id), HeapData::Instance(_)) => {
                // TODO: apply `attrs` to the instance so host-side edits
                // are visible; for now the payload is ignored.
                vm.heap.inc_ref(id);
                Ok(Value::Ref(id))
            }
            _ if host_classes.contains_key(&class_type) => {
                let class = &host_classes[&class_type];
                let pairs = import_pairs(&attrs, built, vm);
                let dict = Dict::from_pairs(pairs, vm)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable class instance attr keys"))?;
                // Guarded while the class type interns (its attrs can fail
                // too); `HostClass::new` then takes both.
                let mut dict_guard = DropGuard::new(dict, vm);
                let (_, vm) = dict_guard.as_parts_mut();
                let class_attrs = import_pairs(&class.attrs, built, vm);
                let class_id =
                    intern_host_class_type(class.name.clone(), class.id, class.is_dataclass, class_attrs, vm)?;
                let (dict, vm) = dict_guard.into_parts();
                let hc = HostClass::new(instance_id, class_id, dict);
                Ok(Value::Ref(vm.heap.allocate(HeapData::HostClass(Box::new(hc)))))
            }
            _ => {
                let Value::Ref(class_id) = built[class_type.index()] else {
                    unreachable!("class nodes always import as heap references");
                };
                Err(InvalidInputError::invalid_type(format!(
                    "sandbox instance of '{}' (id {instance_id}) no longer exists",
                    class_name(class_id, vm.heap, vm.interns)
                )))
            }
        },
        MontyNode::Path(s) => Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(s))))),
        MontyNode::FileHandle(handle) => {
            let file = OpenFile::with_state(handle.path, handle.mode, handle.position);
            Ok(Value::Ref(vm.heap.allocate(HeapData::OpenFile(Box::new(file)))))
        }
        MontyNode::Type(t) => match t.to_internal() {
            Some(ty) => Ok(Value::Builtin(Builtins::Type(ty))),
            None => Err(InvalidInputError::invalid_type(format!(
                "type object '{t}' is not a valid input value"
            ))),
        },
        MontyNode::BuiltinFunction(f) => Ok(Value::Builtin(Builtins::Function(f))),
        MontyNode::Function { name, .. } => Ok(vm.heap.get_ext_function(&name)),
        MontyNode::Repr(_) => Err(InvalidInputError::invalid_type("'Repr' is not a valid input value")),
        MontyNode::Cycle(_) => Err(InvalidInputError::invalid_type("'Cycle' is not a valid input value")),
    }
}

/// Owned clones of already-imported children.
fn import_children(ids: &[NodeId], built: &[Value], vm: &VM<'_>) -> Vec<Value> {
    ids.iter()
        .map(|id| built[id.index()].clone_with_heap(vm.heap))
        .collect()
}

/// Owned clones of already-imported `(key, value)` children.
fn import_pairs(pairs: &[(NodeId, NodeId)], built: &[Value], vm: &VM<'_>) -> Vec<(Value, Value)> {
    pairs
        .iter()
        .map(|(key, value)| {
            (
                built[key.index()].clone_with_heap(vm.heap),
                built[value.index()].clone_with_heap(vm.heap),
            )
        })
        .collect()
}

/// Builds a `Set` from already-imported elements, dropping the partially-built
/// set (and every value already added to it) if any element fails to hash.
fn import_set(
    ids: &[NodeId],
    built: &[Value],
    vm: &mut VM<'_>,
    unhashable_msg: &'static str,
) -> Result<Set, InvalidInputError> {
    let mut guard = DropGuard::new(Set::new(), vm);
    let (set, vm) = guard.as_parts_mut();
    for id in ids {
        let value = built[id.index()].clone_with_heap(vm.heap);
        set.add(value, vm)
            .map_err(|_| InvalidInputError::invalid_type(unhashable_msg))?;
    }
    Ok(guard.into_inner())
}

/// Resolves a host-defined class to the sandbox's single `HostClassType`
/// entry for its uuid, creating it on first sight, and returns an owned
/// reference. A re-send overwrites `name` and `is_dataclass` (the host is
/// authoritative) and, when its eager class attrs are non-empty, replaces
/// them; an empty set leaves them alone.
fn intern_host_class_type(
    name: String,
    type_id: MontyUuid,
    is_dataclass: bool,
    attr_pairs: Vec<(Value, Value)>,
    vm: &mut VM<'_>,
) -> Result<HeapId, InvalidInputError> {
    let attrs =
        Dict::from_pairs(attr_pairs, vm).map_err(|_| InvalidInputError::invalid_type("unhashable class attr keys"))?;
    let name = EitherStr::Heap(name);
    match vm.heap.resolve_host_type(&type_id) {
        Some(class_id) => {
            vm.heap.inc_ref(class_id);
            let HeapReadOutput::HostClassType(mut ty) = vm.heap.read(class_id) else {
                unreachable!("host_type_index points at a non-host-class-type entry");
            };
            let replaced = (!attrs.is_empty()).then_some(attrs);
            let old_attrs = ty.update_from_wire(name, is_dataclass, replaced, vm.heap);
            old_attrs.drop_with(vm);
            Ok(class_id)
        }
        None => Ok(vm
            .heap
            .allocate_host_type(HostClassType::new(name, type_id, is_dataclass, attrs))),
    }
}

/// Clones every `(key, value)` pair out of a dict (or dataclass attrs) so
/// recursive conversion cannot be invalidated by user code mutating it.
fn snapshot_dict_pairs(dict: &Dict, heap: &Heap) -> Vec<(Value, Value)> {
    (0..dict.len())
        .map(|i| {
            (
                dict.key_at(i).expect("index in range").clone_with_heap(heap),
                dict.value_at(i).expect("index in range").clone_with_heap(heap),
            )
        })
        .collect()
}

/// Converts a value to its repr node, falling back to a descriptive error
/// message if `py_repr` fails (e.g. INT_MAX_STR_DIGITS).
fn repr_or_error(value: &Value, vm: &mut VM<'_>) -> MontyNode {
    match value.py_repr(vm) {
        Ok(s) => {
            // `py_repr` yields a heap `str` `Value`; extract its text and drop it.
            defer_drop!(s, vm);
            MontyNode::Repr(s.to_str(vm).map(str::to_owned).unwrap_or_default())
        }
        Err(e) => {
            let ty = value.py_type_name(vm);
            let msg = match &e {
                RunError::Internal(s) => s.to_string(),
                RunError::Exc(exc) | RunError::UncatchableExc(exc) => exc.exc.to_string(),
            };
            MontyNode::Repr(format!("<{ty} object, error on repr(): {msg}>"))
        }
    }
}
