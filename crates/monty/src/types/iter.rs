//! Iterator support for Python for loops and the `iter()` type constructor.
//!
//! This module provides the `MontyIter` struct which encapsulates iteration state
//! for different iterable types. It uses index-based iteration internally to avoid
//! borrow conflicts when accessing the heap during iteration.
//!
//! The design stores iteration state (indices) rather than Rust iterators, allowing
//! `for_next()` to take `&mut Heap` for cloning values and allocating strings.
//!
//! For constructors like `list()` and `tuple()`, use `MontyIter::new()` followed
//! by `collect()` to materialize all items into a Vec.
//!
//! ## Builtin Support
//!
//! The `iterator_next()` helper implements the `next()` builtin.

use std::mem;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::{BytesId, Interns},
    resource::{ResourceError, ResourceTracker, check_estimated_size},
    types::{PyTrait, Range, Type, dict_view::DictView, str::allocate_char},
    value::{VALUE_SIZE, Value, ValueRead},
};

/// Iterator state for Python for loops.
///
/// Contains the current iteration index and the type-specific iteration data.
/// Uses index-based iteration to avoid borrow conflicts when accessing the heap.
///
/// For strings, stores the string content with a byte offset for O(1) UTF-8 iteration.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MontyIter {
    /// Current iteration index, shared across all iterator types.
    index: usize,
    /// Type-specific iteration data.
    iter_value: IterValue,
    /// the actual Value being iterated over.
    value: Value,
}

impl MontyIter {
    /// Creates an iterator from the `iter()` constructor call.
    ///
    /// - `iter(iterable)` - Returns an iterator for the iterable. If the argument is
    ///   already an iterator, returns the same object.
    /// - `iter(callable, sentinel)` - Not yet supported.
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let (iterable, sentinel) = args.get_one_two_args("iter", vm.heap)?;

        if let Some(s) = sentinel {
            // Two-argument form: iter(callable, sentinel)
            // This is the sentinel iteration protocol, not yet supported
            iterable.drop_with(vm);
            s.drop_with(vm);
            return Err(ExcType::type_error("iter(callable, sentinel) is not yet supported"));
        }

        let iterator = iterable.py_iter(vm);
        iterable.drop_with(vm);
        iterator
    }

    /// Creates a new MontyIter from a Value.
    ///
    /// Returns an error if the value is not iterable.
    /// For strings, copies the string content for byte-offset based iteration.
    /// For ranges, the data is copied so the heap reference is dropped immediately.
    pub fn new(mut value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        if let Value::Ref(list_id) = value {
            let list_iterator = match vm.heap.read(list_id) {
                HeapReadOutput::List(list) => Some(list.py_iter(Some(list_id), vm)?),
                _ => None,
            };
            if let Some(list_iterator) = list_iterator {
                value.drop_with(vm);
                let heap_id = list_iterator.ref_id().expect("list iterators are heap allocated");
                return Ok(Self {
                    index: 0,
                    iter_value: IterValue::Opaque { heap_id },
                    value: list_iterator,
                });
            }
        }

        match IterValue::new(&value, vm) {
            Ok(Some(iter_value)) => {
                // For Range, we copy next/step/len into ForIterValue::Range, so we don't need
                // to keep the heap object alive during iteration. Drop it immediately to avoid
                // GC issues (the Range isn't in any namespace slot, so GC wouldn't see it).
                // Same for IterStr which copies the string content.
                if matches!(iter_value, IterValue::Range { .. } | IterValue::IterStr { .. }) {
                    value.drop_with(vm);
                    value = Value::None;
                }
                Ok(Self {
                    index: 0,
                    iter_value,
                    value,
                })
            }
            Ok(None) => {
                let err = ExcType::type_error_not_iterable(&value.py_type_name(vm));
                value.drop_with(vm);
                Err(err)
            }
            Err(err) => {
                value.drop_with(vm);
                Err(err)
            }
        }
    }

    /// Drops the iterator and its held value properly.
    pub fn drop_with(self, heap: &mut impl ContainsHeap) {
        self.value.drop_with(heap);
    }

    /// Collects HeapIds from this iterator for reference counting cleanup.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.value.py_dec_ref_ids(stack);
    }

    /// Returns a reference to the underlying value being iterated.
    ///
    /// Used by GC to traverse heap references held by the iterator.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Returns the next item from the iterator, advancing the internal index.
    ///
    /// Returns `Ok(None)` when the iterator is exhausted.
    /// Returns `Err` if allocation fails (for string character iteration) or if
    /// a dict/set changes size during iteration (RuntimeError).
    pub fn for_next(&mut self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        // Rust-side loops do not return to the bytecode dispatch loop between
        // items, so this is their shared timeout boundary.
        vm.heap.check_time()?;
        if let IterValue::Opaque { heap_id } = &self.iter_value {
            return advance_opaque(*heap_id, vm);
        }
        match &mut self.iter_value {
            IterValue::Range { next, step, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let value = *next;
                *next += *step;
                self.index += 1;
                Ok(Some(Value::Int(value)))
            }
            IterValue::IterStr {
                string,
                byte_offset,
                len,
            } => {
                if self.index >= *len {
                    Ok(None)
                } else {
                    // Get next char at current byte offset
                    let c = string[*byte_offset..]
                        .chars()
                        .next()
                        .expect("index < len implies char exists");
                    *byte_offset += c.len_utf8();
                    self.index += 1;
                    Ok(Some(allocate_char(c, vm.heap)?))
                }
            }
            IterValue::InternBytes { bytes_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let bytes = vm.interns.get_bytes(*bytes_id);
                Ok(Some(Value::Int(i64::from(bytes[i]))))
            }
            IterValue::HeapRef {
                heap_id,
                len,
                checks_mutation,
            } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let expected_len = checks_mutation.then_some(*len);
                let item = get_heap_item(vm, *heap_id, self.index, expected_len)?;
                self.index += 1;
                Ok(Some(item))
            }
            IterValue::Opaque { .. } => unreachable!("opaque iterators return before inline dispatch"),
        }
    }

    /// Returns the remaining size for iterables based on current state.
    ///
    /// For concrete iterables, returns their exact remaining count.
    ///
    /// Opaque iterators may provide a type-specific hint; otherwise this returns zero.
    pub fn size_hint(&self, heap: &Heap<impl ResourceTracker>) -> usize {
        let len = match &self.iter_value {
            IterValue::Range { len, .. } | IterValue::IterStr { len, .. } | IterValue::InternBytes { len, .. } => *len,
            IterValue::HeapRef { len, .. } => *len,
            IterValue::Opaque { heap_id } => match opaque_target(*heap_id, heap) {
                Ok(OpaqueTarget::Iter(heap_id)) => match heap.get(heap_id) {
                    HeapData::Iter(iter) => iter.size_hint(heap),
                    _ => unreachable!("opaque_target validated the iterator type"),
                },
                Ok(OpaqueTarget::ListIterator(heap_id)) => match heap.get(heap_id) {
                    HeapData::ListIterator(iter) => iter.size_hint(heap),
                    _ => unreachable!("opaque_target validated the iterator type"),
                },
                Err(_) => 0,
            },
        };
        len.saturating_sub(self.index)
    }

    /// Returns a capacity hint that is safe to pass to `with_capacity` and friends.
    ///
    /// `size_hint()` reports the exact remaining length of the iterable, which for
    /// `range(huge)` can be astronomically large. Passing that straight to a
    /// container constructor calls the global allocator before the resource tracker
    /// can reject it; the allocator either aborts the process on failure (which is
    /// not catchable) or succeeds and the host is OOM-killed when the pages are
    /// touched. Both outcomes bypass the configured memory limit entirely.
    ///
    /// This helper validates the requested allocation against the resource tracker
    /// (raising `MemoryError` if it would exceed the budget) and clamps the result
    /// to a small fixed bound. The clamp makes the pre-allocation defensively safe
    /// even when no limits are configured: the container still grows naturally as
    /// elements are appended, with each element tracked individually, so the hint
    /// only matters for performance, never for correctness.
    pub fn preallocation_hint(
        &self,
        elem_size: usize,
        vm: &VM<'_, impl ResourceTracker>,
    ) -> Result<usize, ResourceError> {
        checked_preallocation_hint(self.size_hint(vm.heap), elem_size, vm.heap.tracker())
    }

    /// Materializes all remaining items into a `T` (typically `Vec<Value>`).
    ///
    /// Consumes the iterator and returns all items. Used by `list()`, `tuple()`,
    /// `sorted()`, `reversed()`, and similar constructors that need every item.
    ///
    /// # Resource safety
    ///
    /// The destination `T` is backed by the global Rust allocator, *outside*
    /// Monty's resource tracker. The tracker would otherwise only see the
    /// finished buffer when it is wrapped into a heap object — far too late for
    /// a cheap-to-represent but enormous iterable like `list(range(10**12))` or
    /// `tuple(x for x in ...)`, where the whole native buffer is built first and
    /// the host is driven to OOM or a capacity-overflow abort before that
    /// post-construction check ever runs (an uncatchable sandbox escape).
    ///
    /// [`HeapedMontyIter`] therefore re-estimates the projected buffer size
    /// after every element and runs it through the tracker, so an over-budget
    /// collection fails *during* accumulation, near the configured limit,
    /// rather than after full materialization. This is the only sanctioned way
    /// to drain a `MontyIter` into a native container — `MontyIter`
    /// deliberately does not implement [`Iterator`] so callers cannot bypass
    /// this check with a plain `.collect()`.
    pub fn collect<T: FromIterator<Value>>(self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<T> {
        let mut guard = DropGuard::new(self, vm);
        let (this, vm) = guard.as_parts_mut();
        if let IterValue::Opaque { heap_id } = &this.iter_value {
            let target = opaque_target(*heap_id, vm.heap).map_err(OpaqueError::into_exception)?;
            let target_id = target.heap_id();
            vm.heap.inc_ref(target_id);
            let target_value = Value::Ref(target_id);
            defer_drop!(target_value, vm);
            let mut iter = target_value.read(vm);
            HeapedMontyIter {
                iter: &mut iter,
                vm,
                yielded: 0,
            }
            .collect()
        } else {
            HeapedMontyIter {
                iter: this,
                vm,
                yielded: 0,
            }
            .collect()
        }
    }
}

/// Validates and clamps an iterator capacity hint before native allocation.
///
/// The clamp bounds untracked preallocation to a few MiB while retaining the
/// performance benefit for moderate containers.
pub(crate) fn checked_preallocation_hint(
    hint: usize,
    elem_size: usize,
    tracker: &impl ResourceTracker,
) -> Result<usize, ResourceError> {
    /// Upper bound on the number of slots reserved from an untrusted hint.
    const MAX_PREALLOCATION_HINT: usize = 65_536;

    check_estimated_size(hint.saturating_mul(elem_size), tracker)?;
    Ok(hint.min(MAX_PREALLOCATION_HINT))
}

/// Adapts an internal iterator driver to [`Iterator`] while enforcing the
/// memory budget incrementally.
///
/// `collect()` builds a native `Vec`/`SmallVec` whose backing storage is
/// allocated by the global Rust allocator and is invisible to Monty's resource
/// tracker until the finished object is handed to the heap. Each [`next`] call
/// therefore re-estimates the projected buffer size (`yielded * VALUE_SIZE`)
/// and validates it against the tracker via [`check_estimated_size`], so a
/// runaway collection is rejected near the limit instead of after it has
/// already exhausted host memory. The check is free below
/// `LARGE_RESULT_THRESHOLD` (a single multiply and comparison), matching the
/// policy used by [`MontyIter::preallocation_hint`].
///
/// [`next`]: Iterator::next
struct HeapedMontyIter<'this, 'h, T: ResourceTracker, I: CollectIter<'h>> {
    /// The underlying iterator being drained.
    iter: &'this mut I,
    /// VM handle, needed both to advance `iter` and to reach the tracker.
    vm: &'this mut VM<'h, T>,
    /// Count of elements yielded so far; drives the running size estimate.
    yielded: usize,
}

impl<'h, T: ResourceTracker, I: CollectIter<'h>> Iterator for HeapedMontyIter<'_, 'h, T, I> {
    type Item = RunResult<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next_value(self.vm) {
            Ok(None) => None,
            Err(e) => Some(Err(e)),
            Ok(Some(value)) => {
                self.yielded += 1;
                let estimated = self.yielded.saturating_mul(VALUE_SIZE);
                // Borrow order matters: `for_next` took `&mut vm` above and has
                // already returned, so the immutable tracker borrow here is fine.
                match check_estimated_size(estimated, self.vm.heap.tracker()) {
                    Ok(()) => Some(Ok(value)),
                    // Over budget mid-collection. The partially built buffer is
                    // dropped without `drop_with`, leaking the refcounts of
                    // `value` and the already-collected items. This is the
                    // existing, explicitly sanctioned behaviour for resource
                    // errors (terminal; heap state is discarded — see CLAUDE.md
                    // and the `Heap` resource-limit docs).
                    Err(e) => Some(Err(e.into())),
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.iter.remaining(self.vm);
        (remaining, Some(remaining))
    }
}

/// Common interface for stack and retained-heap iterators drained by `collect()`.
trait CollectIter<'h> {
    /// Advances the iterator by one item.
    fn next_value(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>>;

    /// Returns an internal remaining-length hint when available.
    fn remaining(&self, vm: &VM<'h, impl ResourceTracker>) -> usize;
}

impl<'h> CollectIter<'h> for MontyIter {
    fn next_value(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        self.for_next(vm)
    }

    fn remaining(&self, vm: &VM<'h, impl ResourceTracker>) -> usize {
        self.size_hint(vm.heap)
    }
}

impl<'h> CollectIter<'h> for ValueRead<'h, '_> {
    fn next_value(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        self.py_next(vm)
    }

    fn remaining(&self, vm: &VM<'h, impl ResourceTracker>) -> usize {
        self.iter_size_hint(vm)
    }
}

impl<'h> HeapRead<'h, MontyIter> {
    /// Advances an iterator without checking the execution timeout.
    ///
    /// Bytecode callers are checked by the VM dispatch loop; Rust-side loops
    /// must use [`MontyIter::for_next`] or [`ValueRead::py_next`].
    pub(crate) fn advance(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        if let IterValue::Opaque { heap_id } = &self.get(vm.heap).iter_value {
            return advance_opaque(*heap_id, vm);
        }
        let this = self.get_mut(vm.heap);
        match &mut this.iter_value {
            IterValue::Range { next, step, len } => {
                if this.index >= *len {
                    Ok(None)
                } else {
                    let value = *next;
                    *next += *step;
                    this.index += 1;
                    Ok(Some(Value::Int(value)))
                }
            }
            IterValue::IterStr {
                string,
                byte_offset,
                len,
            } => {
                if this.index >= *len {
                    Ok(None)
                } else {
                    // Get the next character at current byte offset
                    let c = string[*byte_offset..]
                        .chars()
                        .next()
                        .expect("index < len implies char exists");
                    this.index += 1;
                    *byte_offset += c.len_utf8();
                    Ok(Some(allocate_char(c, vm.heap)?))
                }
            }
            IterValue::InternBytes { bytes_id, len } => {
                if this.index >= *len {
                    Ok(None)
                } else {
                    let i = this.index;
                    this.index += 1;
                    let bytes = vm.interns.get_bytes(*bytes_id);
                    Ok(Some(Value::Int(i64::from(bytes[i]))))
                }
            }
            IterValue::HeapRef {
                heap_id,
                len,
                checks_mutation,
            } => {
                if this.index >= *len {
                    return Ok(None);
                }
                let heap_id = *heap_id;
                let index = this.index;
                let expected_len = checks_mutation.then_some(*len);
                let item = get_heap_item(vm, heap_id, index, expected_len)?;
                self.get_mut(vm.heap).index += 1;
                Ok(Some(item))
            }
            IterValue::Opaque { .. } => unreachable!("opaque iterators return before inline dispatch"),
        }
    }
}

/// Collects every remaining item of an iterable into a `Vec`.
///
/// For the sites that need all items at once (sequence unpacking, `*` literal
/// unpack). Clones `value`, so callers holding a borrowed value — e.g. behind
/// `defer_drop!` — can use it without giving up ownership.
pub fn collect_iterable(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Vec<Value>> {
    let cloned = value.clone_with_heap(vm.heap);
    MontyIter::new(cloned, vm)?.collect(vm)
}

/// Pulls at most `limit` items from an iterable, stopping early.
///
/// Sequence unpacking only needs to know whether there is one item too many, and
/// CPython stops consuming there. Draining instead would over-consume a shared
/// iterator and change the error message.
pub fn collect_iterable_bounded(
    value: &Value,
    limit: usize,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<Value>> {
    let cloned = value.clone_with_heap(vm.heap);
    let iter = MontyIter::new(cloned, vm)?;
    let mut guard = DropGuard::new(iter, vm);
    let (iter, vm) = guard.as_parts_mut();
    let mut items = Vec::new();
    while items.len() < limit {
        match iter.for_next(vm) {
            Ok(Some(value)) => items.push(value),
            Ok(None) => break,
            Err(e) => {
                for item in items {
                    item.drop_with(vm);
                }
                return Err(e);
            }
        }
    }
    Ok(items)
}

/// The most opaque iterator links [`flatten_opaque`] follows before rejecting a chain.
const MAX_OPAQUE_DEPTH: usize = 1000;

/// A heap iterator which can be driven without further delegation.
#[derive(Clone, Copy)]
enum OpaqueTarget {
    /// A terminal general-purpose iterator.
    Iter(HeapId),
    /// A list iterator with independently retained state.
    ListIterator(HeapId),
}

impl OpaqueTarget {
    /// Returns the heap id of the validated terminal iterator.
    fn heap_id(self) -> HeapId {
        match self {
            Self::Iter(heap_id) | Self::ListIterator(heap_id) => heap_id,
        }
    }
}

/// Why an opaque iterator could not be resolved safely.
enum OpaqueError {
    /// The chain was nested, cyclic, or unreasonably deep.
    TooDeep,
    /// A link pointed at a heap entry that is not an iterator.
    NotAnIterator,
}

impl OpaqueError {
    /// Converts malformed snapshot state into a catchable runtime error.
    fn into_exception(self) -> RunError {
        match self {
            Self::TooDeep => ExcType::runtime_error_iter_delegation_too_deep(),
            Self::NotAnIterator => ExcType::runtime_error_iter_delegation_invalid(),
        }
    }
}

/// Flattens opaque iterator chains while constructing a stack iterator.
///
/// The walk is iterative and bounded because restored snapshot data is untrusted.
/// Its result is always safe to dispatch without another opaque hop.
fn flatten_opaque(start: HeapId, heap: &Heap<impl ResourceTracker>) -> Result<HeapId, OpaqueError> {
    let mut current = start;
    for _ in 0..MAX_OPAQUE_DEPTH {
        match heap.get(current) {
            HeapData::Iter(inner) => match inner.iter_value {
                IterValue::Opaque { heap_id } => current = heap_id,
                _ => return Ok(current),
            },
            HeapData::ListIterator(_) => return Ok(current),
            _ => return Err(OpaqueError::NotAnIterator),
        }
    }
    Err(OpaqueError::TooDeep)
}

/// Resolves a direct opaque target, rejecting nested links from malformed snapshots.
fn opaque_target(heap_id: HeapId, heap: &Heap<impl ResourceTracker>) -> Result<OpaqueTarget, OpaqueError> {
    match heap.get(heap_id) {
        HeapData::Iter(inner) if matches!(inner.iter_value, IterValue::Opaque { .. }) => Err(OpaqueError::TooDeep),
        HeapData::Iter(_) => Ok(OpaqueTarget::Iter(heap_id)),
        HeapData::ListIterator(_) => Ok(OpaqueTarget::ListIterator(heap_id)),
        _ => Err(OpaqueError::NotAnIterator),
    }
}

/// Advances a direct opaque target without recursive iterator dispatch.
fn advance_opaque(heap_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
    match opaque_target(heap_id, vm.heap).map_err(OpaqueError::into_exception)? {
        OpaqueTarget::Iter(heap_id) => {
            let HeapReadOutput::Iter(mut iter) = vm.heap.read(heap_id) else {
                unreachable!("opaque_target validated the iterator type")
            };
            iter.advance(vm)
        }
        OpaqueTarget::ListIterator(heap_id) => {
            let HeapReadOutput::ListIterator(mut iter) = vm.heap.read(heap_id) else {
                unreachable!("opaque_target validated the iterator type")
            };
            iter.py_next(vm)
        }
    }
}

/// Gets an item from a heap-allocated container at the given index.
///
/// Returns an error if a dict or set changed size during iteration.
fn get_heap_item(
    vm: &VM<'_, impl ResourceTracker>,
    heap_id: HeapId,
    index: usize,
    expected_len: Option<usize>,
) -> RunResult<Value> {
    match vm.heap.get(heap_id) {
        HeapData::Tuple(tuple) => Ok(tuple.as_slice()[index].clone_with_heap(vm)),
        HeapData::NamedTuple(namedtuple) => Ok(namedtuple.as_vec()[index].clone_with_heap(vm)),
        HeapData::Dict(dict) => {
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(dict.key_at(index).expect("index should be valid").clone_with_heap(vm))
        }
        HeapData::DictKeysView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(dict.key_at(index).expect("index should be valid").clone_with_heap(vm))
        }
        HeapData::DictItemsView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            let (key, value) = dict.item_at(index).expect("index should be valid");
            Ok(super::allocate_tuple(
                smallvec::smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)],
                vm.heap,
            )?)
        }
        HeapData::DictValuesView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(dict.value_at(index).expect("index should be valid").clone_with_heap(vm))
        }
        HeapData::Bytes(bytes) => Ok(Value::Int(i64::from(bytes.as_slice()[index]))),
        HeapData::Set(set) => {
            if let Some(expected) = expected_len
                && set.len() != expected
            {
                return Err(ExcType::runtime_error_set_changed_size());
            }
            Ok(set
                .storage()
                .value_at(index)
                .expect("index should be valid")
                .clone_with_heap(vm))
        }
        HeapData::FrozenSet(frozenset) => Ok(frozenset
            .storage()
            .value_at(index)
            .expect("index should be valid")
            .clone_with_heap(vm)),
        _ => panic!("get_heap_item: unexpected heap data type"),
    }
}

/// Gets the next item from an iterator.
///
/// If the iterator is exhausted:
/// - If `default` is `Some`, returns the default value
/// - If `default` is `None`, raises `StopIteration`
///
/// This implements Python's `next()` builtin semantics.
///
/// # Arguments
/// * `iter_value` - Must be an iterator (heap-allocated MontyIter)
/// * `default` - Optional default value to return when exhausted
/// * `heap` - The heap for memory operations
/// * `interns` - String interning table
///
/// # Errors
/// Returns `StopIteration` if exhausted with no default, or propagates errors from iteration.
pub fn iterator_next(
    iter_value: &Value,
    default: Option<Value>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let mut default_guard = DropGuard::new(default, vm);
    let vm = default_guard.ctx();

    let Value::Ref(iter_id) = iter_value else {
        return Err(ExcType::type_error_not_iterator(&iter_value.py_type_name(vm)));
    };
    let result = vm.heap.read(*iter_id).py_next(vm)?;

    // Get next item using the MontyIter::advance_on_heap method
    match result {
        Some(item) => Ok(item),
        None => {
            // Iterator exhausted
            match default_guard.into_inner() {
                Some(d) => Ok(d),
                None => Err(ExcType::stop_iteration()),
            }
        }
    }
}

/// Type-specific iteration data for different Python iterable types.
///
/// Each variant stores the data needed to iterate over a specific type,
/// excluding the index which is stored in the parent `MontyIter` struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum IterValue {
    /// Iterating over a Range, yields `Value::Int`.
    Range {
        /// Next value to yield.
        next: i64,
        /// Step between values.
        step: i64,
        /// Total number of elements.
        len: usize,
    },
    /// Iterating over a string (heap or interned), yields single-char Str values.
    ///
    /// Stores a copy of the string content plus a byte offset for O(1) UTF-8 character access.
    /// We store the string rather than referencing the heap because `for_next()` needs mutable
    /// heap access to allocate the returned character strings, which would conflict with
    /// borrowing the source string from the heap.
    IterStr {
        /// Copy of the string content for iteration.
        string: String,
        /// Current byte offset into the string (points to next char to yield).
        byte_offset: usize,
        /// Total number of characters in the string.
        len: usize,
    },
    /// Iterating over interned bytes, yields `Value::Int` for each byte.
    InternBytes { bytes_id: BytesId, len: usize },
    /// Iterating over a heap-allocated tuple, dict, bytes, set, or frozen set.
    ///
    /// `checks_mutation` is true for dicts and sets, which reject size changes.
    HeapRef {
        heap_id: HeapId,
        len: usize,
        checks_mutation: bool,
    },
    /// Iterating over an iterator type not inlined in this type. It is responsible for all iteration.
    Opaque { heap_id: HeapId },
}

impl IterValue {
    fn new(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Self>> {
        match value {
            Value::InternString(string_id) => Ok(Some(Self::from_str(vm.interns.get_str(*string_id)))),
            Value::InternBytes(bytes_id) => Ok(Some(Self::from_intern_bytes(*bytes_id, vm.interns))),
            Value::Ref(heap_id) => Self::from_heap_data(*heap_id, vm.heap),
            _ => Ok(None),
        }
    }

    /// Creates a Range iterator value.
    fn from_range(range: &Range) -> Self {
        Self::Range {
            next: range.start,
            step: range.step,
            len: range.len(),
        }
    }

    /// Creates an iterator value over a string.
    ///
    /// Copies the string content and counts characters for the length field.
    fn from_str(s: &str) -> Self {
        let len = s.chars().count();
        Self::IterStr {
            string: s.to_owned(),
            byte_offset: 0,
            len,
        }
    }

    /// Creates an iterator value over interned bytes.
    fn from_intern_bytes(bytes_id: BytesId, interns: &Interns) -> Self {
        let bytes = interns.get_bytes(bytes_id);
        Self::InternBytes {
            bytes_id,
            len: bytes.len(),
        }
    }

    /// Creates an iterator value from heap data.
    fn from_heap_data(heap_id: HeapId, heap: &Heap<impl ResourceTracker>) -> RunResult<Option<Self>> {
        let iter_value = match heap.get(heap_id) {
            // Tuple/NamedTuple/Bytes/FrozenSet: captured len, no mutation check
            HeapData::Tuple(tuple) => Some(Self::HeapRef {
                heap_id,
                len: tuple.as_slice().len(),
                checks_mutation: false,
            }),
            HeapData::NamedTuple(namedtuple) => Some(Self::HeapRef {
                heap_id,
                len: namedtuple.len(),
                checks_mutation: false,
            }),
            HeapData::Bytes(b) => Some(Self::HeapRef {
                heap_id,
                len: b.len(),
                checks_mutation: false,
            }),
            HeapData::FrozenSet(frozenset) => Some(Self::HeapRef {
                heap_id,
                len: frozenset.len(),
                checks_mutation: false,
            }),
            // Dict and dict views: captured len, WITH mutation check
            HeapData::Dict(dict) => Some(Self::HeapRef {
                heap_id,
                len: dict.len(),
                checks_mutation: true,
            }),
            HeapData::DictKeysView(view) => Some(Self::HeapRef {
                heap_id,
                len: view.dict(heap).len(),
                checks_mutation: true,
            }),
            HeapData::DictItemsView(view) => Some(Self::HeapRef {
                heap_id,
                len: view.dict(heap).len(),
                checks_mutation: true,
            }),
            HeapData::DictValuesView(view) => Some(Self::HeapRef {
                heap_id,
                len: view.dict(heap).len(),
                checks_mutation: true,
            }),
            HeapData::Set(set) => Some(Self::HeapRef {
                heap_id,
                len: set.len(),
                checks_mutation: true,
            }),
            // String: copy content for iteration
            HeapData::Str(s) => Some(Self::from_str(s.as_str())),
            // Range: copy values for iteration
            HeapData::Range(range) => Some(Self::from_range(range)),
            HeapData::Iter(_) | HeapData::ListIterator(_) => Some(Self::Opaque {
                heap_id: flatten_opaque(heap_id, heap).map_err(OpaqueError::into_exception)?,
            }),
            // other types are not iterable
            _ => None,
        };
        Ok(iter_value)
    }
}

impl<C: ContainsHeap> DropWithContext<C> for MontyIter {
    #[inline]
    fn drop_with(self, heap: &mut C) {
        Self::drop_with(self, heap);
    }
}

impl HeapItem for MontyIter {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.value.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, MontyIter> {
    fn py_type(&self, _: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Iterator
    }

    fn py_len(&self, _: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let self_id = self_id.expect("heap values have an id");
        vm.heap.inc_ref(self_id);
        Ok(Value::Ref(self_id))
    }

    fn py_next(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        self.advance(vm)
    }
}
