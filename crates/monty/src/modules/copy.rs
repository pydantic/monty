//! Implementation of Python's `copy` module.
//!
//! CPython reaches most types through the pickle protocol (`__reduce_ex__`).
//! Monty has no pickle, so both functions dispatch on the heap type directly:
//! immutable values are shared, containers are rebuilt, and anything else
//! raises the `TypeError` CPython's pickler would. `__copy__`/`__deepcopy__`
//! are still honoured. Divergences are listed in `limitations/copy.md`.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{Dict, List, Module, instance_call_copy_hook},
    value::{VALUE_SIZE, Value},
};

/// `copy` module functions, one variant per Python-visible function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
pub(crate) enum CopyFunctions {
    #[strum(serialize = "copy")]
    Copy,
    #[strum(serialize = "deepcopy")]
    Deepcopy,
}

/// Static mapping of attribute names to functions for module creation.
const COPY_FUNCTIONS: &[(StaticStrings, CopyFunctions)] = &[
    (StaticStrings::Copy, CopyFunctions::Copy),
    (StaticStrings::Deepcopy, CopyFunctions::Deepcopy),
];

/// Creates the `copy` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Copy);

    for (name, func) in COPY_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Copy(*func)), vm);
    }

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `copy` module function.
pub(super) fn call(vm: &mut VM<'_>, function: CopyFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        CopyFunctions::Copy => call_copy(vm, args),
        CopyFunctions::Deepcopy => call_deepcopy(vm, args),
    }
}

/// A pure-Python `def` in CPython, so the parameter binds without a type check.
#[derive(FromArgs)]
#[from_args(name = "copy", style = def)]
struct CopyArgs {
    x: Value,
}

/// `copy.copy(x)`: a new object holding the *same* items as `x`.
fn call_copy(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CopyArgs { x } = CopyArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    shallow_copy(x, vm)
}

/// CPython's signature. `_nil` is its private "missing from `memo`" sentinel,
/// accepted for arity parity and never read.
#[derive(FromArgs)]
#[from_args(name = "deepcopy", style = def)]
struct DeepcopyArgs {
    x: Value,
    #[from_args(default = Value::None, static_string = "Memo")]
    memo: Value,
    #[from_args(default = Value::None, static_string = "NilSentinel")]
    _nil: Value,
}

/// `copy.deepcopy(x, memo=None)`: rebuilds `x` and everything it holds.
fn call_deepcopy(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DeepcopyArgs { x, memo, _nil: nil } = DeepcopyArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    defer_drop!(nil, vm);
    let memo = Memo::new(memo, vm)?;
    let mut guard = DropGuard::new(memo, vm);
    let (memo, vm) = guard.as_parts_mut();
    deep_copy(x, memo, vm)
}

// ===========================================================================
// Shallow copy
// ===========================================================================

/// Returns a new container holding the very objects the original holds.
///
/// The match is exhaustive over `HeapReadOutput` on purpose: a new heap type
/// must decide whether it is shared, rebuilt or refused, rather than falling
/// into "cannot pickle" because nobody looked. Shared values are CPython's
/// `_copy_atomic_types` set.
fn shallow_copy(value: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let Value::Ref(id) = value else {
        return Ok(value.clone_with_heap(vm.heap));
    };
    let id = *id;
    match vm.heap.read(id) {
        HeapReadOutput::List(list) => {
            let items = clone_items(list.get(vm.heap).len(), vm, |index, vm| list.clone_item(index, vm))?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
        }
        HeapReadOutput::Dict(dict) => {
            let pairs = dict.clone_all_pairs(vm)?;
            let copy_id = dict.allocate_empty_like(vm);
            let filled = insert_pairs(copy_id, pairs, vm);
            release_on_error(Value::Ref(copy_id), filled, vm)
        }
        HeapReadOutput::Set(set) => {
            let copy = set.copy(vm);
            Ok(Value::Ref(vm.heap.allocate(HeapData::Set(copy))))
        }
        HeapReadOutput::Deque(deque) => {
            let items = clone_items(deque.get(vm.heap).len(), vm, |index, vm| {
                deque
                    .get(vm.heap)
                    .get(index)
                    .expect("index is in bounds")
                    .clone_with_heap(vm.heap)
            })?;
            Ok(deque.allocate_like(items, vm))
        }
        HeapReadOutput::NamedTuple(named) => {
            let items = clone_items(named.get(vm.heap).field_names().len(), vm, |index, vm| {
                named.clone_item(index, vm)
            })?;
            Ok(named.allocate_like(items, vm))
        }
        HeapReadOutput::Instance(instance) => {
            if let Some(copy) = instance_call_copy_hook(id, "__copy__", None, vm)? {
                Ok(copy)
            } else {
                let pairs = clone_attrs(id, vm)?;
                let copy_id = instance.allocate_empty_like(vm);
                let filled = insert_attrs(copy_id, pairs, vm);
                release_on_error(Value::Ref(copy_id), filled, vm)
            }
        }
        // A new binding over the same receiver and function, as CPython's
        // `getattr(obj, name)` reducer produces.
        HeapReadOutput::BoundMethod(bound) => {
            let instance = bound.get(vm.heap).instance.clone_with_heap(vm.heap);
            Ok(bound.allocate_like(instance, vm))
        }
        // A new partial over the same callable and bound values, as CPython's
        // reducer produces. Unlike the immutable containers below, this is a
        // distinct object in CPython too, so sharing it would be visible.
        HeapReadOutput::Partial(partial) => partial.allocate_like(vm),
        // Leaves and immutable containers Monty can never mutate, so a copy
        // that shared them is indistinguishable from one that rebuilt them.
        HeapReadOutput::Str(_)
        | HeapReadOutput::Bytes(_)
        | HeapReadOutput::LongInt(_)
        | HeapReadOutput::Range(_)
        | HeapReadOutput::Slice(_)
        | HeapReadOutput::RePattern(_)
        | HeapReadOutput::ReMatch(_)
        | HeapReadOutput::Exception(_)
        | HeapReadOutput::Date(_)
        | HeapReadOutput::Time(_)
        | HeapReadOutput::DateTime(_)
        | HeapReadOutput::TimeDelta(_)
        | HeapReadOutput::TimeZone(_)
        | HeapReadOutput::Path(_)
        // Classes and functions. A `def`/`lambda` only reaches the heap once
        // it captures an enclosing scope or evaluates a default; a plain one
        // is a `Value::Function` and never gets here. Their state is mutable
        // (`nonlocal`, class attributes) but CPython shares them anyway, so
        // rebuilding would be the divergence.
        | HeapReadOutput::Class(_)
        | HeapReadOutput::NamedTupleClass(_)
        | HeapReadOutput::HostClassType(_)
        | HeapReadOutput::Closure(_)
        | HeapReadOutput::FunctionDefaults(_)
        // A shallow copy of an immutable container holds the same items, so
        // CPython hands back the original; only `deepcopy` rebuilds these.
        | HeapReadOutput::Tuple(_)
        | HeapReadOutput::FrozenSet(_) => Ok(value.clone_with_heap(vm.heap)),
        // Refused, with the `TypeError` CPython's pickler raises. Views and
        // iterators are positions into something else; the rest are host
        // objects or interpreter internals with no Python-visible
        // constructor to rebuild them from. A `HostClass` is the clearest of
        // those: its identity belongs to the host, so a rebuilt one would
        // answer lazy attribute reads through the very object it was supposed
        // to be detached from. See `limitations/copy.md` for the cases where
        // CPython manages to copy one of these and Monty does not.
        HeapReadOutput::HostClass(_)
        | HeapReadOutput::DictKeysView(_)
        | HeapReadOutput::DictItemsView(_)
        | HeapReadOutput::DictValuesView(_)
        | HeapReadOutput::ListIterator(_)
        | HeapReadOutput::DequeIterator(_)
        | HeapReadOutput::TupleIterator(_)
        | HeapReadOutput::StringIterator(_)
        | HeapReadOutput::BytesIterator(_)
        | HeapReadOutput::RangeIterator(_)
        | HeapReadOutput::DictKeyIterator(_)
        | HeapReadOutput::DictItemIterator(_)
        | HeapReadOutput::DictValueIterator(_)
        | HeapReadOutput::SetIterator(_)
        | HeapReadOutput::CallableIterator(_)
        | HeapReadOutput::Itertools(_)
        | HeapReadOutput::Module(_)
        | HeapReadOutput::Coroutine(_)
        | HeapReadOutput::GatherFuture(_)
        | HeapReadOutput::ExternalFuture(_)
        | HeapReadOutput::OpenFile(_)
        | HeapReadOutput::ExtFunction(_)
        | HeapReadOutput::Cell(_)
        | HeapReadOutput::DataclassField(_)
        | HeapReadOutput::DataclassParams(_) => Err(cannot_copy(value, vm)),
    }
}

// ===========================================================================
// Deep copy
// ===========================================================================

/// Deep-copying, implemented by the heap types `copy.deepcopy` rebuilds.
///
/// Narrow on purpose: only ten of the fifty-four heap types are rebuilt, so
/// this is a trait of its own rather than a method on
/// [`PyTrait`](crate::types::PyTrait) that the other forty-four would carry
/// meaninglessly. The method is required, with no
/// default — a default would be the "refuse it" answer, which is legitimate for
/// most types, so forgetting to override it would look exactly like deciding
/// not to. Which types reach this at all is settled by the exhaustive match in
/// [`deep_copy`].
///
/// Implementations memoize an empty shell before filling it, so a container
/// holding itself resolves to the shell instead of recursing forever, and hold
/// that shell in a `DropGuard` so a failure partway releases it.
pub(crate) trait PyDeepCopy<'h> {
    /// Returns a new object holding deep copies of everything this one holds.
    ///
    /// `source` is the `Value` being copied — the memo key, and what a type
    /// that turns out not to need rebuilding hands back unchanged.
    fn py_deep_copy(&self, source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value>;
}

/// Returns a deep copy of `value`, reusing anything this pass already copied.
///
/// The memo is what makes a graph that reaches the same object twice — itself
/// included — copy to a graph with the same sharing.
///
/// The whole walk runs inside one call, past no instruction checkpoint, so
/// every fill loop polls the duration limit as the container iterators do.
pub(crate) fn deep_copy(source: &Value, memo: &mut Memo, vm: &mut VM<'_>) -> RunResult<Value> {
    let Value::Ref(id) = source else {
        return Ok(source.clone_with_heap(vm.heap));
    };
    let id = *id;
    if let Some(hit) = memo.get(source, vm)? {
        return Ok(hit);
    }
    // One level per step, as every other Rust-side walk charges. A step costs
    // ~770 bytes of native stack for a list and ~1.1 KiB for a dict or an
    // instance, so a limit's worth of the latter exceeds a wasm worker's 1 MiB
    // — the same exposure `py_eq` and `py_repr` have, and the same fix (a
    // smaller `RunError`). See `tests/copy_module.rs`.
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    // Dispatched here rather than in a function of its own: every frame live
    // across the recursion is paid once per level of nesting.
    let copy = match vm.heap.read(id) {
        HeapReadOutput::List(list) => list.py_deep_copy(source, memo, vm),
        HeapReadOutput::Dict(dict) => dict.py_deep_copy(source, memo, vm),
        HeapReadOutput::Deque(deque) => deque.py_deep_copy(source, memo, vm),
        HeapReadOutput::Set(set) => set.py_deep_copy(source, memo, vm),
        HeapReadOutput::FrozenSet(frozen) => frozen.py_deep_copy(source, memo, vm),
        HeapReadOutput::Instance(instance) => instance.py_deep_copy(source, memo, vm),
        HeapReadOutput::Tuple(tuple) => tuple.py_deep_copy(source, memo, vm),
        HeapReadOutput::NamedTuple(named) => named.py_deep_copy(source, memo, vm),
        HeapReadOutput::BoundMethod(bound) => bound.py_deep_copy(source, memo, vm),
        HeapReadOutput::Partial(partial) => partial.py_deep_copy(source, memo, vm),
        // Leaves and immutable containers Monty can never mutate, so a copy
        // that shared them is indistinguishable from one that rebuilt them.
        HeapReadOutput::Str(_)
        | HeapReadOutput::Bytes(_)
        | HeapReadOutput::LongInt(_)
        | HeapReadOutput::Range(_)
        | HeapReadOutput::Slice(_)
        | HeapReadOutput::RePattern(_)
        | HeapReadOutput::ReMatch(_)
        | HeapReadOutput::Exception(_)
        | HeapReadOutput::Date(_)
        | HeapReadOutput::Time(_)
        | HeapReadOutput::DateTime(_)
        | HeapReadOutput::TimeDelta(_)
        | HeapReadOutput::TimeZone(_)
        | HeapReadOutput::Path(_)
        // Classes and functions. A `def`/`lambda` only reaches the heap once
        // it captures an enclosing scope or evaluates a default; a plain one
        // is a `Value::Function` and never gets here. Their state is mutable
        // (`nonlocal`, class attributes) but CPython shares them anyway, so
        // rebuilding would be the divergence.
        | HeapReadOutput::Class(_)
        | HeapReadOutput::NamedTupleClass(_)
        | HeapReadOutput::HostClassType(_)
        | HeapReadOutput::Closure(_)
        | HeapReadOutput::FunctionDefaults(_) => Ok(source.clone_with_heap(vm.heap)),
        // Refused, with the `TypeError` CPython's pickler raises. Views and
        // iterators are positions into something else; the rest are host
        // objects or interpreter internals with no Python-visible
        // constructor to rebuild them from. A `HostClass` is the clearest of
        // those: its identity belongs to the host, so a rebuilt one would
        // answer lazy attribute reads through the very object it was supposed
        // to be detached from. See `limitations/copy.md` for the cases where
        // CPython manages to copy one of these and Monty does not.
        HeapReadOutput::HostClass(_)
        | HeapReadOutput::DictKeysView(_)
        | HeapReadOutput::DictItemsView(_)
        | HeapReadOutput::DictValuesView(_)
        | HeapReadOutput::ListIterator(_)
        | HeapReadOutput::DequeIterator(_)
        | HeapReadOutput::TupleIterator(_)
        | HeapReadOutput::StringIterator(_)
        | HeapReadOutput::BytesIterator(_)
        | HeapReadOutput::RangeIterator(_)
        | HeapReadOutput::DictKeyIterator(_)
        | HeapReadOutput::DictItemIterator(_)
        | HeapReadOutput::DictValueIterator(_)
        | HeapReadOutput::SetIterator(_)
        | HeapReadOutput::CallableIterator(_)
        | HeapReadOutput::Itertools(_)
        | HeapReadOutput::Module(_)
        | HeapReadOutput::Coroutine(_)
        | HeapReadOutput::GatherFuture(_)
        | HeapReadOutput::ExternalFuture(_)
        | HeapReadOutput::OpenFile(_)
        | HeapReadOutput::ExtFunction(_)
        | HeapReadOutput::Cell(_)
        | HeapReadOutput::DataclassField(_)
        | HeapReadOutput::DataclassParams(_) => Err(cannot_copy(source, vm)),
    }?;
    // CPython only memoizes a copy that is a new object ("if y is not x").
    if same_object(source, &copy) {
        Ok(copy)
    } else {
        match memo.insert(source, &copy, vm) {
            Ok(()) => Ok(copy),
            Err(e) => {
                copy.drop_with(vm);
                Err(e)
            }
        }
    }
}

/// Fills an instance copy's `__dict__` from the original's, going
/// through the attribute dict rather than `setattr` so frozen ones can be
/// rebuilt — the bypass CPython's `__dict__.update` is.
pub(crate) fn deep_copy_attrs(
    source_id: HeapId,
    copy_id: HeapId,
    source: &Value,
    memo: &mut Memo,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let mut guard = DropGuard::new(Value::Ref(copy_id), vm);
    let (copy, vm) = guard.as_parts_mut();
    memo.insert(source, copy, vm)?;
    let expected_len = attrs(source_id, vm).len();
    // Preflighted whole, for the reason the dict copy is: the loop below
    // rejects a resize rather than following it, so this is the final width.
    vm.heap
        .tracker
        .check_allocation(expected_len.saturating_mul(2 * VALUE_SIZE))?;
    for index in 0.. {
        let (_, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        // `__dict__` is a dict, and CPython copies it by iterating it.
        if attrs(source_id, vm).len() != expected_len {
            return Err(ExcType::runtime_error_dict_changed_size());
        }
        let Some((key, value)) = clone_pair(attrs(source_id, vm), index, vm) else {
            break;
        };
        let (key_copy, value_copy) = deep_copy_pair(key, value, memo, vm)?;
        let HeapReadOutput::Instance(mut instance) = vm.heap.read(copy_id) else {
            unreachable!("caller allocated an instance")
        };
        let replaced = instance.attrs_mut().set(key_copy, value_copy, vm)?;
        if let Some(replaced) = replaced {
            replaced.drop_with(vm);
        }
    }
    let (copy, _) = guard.into_parts();
    Ok(copy)
}

/// Deep-copies `len` items into a vector, for the immutable containers that
/// cannot be built until they have all of them.
pub(crate) fn deep_copy_slots<'h>(
    len: usize,
    memo: &mut Memo,
    vm: &mut VM<'h>,
    mut clone_item: impl FnMut(usize, &mut VM<'h>) -> Value,
) -> RunResult<Vec<Value>> {
    vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
    let mut guard = DropGuard::new(Vec::with_capacity(len), vm);
    for index in 0..len {
        let (copied, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        let item = clone_item(index, vm);
        let result = deep_copy(&item, memo, vm);
        item.drop_with(vm);
        copied.push(result?);
    }
    let (copied, _) = guard.into_parts();
    Ok(copied)
}

/// Clones the `index`th key/value pair out of a dict, or `None` past its end.
pub(crate) fn clone_pair(dict: &Dict, index: usize, vm: &VM<'_>) -> Option<(Value, Value)> {
    let (key, value) = dict.item_at(index)?;
    Some((key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)))
}

/// Deep-copies an owned key and value, releasing both either way.
pub(crate) fn deep_copy_pair(key: Value, value: Value, memo: &mut Memo, vm: &mut VM<'_>) -> RunResult<(Value, Value)> {
    let key_copy = deep_copy(&key, memo, vm);
    key.drop_with(vm);
    let key_copy = match key_copy {
        Ok(key_copy) => key_copy,
        Err(e) => {
            value.drop_with(vm);
            return Err(e);
        }
    };
    let value_copy = deep_copy(&value, memo, vm);
    value.drop_with(vm);
    match value_copy {
        Ok(value_copy) => Ok((key_copy, value_copy)),
        Err(e) => {
            key_copy.drop_with(vm);
            Err(e)
        }
    }
}

// ===========================================================================
// The memo
// ===========================================================================

/// One `deepcopy` pass's memo: CPython's `id(source) -> copy` dict, plus the
/// sources it has seen.
///
/// Sources are pinned for the whole pass because heap ids are recycled: one
/// dying mid-pass would let a later object inherit its id, and its entry.
pub(crate) struct Memo {
    /// The memo dict — the caller's when they passed one, else freshly made.
    dict: Value,
    /// Every source visited this pass, keeping their ids unique.
    keep_alive: Vec<Value>,
}

impl Memo {
    /// `None` allocates a fresh dict; a caller's dict is adopted, so they see
    /// what the pass recorded.
    fn new(memo: Value, vm: &mut VM<'_>) -> RunResult<Self> {
        let dict = match memo {
            Value::None => Value::Ref(vm.heap.allocate(HeapData::Dict(Dict::new()))),
            Value::Ref(id) if matches!(vm.heap.read(id), HeapReadOutput::Dict(_)) => memo,
            other => {
                let type_name = other.py_type_name(vm);
                other.drop_with(vm);
                return Err(ExcType::type_error(format!(
                    "deepcopy() memo must be a dict or None, not {type_name}"
                )));
            }
        };
        Ok(Self {
            dict,
            keep_alive: Vec::new(),
        })
    }

    /// Returns the copy this pass already made for `source`, if any.
    pub(crate) fn get(&self, source: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
        let key = source.id().into_value(vm.heap);
        defer_drop!(key, vm);
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.dict_id()) else {
            unreachable!("memo is a dict")
        };
        dict.dict_get(key, vm)
    }

    /// Records `copy` as the copy of `source`, and pins `source` for the pass.
    pub(crate) fn insert(&mut self, source: &Value, copy: &Value, vm: &mut VM<'_>) -> RunResult<()> {
        let key = source.id().into_value(vm.heap);
        let value = copy.clone_with_heap(vm.heap);
        let HeapReadOutput::Dict(mut dict) = vm.heap.read(self.dict_id()) else {
            unreachable!("memo is a dict")
        };
        if let Some(replaced) = dict.set(key, value, vm)? {
            replaced.drop_with(vm);
        }
        vm.heap.tracker.check_allocation(VALUE_SIZE)?;
        self.keep_alive.push(source.clone_with_heap(vm.heap));
        Ok(())
    }

    /// The memo dict as an owned value, for handing to `__deepcopy__`.
    pub(crate) fn dict_value(&self, vm: &VM<'_>) -> Value {
        self.dict.clone_with_heap(vm.heap)
    }

    fn dict_id(&self) -> HeapId {
        self.dict.ref_id().expect("memo dict is heap allocated")
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Memo {
    fn drop_with(self, context: &mut C) {
        self.dict.drop_with(context);
        self.keep_alive.drop_with(context);
    }
}

// ===========================================================================
// Filling helpers
// ===========================================================================

/// Clones `len` items out of a container, preflighting the slot bytes.
///
/// Polls the clock as the deep-copy loops do: a shallow copy of a large
/// container reaches no instruction checkpoint between entering `copy.copy`
/// and returning, so without this the whole walk is invisible to
/// `max_duration`. The clones are guarded because that poll can now cut the
/// loop short with items already taken.
pub(crate) fn clone_items<'h>(
    len: usize,
    vm: &mut VM<'h>,
    mut clone_item: impl FnMut(usize, &mut VM<'h>) -> Value,
) -> RunResult<Vec<Value>> {
    vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
    let mut guard = DropGuard::new(Vec::with_capacity(len), vm);
    for index in 0..len {
        let (items, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        items.push(clone_item(index, vm));
    }
    Ok(guard.into_inner())
}

/// Clones an instance's `__dict__` entries, polling and guarded for the reason
/// [`clone_items`] is.
fn clone_attrs(id: HeapId, vm: &mut VM<'_>) -> RunResult<Vec<(Value, Value)>> {
    let len = attrs(id, vm).len();
    vm.heap
        .tracker
        .check_allocation(len.saturating_mul(2).saturating_mul(VALUE_SIZE))?;
    let mut guard = DropGuard::new(Vec::with_capacity(len), vm);
    for index in 0..len {
        let (pairs, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        let attrs = attrs(id, vm);
        let (key, value) = attrs.item_at(index).expect("index is in bounds");
        pairs.push((key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)));
    }
    Ok(guard.into_inner())
}

/// Borrows an instance's attribute dict.
fn attrs<'a>(id: HeapId, vm: &'a VM<'_>) -> &'a Dict {
    match vm.heap.get(id) {
        HeapData::Instance(instance) => instance.attrs(),
        _ => unreachable!("caller checked the type"),
    }
}

/// Moves owned `pairs` into a dict copy.
///
/// The iterator is guarded because `set` re-hashes each key, running a user
/// `__hash__` that can raise: the pairs the loop has not reached yet are still
/// owned here, and a plain `for` would drop them without releasing them.
///
/// That re-hashing also makes this the most expensive of the shallow-copy
/// loops — a wide dict spends hundreds of milliseconds here — so it polls the
/// clock as well; see `timeout_in_shallow_copy_fill_loop`. The poll runs
/// *before* each pair is taken from the iterator, so a timeout leaves every
/// remaining pair inside the guard; pulling first would strand the one in hand.
fn insert_pairs(dict_id: HeapId, pairs: Vec<(Value, Value)>, vm: &mut VM<'_>) -> RunResult<()> {
    let pairs = pairs.into_iter();
    defer_drop_mut!(pairs, vm);
    for index in 0.. {
        vm.heap.tracker.check_time_every(index)?;
        let Some((key, value)) = pairs.next() else {
            break;
        };
        let HeapReadOutput::Dict(mut dest) = vm.heap.read(dict_id) else {
            unreachable!("copy was allocated as a dict")
        };
        // `set` takes ownership of the pair and releases it on failure.
        if let Some(replaced) = dest.set(key, value, vm)? {
            replaced.drop_with(vm);
        }
    }
    Ok(())
}

/// Moves owned `pairs` into the `__dict__` of an instance copy, bypassing
/// `setattr` as [`deep_copy_attrs`] does. Guarded, and polled before the pull,
/// for the reasons [`insert_pairs`] is.
fn insert_attrs(id: HeapId, pairs: Vec<(Value, Value)>, vm: &mut VM<'_>) -> RunResult<()> {
    let pairs = pairs.into_iter();
    defer_drop_mut!(pairs, vm);
    for index in 0.. {
        vm.heap.tracker.check_time_every(index)?;
        let Some((key, value)) = pairs.next() else {
            break;
        };
        let HeapReadOutput::Instance(mut instance) = vm.heap.read(id) else {
            unreachable!("caller allocated an instance")
        };
        let replaced = instance.attrs_mut().set(key, value, vm)?;
        if let Some(replaced) = replaced {
            replaced.drop_with(vm);
        }
    }
    Ok(())
}

// ===========================================================================
// Small helpers
// ===========================================================================

/// Releases a half-built copy when filling it failed.
fn release_on_error(copy: Value, filled: RunResult<()>, vm: &mut VM<'_>) -> RunResult<Value> {
    match filled {
        Ok(()) => Ok(copy),
        Err(e) => {
            copy.drop_with(vm);
            Err(e)
        }
    }
}

/// Whether the two values are the same object (Python's `is`).
fn same_object(a: &Value, b: &Value) -> bool {
    a.id() == b.id()
}

/// The `TypeError` CPython's pickler raises for a type it cannot rebuild.
fn cannot_copy(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!("cannot pickle '{}' object", value.py_type_name(vm)))
}
