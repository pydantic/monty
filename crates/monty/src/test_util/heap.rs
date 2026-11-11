//! A model harness for the heap arena's reference counts and reachability.
//!
//! [`run_ops`] drives a fresh [`Heap`] through an [`Op`] sequence while a shadow
//! model tracks every allocation, owned child edge, slot reference and extra
//! reference the harness holds. After every operation it asserts that the heap's
//! live set and per-entry refcounts match the model, and after [`Op::Collect`]
//! that the live set is exactly the root closure. It finds bookkeeping bugs
//! (a missing or extra child edge, a leaked reference, a collector that keeps an
//! unreachable cycle) that are not memory errors and so are invisible to Miri.
//!
//! Gated behind `ref-count-return` because the oracle needs
//! [`Heap::get_refcount`] and the live-entry iterator.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt, mem,
};

use monty_types::ResourceTracker;

use crate::{
    asyncio::{CallId, ExternalFuture, GatherFuture},
    heap::{Heap, HeapData, HeapId, HeapReader},
    heap_data::{CellValue, Closure},
    intern::FunctionId,
    types::{Dict, List, Set, tuple::allocate_tuple},
    value::Value,
};

/// Slot count for a model run: small enough to run under Miri, large enough for
/// multi-edge and duplicate-child cycles.
pub const MAX_SLOTS: usize = 8;

/// One heap storage class the model can allocate.
///
/// `children` in [`Op::Alloc`] are slot indices; each becomes one owned edge, so
/// a repeated index creates a duplicate edge and an index equal to the allocated
/// slot creates a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Any number of children.
    List,
    /// Requires at least one child: an empty tuple is the heap singleton.
    Tuple,
    /// Zero or one child; zero stores `Value::None`.
    Cell,
    /// Any number of children, stored as captured cells.
    Closure,
    /// Any number of children, stored as gather items.
    GatherFuture,
    /// Zero or one child, stored as `sleep_result`.
    ExternalFuture,
    /// Childless: inserting into a `Dict` needs a `VM` to hash keys.
    Dict,
    /// Childless: inserting into a `Set` needs a `VM` to hash values.
    Set,
}

/// One operation in a heap-model sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Allocates `kind` into `slot`, adopting one reference to each child slot.
    Alloc { slot: u8, kind: Kind, children: Vec<u8> },
    /// Appends one more owned reference to `child` to the `List` in `slot`.
    Append { slot: u8, child: u8 },
    /// Removes and releases the last element of the `List` in `slot`.
    Pop(u8),
    /// Replaces the value stored in the `Cell` in `slot` with a reference to
    /// `child`, releasing the previous value.
    Set { slot: u8, child: u8 },
    /// Releases the slot's owned reference and empties the slot. Invalid while
    /// the slot holds extra references from [`Op::Inc`].
    Drop(u8),
    /// Takes one extra reference the model holds outside any slot.
    Inc(u8),
    /// Releases one extra reference previously taken by [`Op::Inc`].
    Dec(u8),
    /// Runs [`Heap::collect_cycles`].
    Collect,
    /// Round-trips the heap through the session dump codec.
    DumpRestore,
    /// Holds a `HeapRead` on the slot while allocating and freeing an empty list.
    AllocDuringRead(u8),
}

/// Why [`run_ops`] stopped.
#[derive(Debug)]
pub enum ModelError {
    /// The sequence is not well formed: an out-of-range or empty slot, an
    /// occupied allocation slot, a missing extra reference, or a child list the
    /// kind does not accept. Callers may ignore this for random input.
    Invalid(&'static str),
    /// The heap disagreed with the model; this is a heap bug.
    Mismatch(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid op: {reason}"),
            Self::Mismatch(reason) => write!(f, "model mismatch: {reason}"),
        }
    }
}

impl Error for ModelError {}

/// Drives `ops` against a fresh heap and checks the shadow model after every
/// operation.
///
/// `Err(ModelError::Invalid)` means the sequence is malformed; any other error
/// is a heap bookkeeping bug.
pub fn run_ops(ops: &[Op]) -> Result<(), ModelError> {
    drive(ops, false)
}

/// Like [`run_ops`], but skips operations that cannot apply to the current
/// state instead of stopping the sequence.
///
/// Fuzz targets use this so a random stream still reaches deep into the
/// operation surface: only a heap/model divergence ends the run.
pub fn run_ops_ignoring_invalid(ops: &[Op]) -> Result<(), ModelError> {
    drive(ops, true)
}

/// Shared driver; `ignore_invalid` selects the two public entry points above.
fn drive(ops: &[Op], ignore_invalid: bool) -> Result<(), ModelError> {
    let mut model = Model::new();
    for op in ops {
        match model.apply(op) {
            Ok(()) => model.check(matches!(op, Op::Collect))?,
            Err(ModelError::Invalid(_)) if ignore_invalid => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// The heap plus the shadow state the oracle compares against.
struct Model {
    heap: Heap,
    /// One owned reference per occupied slot.
    slots: [Option<HeapId>; MAX_SLOTS],
    /// Extra references held outside any slot, by id.
    extras: HashMap<HeapId, usize>,
    /// Owned child edges per allocated id; duplicates are independent edges.
    edges: HashMap<HeapId, Vec<HeapId>>,
    /// Every id the model believes the heap still holds; pruned to the heap's
    /// live set at the end of each successful check.
    known: HashSet<HeapId>,
    /// Monotonic source for `ExternalFuture` call ids.
    call_ids: u32,
}

impl Model {
    fn new() -> Self {
        Self {
            heap: Heap::new(16, ResourceTracker::default()),
            slots: [None; MAX_SLOTS],
            extras: HashMap::new(),
            edges: HashMap::new(),
            known: HashSet::new(),
            call_ids: 0,
        }
    }

    fn apply(&mut self, op: &Op) -> Result<(), ModelError> {
        match op {
            Op::Alloc { slot, kind, children } => self.alloc(*slot, *kind, children),
            Op::Append { slot, child } => {
                let id = self.list_id(*slot)?;
                let child_id = self.slot_id(*child)?;
                self.heap.inc_ref(child_id);

                HeapReader::with(&mut self.heap, &mut (), |reader, ()| {
                    let list = reader.read_as::<List>(id).expect("list_id checked the variant");
                    let list = list.get_mut(reader);
                    list.set_contains_refs();
                    list.as_vec_mut().push(Value::Ref(child_id));
                });

                self.edges.get_mut(&id).expect("allocated id").push(child_id);

                Ok(())
            }
            Op::Pop(slot) => {
                let id = self.list_id(*slot)?;
                let popped = HeapReader::with(&mut self.heap, &mut (), |reader, ()| {
                    let list = reader.read_as::<List>(id).expect("list_id checked the variant");
                    list.get_mut(reader).as_vec_mut().pop()
                });

                let Some(popped) = popped else {
                    return Err(ModelError::Invalid("Pop on an empty List"));
                };

                let child = match &popped {
                    Value::Ref(child) => Some(*child),
                    _ => None,
                };

                popped.drop_with(&mut self.heap);
                let expected = self.edges.get_mut(&id).expect("allocated id").pop();

                if expected != child {
                    return Err(ModelError::Mismatch(format!(
                        "Pop removed {child:?}, model expected {expected:?}"
                    )));
                }

                Ok(())
            }
            Op::Set { slot, child } => {
                let id = self.cell_id(*slot)?;
                let child_id = self.slot_id(*child)?;
                self.heap.inc_ref(child_id);

                HeapReader::with(&mut self.heap, &mut (), |reader, ()| {
                    let cell = reader.read_as::<CellValue>(id).expect("cell_id checked the variant");
                    let cell = cell.get_mut(reader);
                    let old = mem::replace(&mut cell.0, Value::Ref(child_id));

                    old.drop_with(reader);
                });

                let edges = self.edges.get_mut(&id).expect("allocated id");
                edges.clear();
                edges.push(child_id);

                Ok(())
            }
            Op::Drop(slot) => {
                let index = Self::slot_index(*slot)?;

                let Some(id) = self.slots[index] else {
                    return Err(ModelError::Invalid("slot is empty"));
                };

                if self.extras.contains_key(&id) {
                    return Err(ModelError::Invalid("slot still holds extra references"));
                }

                self.slots[index] = None;
                self.heap.dec_ref(id);

                Ok(())
            }
            Op::Inc(slot) => {
                let id = self.slot_id(*slot)?;
                self.heap.inc_ref(id);
                *self.extras.entry(id).or_default() += 1;

                Ok(())
            }
            Op::Dec(slot) => {
                let id = self.slot_id(*slot)?;

                let count = self
                    .extras
                    .get_mut(&id)
                    .ok_or(ModelError::Invalid("slot holds no extra reference"))?;

                *count -= 1;
                if *count == 0 {
                    self.extras.remove(&id);
                }

                self.heap.dec_ref(id);

                Ok(())
            }
            Op::Collect => {
                self.heap.collect_cycles();
                Ok(())
            }
            Op::DumpRestore => {
                let bytes = minicbor_serde::to_vec(&self.heap)
                    .map_err(|err| ModelError::Mismatch(format!("heap serialize failed: {err:?}")))?;

                self.heap = minicbor_serde::from_slice(&bytes)
                    .map_err(|err| ModelError::Mismatch(format!("heap restore failed: {err:?}")))?;

                Ok(())
            }
            Op::AllocDuringRead(slot) => {
                let id = self.slot_id(*slot)?;

                HeapReader::with(&mut self.heap, &mut (), |reader, ()| {
                    let _held = reader.read(id);
                    let temp = reader.allocate(HeapData::List(List::new(Vec::new())));
                    Value::Ref(temp).drop_with(reader);
                });

                Ok(())
            }
        }
    }

    /// Allocates `kind` into `slot`, taking one owned reference per child.
    fn alloc(&mut self, slot: u8, kind: Kind, children: &[u8]) -> Result<(), ModelError> {
        let index = Self::slot_index(slot)?;

        if self.slots[index].is_some() {
            return Err(ModelError::Invalid("allocation slot is occupied"));
        }

        if children.len() > MAX_SLOTS {
            return Err(ModelError::Invalid("too many children"));
        }

        let mut child_ids = Vec::with_capacity(children.len());

        for child in children {
            child_ids.push(self.slot_id(*child)?);
        }

        match kind {
            Kind::Tuple if child_ids.is_empty() => {
                return Err(ModelError::Invalid("Tuple requires at least one child"));
            }
            Kind::Dict | Kind::Set if !child_ids.is_empty() => {
                return Err(ModelError::Invalid("Dict and Set cannot take children"));
            }
            Kind::Cell | Kind::ExternalFuture if child_ids.len() > 1 => {
                return Err(ModelError::Invalid("Cell and ExternalFuture take at most one child"));
            }
            _ => {}
        }

        for child in &child_ids {
            self.heap.inc_ref(*child);
        }

        let values = || child_ids.iter().map(|id| Value::Ref(*id)).collect::<Vec<_>>();

        let id = match kind {
            Kind::List => self.heap.allocate(HeapData::List(List::new(values()))),
            Kind::Tuple => allocate_tuple(values().into(), &self.heap)
                .into_ref_id()
                .expect("a non-empty tuple is a heap reference"),
            Kind::Cell => self
                .heap
                .allocate(HeapData::Cell(CellValue(values().pop().unwrap_or(Value::None)))),
            Kind::Closure => self.heap.allocate(HeapData::Closure(Closure {
                func_id: FunctionId::from_index(0),
                cells: child_ids.clone().into_boxed_slice(),
                defaults: Box::default(),
                globals: None,
            })),
            Kind::GatherFuture => self
                .heap
                .allocate(HeapData::GatherFuture(Box::new(GatherFuture::new(child_ids.clone())))),
            Kind::ExternalFuture => {
                self.call_ids += 1;
                let sleep_result = values().pop();
                self.heap
                    .allocate(HeapData::ExternalFuture(Box::new(ExternalFuture::new_pending(
                        CallId::new(self.call_ids),
                        sleep_result,
                    ))))
            }
            Kind::Dict => self.heap.allocate(HeapData::Dict(Dict::new())),
            Kind::Set => self.heap.allocate(HeapData::Set(Set::new())),
        };

        // Keeps `classify` live in debug builds and catches a `Kind` that stops
        // describing what its arm actually allocates.
        debug_assert_eq!(
            HeapReader::with(&mut self.heap, &mut (), |reader, ()| {
                classify(reader.read_ptr(id).data(reader))
            }),
            Some(kind),
            "model allocated {kind:?} but the heap holds a different class"
        );

        self.slots[index] = Some(id);
        self.known.insert(id);
        self.edges.insert(id, child_ids);

        Ok(())
    }

    /// Checks the heap against the model.
    ///
    /// Between collections the expected live set is the reference-count fixpoint
    /// over every modelled id (root-less cycles stay alive). After a collection it
    /// is the root closure: the collector must have reclaimed everything else.
    /// On success the model is pruned to the heap's live set so later operations
    /// do not count edges of objects that are gone.
    fn check(&mut self, after_collect: bool) -> Result<(), ModelError> {
        let mut roots: HashSet<HeapId> = HashSet::new();
        let mut root_refs: HashMap<HeapId, usize> = HashMap::new();

        for id in self.slots.iter().flatten() {
            *root_refs.entry(*id).or_default() += 1;
            roots.insert(*id);
        }

        for (id, count) in &self.extras {
            *root_refs.entry(*id).or_default() += *count;
            roots.insert(*id);
        }

        let expected_live = if after_collect {
            let mut reachable = roots.clone();
            let mut stack: Vec<HeapId> = roots.into_iter().collect();

            while let Some(id) = stack.pop() {
                for child in self.edges.get(&id).into_iter().flatten() {
                    if reachable.insert(*child) {
                        stack.push(*child);
                    }
                }
            }

            reachable
        } else {
            let mut support: HashMap<HeapId, usize> = self.known.iter().map(|id| (*id, 0)).collect();
            for (id, count) in &root_refs {
                *support.get_mut(id).expect("root ids are known") += *count;
            }

            for (parent, children) in &self.edges {
                debug_assert!(support.contains_key(parent), "edge owner is always a known id");

                for child in children {
                    *support.get_mut(child).expect("edge targets are known") += 1;
                }
            }

            // Peel ids with no support: exactly what reference counting frees
            // eagerly. Survivors keep a root reference or sit in a cycle.
            let mut queue: Vec<HeapId> = support
                .iter()
                .filter(|(_, count)| **count == 0)
                .map(|(id, _)| *id)
                .collect();

            while let Some(id) = queue.pop() {
                support.remove(&id);
                if let Some(children) = self.edges.get(&id) {
                    for child in children {
                        let Some(count) = support.get_mut(child) else { continue };

                        *count -= 1;
                        if *count == 0 {
                            queue.push(*child);
                        }
                    }
                }
            }
            support.keys().copied().collect()
        };

        let heap_live: HashSet<HeapId> = self.heap.live_ids().collect();
        if heap_live != expected_live {
            let mut heap_only: Vec<usize> = heap_live.difference(&expected_live).map(|id| id.index()).collect();
            heap_only.sort_unstable();

            let mut model_only: Vec<usize> = expected_live.difference(&heap_live).map(|id| id.index()).collect();
            model_only.sort_unstable();

            let what = if after_collect {
                "collector retained unreachable entries"
            } else {
                "live set differs"
            };

            return Err(ModelError::Mismatch(format!(
                "{what}: heap-only {heap_only:?}, model-only {model_only:?}"
            )));
        }

        // Expected refcount: root references plus in-edges from live parents only,
        // so a cycle the collector just freed no longer contributes.
        let mut expected_rc: HashMap<HeapId, usize> = HashMap::new();
        for (id, count) in &root_refs {
            if expected_live.contains(id) {
                *expected_rc.entry(*id).or_default() += *count;
            }
        }

        for (parent, children) in &self.edges {
            if !expected_live.contains(parent) {
                continue;
            }

            for child in children {
                if expected_live.contains(child) {
                    *expected_rc.entry(*child).or_default() += 1;
                }
            }
        }

        for id in &heap_live {
            let actual = self.heap.get_refcount(*id);
            let expected = expected_rc.get(id).copied().unwrap_or(0);

            if actual != expected {
                return Err(ModelError::Mismatch(format!(
                    "HeapId({}) refcount is {actual}, model expects {expected}",
                    id.index()
                )));
            }
        }

        self.known.clone_from(&heap_live);
        self.edges.retain(|id, _| heap_live.contains(id));
        self.extras.retain(|id, _| heap_live.contains(id));

        Ok(())
    }

    /// Resolves a slot index for a read: in range and occupied.
    fn slot_id(&self, slot: u8) -> Result<HeapId, ModelError> {
        let index = Self::slot_index(slot)?;
        self.slots[index].ok_or(ModelError::Invalid("slot is empty"))
    }

    /// Resolves a slot that holds a `List`.
    fn list_id(&self, slot: u8) -> Result<HeapId, ModelError> {
        let id = self.slot_id(slot)?;
        if matches!(self.heap.get(id), HeapData::List(_)) {
            Ok(id)
        } else {
            Err(ModelError::Invalid("slot does not hold a List"))
        }
    }

    /// Resolves a slot that holds a `Cell`.
    fn cell_id(&self, slot: u8) -> Result<HeapId, ModelError> {
        let id = self.slot_id(slot)?;
        if matches!(self.heap.get(id), HeapData::Cell(_)) {
            Ok(id)
        } else {
            Err(ModelError::Invalid("slot does not hold a Cell"))
        }
    }

    /// Resolves an in-range slot index.
    fn slot_index(slot: u8) -> Result<usize, ModelError> {
        let index = usize::from(slot);
        if index < MAX_SLOTS {
            Ok(index)
        } else {
            Err(ModelError::Invalid("slot index out of range"))
        }
    }
}

/// Maps a live payload to the model class that can construct it, or `None` for
/// payloads the model deliberately does not build.
///
/// Matched exhaustively (no `_` arm), so a new `HeapData` variant fails to
/// compile until it is classified here. `None` must be a decision, not an
/// omission: a payload that owns children and needs no `VM` to construct
/// belongs in [`Kind`] so the oracle exercises it.
fn classify(data: &HeapData) -> Option<Kind> {
    match data {
        HeapData::List(_) => Some(Kind::List),
        HeapData::Tuple(_) => Some(Kind::Tuple),
        HeapData::Cell(_) => Some(Kind::Cell),
        HeapData::Closure(_) => Some(Kind::Closure),
        HeapData::GatherFuture(_) => Some(Kind::GatherFuture),
        HeapData::ExternalFuture(_) => Some(Kind::ExternalFuture),
        // Modelled empty only: inserting into either needs a `VM` to hash keys.
        HeapData::Dict(_) => Some(Kind::Dict),
        HeapData::Set(_) => Some(Kind::Set),
        // Owned child edges, but constructing these needs a `VM`, `Interns`, or
        // interpreter-side helpers the model does not have.
        HeapData::Itertools(_)
        | HeapData::DirScan(_)
        | HeapData::Deque(_)
        | HeapData::NamedTuple(_)
        | HeapData::NamedTupleClass(_)
        | HeapData::DictKeysView(_)
        | HeapData::DictItemsView(_)
        | HeapData::DictValuesView(_)
        | HeapData::FrozenSet(_)
        | HeapData::FunctionDefaults(_)
        | HeapData::HostClass(_)
        | HeapData::HostClassType(_)
        | HeapData::Class(_)
        | HeapData::Instance(_)
        | HeapData::BoundMethod(_)
        | HeapData::DataclassField(_)
        | HeapData::DataclassMethod(_)
        | HeapData::ListIterator(_)
        | HeapData::DequeIterator(_)
        | HeapData::TupleIterator(_)
        | HeapData::DictKeyIterator(_)
        | HeapData::DictItemIterator(_)
        | HeapData::DictValueIterator(_)
        | HeapData::SetIterator(_)
        | HeapData::CallableIterator(_)
        | HeapData::Module(_)
        | HeapData::Coroutine(_)
        | HeapData::Partial(_)
        | HeapData::GenericAlias(_)
        | HeapData::Union(_)
        | HeapData::ForwardRef(_)
        | HeapData::LruCache(_)
        | HeapData::Csv(_)
        | HeapData::Exception(_)
        | HeapData::StringIterator(_)
        | HeapData::BytesIterator(_)
        | HeapData::RangeIterator(_)
        | HeapData::OpenFile(_)
        | HeapData::ReMatch(_)
        | HeapData::DecimalSignalDict(_)
        | HeapData::DecimalContextManager(_)
        | HeapData::DateTime(_)
        | HeapData::Time(_) => None,
        // No owned heap references: `for_each_child_id` classifies these leaves.
        HeapData::Str(_)
        | HeapData::Bytes(_)
        | HeapData::Range(_)
        | HeapData::Slice(_)
        | HeapData::LongInt(_)
        | HeapData::Complex(_)
        | HeapData::Path(_)
        | HeapData::RePattern(_)
        | HeapData::ExtFunction(_)
        | HeapData::Date(_)
        | HeapData::TimeDelta(_)
        | HeapData::TimeZone(_)
        | HeapData::DataclassParams(_)
        | HeapData::Random(_)
        | HeapData::HashObject(_)
        | HeapData::Decimal(_)
        | HeapData::DecimalContext(_) => None,
    }
}
