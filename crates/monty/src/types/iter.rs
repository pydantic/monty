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
    exception_private::{ExcType, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::{BytesId, Interns},
    resource::{ResourceError, ResourceTracker, check_estimated_size},
    types::{PyTrait, Range, dict_view::DictView, str::allocate_char},
    value::{VALUE_SIZE, Value},
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

        // Check if already an iterator - return self
        if let Value::Ref(id) = &iterable
            && matches!(vm.heap.get(*id), HeapData::Iter(_))
        {
            // Already an iterator - return it (refcount already correct from caller)
            return Ok(iterable);
        }

        // Create new iterator
        let iter = Self::new(iterable, vm)?;
        let id = vm.heap.allocate(HeapData::Iter(iter))?;
        Ok(Value::Ref(id))
    }

    /// Creates a new MontyIter from a Value.
    ///
    /// Returns an error if the value is not iterable.
    /// For strings, copies the string content for byte-offset based iteration.
    /// For ranges, the data is copied so the heap reference is dropped immediately.
    pub fn new(mut value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        if let Some(iter_value) = IterValue::new(&value, vm) {
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
        } else {
            let err = ExcType::type_error_not_iterable(&value.py_type_name(vm));
            value.drop_with(vm);

            Err(err)
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
        // Check timeout on every iteration step. For NoLimitTracker this is
        // inlined as a no-op. For LimitTracker it ensures that Rust-side loops
        // (sum, sorted, min, max, etc.) cannot bypass the VM's per-instruction
        // timeout check by running entirely within a single bytecode instruction.
        vm.heap.check_time()?;
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
                // Check exhaustion for types with captured len
                if let Some(l) = len
                    && self.index >= *l
                {
                    return Ok(None);
                }
                let i = self.index;
                let expected_len = if *checks_mutation { *len } else { None };
                let item = get_heap_item(vm, *heap_id, i, expected_len)?;
                // Check for list exhaustion (list can shrink during iteration)
                let Some(item) = item else {
                    return Ok(None);
                };
                self.index += 1;
                Ok(Some(item))
            }
            IterValue::IterHeapRef { iter_id } => {
                // Delegate to the terminal iterator so position is shared;
                // `self.value` keeps it alive, so this always resolves to an Iter.
                let target = resolve_delegate(*iter_id, vm.heap).map_err(DelegateError::into_exception)?;
                let HeapReadOutput::Iter(mut inner) = vm.heap.read(target) else {
                    unreachable!("resolve_delegate only returns Ok for an Iter")
                };
                inner.advance(vm)
            }
        }
    }

    /// Returns the remaining size for iterables based on current state.
    ///
    /// For immutable types (Range, Tuple, Str, Bytes, FrozenSet), returns the exact remaining count.
    /// For List, returns current length minus index (may change if list is mutated).
    /// For Dict and Set, returns the captured length minus index (used for size-change detection).
    pub fn size_hint(&self, heap: &Heap<impl ResourceTracker>) -> usize {
        let len = match &self.iter_value {
            IterValue::Range { len, .. } | IterValue::IterStr { len, .. } | IterValue::InternBytes { len, .. } => *len,
            IterValue::HeapRef { heap_id, len, .. } => {
                // For List (len=None), check current length dynamically
                len.unwrap_or_else(|| {
                    let HeapData::List(list) = heap.get(*heap_id) else {
                        panic!("HeapRef with len=None should only be List")
                    };
                    list.len()
                })
            }
            // The wrapper's own index is unused; report the terminal iterator's
            // remaining length. A cyclic/over-deep chain degrades to 0, which is
            // safe: this is only a capacity hint.
            // A broken chain degrades to 0: this is only a capacity hint, and the
            // consuming site raises the real error.
            IterValue::IterHeapRef { iter_id } => match resolve_delegate(*iter_id, heap) {
                Ok(target) => match heap.get(target) {
                    HeapData::Iter(inner) => inner.size_hint(heap),
                    _ => 0,
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
        /// Upper bound on the number of slots we are willing to reserve up front.
        ///
        /// Chosen so the worst-case pre-allocation (a few MiB) is small relative
        /// to any realistic memory budget, while still avoiding repeated
        /// reallocations for the common case of building moderate containers.
        const MAX_PREALLOCATION_HINT: usize = 65_536;
        let hint = self.size_hint(vm.heap);
        check_estimated_size(hint.saturating_mul(elem_size), vm.heap.tracker())?;
        Ok(hint.min(MAX_PREALLOCATION_HINT))
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
        HeapedMontyIter {
            iter: this,
            vm,
            yielded: 0,
        }
        .collect()
    }
}

/// Adapter that drives a [`MontyIter`] as a standard [`Iterator`] so it can be
/// fed to `collect()`, while enforcing the memory budget *incrementally*.
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
struct HeapedMontyIter<'this, 'h, T: ResourceTracker> {
    /// The underlying iterator being drained.
    iter: &'this mut MontyIter,
    /// VM handle, needed both to advance `iter` and to reach the tracker.
    vm: &'this mut VM<'h, T>,
    /// Count of elements yielded so far; drives the running size estimate.
    yielded: usize,
}

impl<T: ResourceTracker> Iterator for HeapedMontyIter<'_, '_, T> {
    type Item = RunResult<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.for_next(self.vm) {
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
        let remaining = self.iter.size_hint(self.vm.heap);
        (remaining, Some(remaining))
    }
}

impl<'h> HeapRead<'h, MontyIter> {
    /// Advances an iterator and returns the next value.
    ///
    /// Returns `Ok(None)` when the iterator is exhausted.
    /// Returns `Err` for dict/set size changes or allocation failures.
    pub(crate) fn advance(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
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
                if let Some(l) = len
                    && this.index >= *l
                {
                    return Ok(None);
                }

                let heap_id = *heap_id;
                let expected_len = if *checks_mutation { *len } else { None };
                let index = this.index;
                let item = get_heap_item(vm, heap_id, index, expected_len)?;

                // Check for list exhaustion (list can shrink during iteration)
                let Some(item) = item else {
                    return Ok(None);
                };
                self.get_mut(vm.heap).index += 1;
                Ok(Some(item))
            }
            IterValue::IterHeapRef { iter_id } => {
                // Delegate to the terminal iterator (see `for_next`).
                let iter_id = *iter_id;
                let target = resolve_delegate(iter_id, vm.heap).map_err(DelegateError::into_exception)?;
                let HeapReadOutput::Iter(mut inner) = vm.heap.read(target) else {
                    unreachable!("resolve_delegate only returns Ok for an Iter")
                };
                inner.advance(vm)
            }
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

/// The most `IterHeapRef` links [`resolve_delegate`] will follow before giving up.
///
/// Normal code produces depth 1; the cap only bounds the walk against a cyclic
/// chain, which is unreachable by construction but not from an untrusted snapshot.
const MAX_DELEGATION_DEPTH: usize = 1000;

/// Why [`resolve_delegate`] could not reach a terminal iterator.
///
/// Neither variant is reachable from Python — a delegating iterator always
/// points at an `Iter` and chains never exceed depth 1 — so both exist to keep
/// malformed snapshot data on a catchable path instead of panicking.
enum DelegateError {
    /// The chain exceeded [`MAX_DELEGATION_DEPTH`]; cyclic or absurdly deep.
    TooDeep,
    /// A link pointed at a heap entry that is not an iterator.
    NotAnIterator,
}

impl DelegateError {
    /// Converts to the `RuntimeError` the consuming site raises.
    fn into_exception(self) -> RunError {
        match self {
            Self::TooDeep => ExcType::runtime_error_iter_delegation_too_deep(),
            Self::NotAnIterator => ExcType::runtime_error_iter_delegation_invalid(),
        }
    }
}

/// Follows a chain of delegating (`IterHeapRef`) iterators to the terminal
/// iterator holding the iteration state.
///
/// MUST stay iterative, never recursing into `advance`: chain depth is
/// attacker-controlled, and native recursion would overflow the stack and abort
/// the process — uncatchable, and beyond any `ResourceTracker` limit. For the
/// same reason a non-iterator link is reported rather than passed on to the
/// caller, whose `heap.read` would panic on it.
fn resolve_delegate(start: HeapId, heap: &Heap<impl ResourceTracker>) -> Result<HeapId, DelegateError> {
    let mut current = start;
    for _ in 0..MAX_DELEGATION_DEPTH {
        let HeapData::Iter(inner) = heap.get(current) else {
            return Err(DelegateError::NotAnIterator);
        };
        match inner.iter_value {
            IterValue::IterHeapRef { iter_id } => current = iter_id,
            _ => return Ok(current),
        }
    }
    Err(DelegateError::TooDeep)
}

/// Gets an item from a heap-allocated container at the given index.
///
/// Returns `Ok(None)` if the index is out of bounds (for lists that shrunk during iteration).
/// Returns `Err` if a dict/set changed size during iteration (RuntimeError).
fn get_heap_item(
    vm: &VM<'_, impl ResourceTracker>,
    heap_id: HeapId,
    index: usize,
    expected_len: Option<usize>,
) -> RunResult<Option<Value>> {
    match vm.heap.get(heap_id) {
        HeapData::List(list) => {
            // Check if list shrunk during iteration
            if index >= list.len() {
                return Ok(None);
            }
            Ok(Some(list.as_slice()[index].clone_with_heap(vm)))
        }
        HeapData::Tuple(tuple) => Ok(Some(tuple.as_slice()[index].clone_with_heap(vm))),
        HeapData::NamedTuple(namedtuple) => Ok(Some(namedtuple.as_vec()[index].clone_with_heap(vm))),
        HeapData::Dict(dict) => {
            // Check for dict mutation
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(Some(
                dict.key_at(index).expect("index should be valid").clone_with_heap(vm),
            ))
        }
        HeapData::DictKeysView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(Some(
                dict.key_at(index).expect("index should be valid").clone_with_heap(vm),
            ))
        }
        HeapData::DictItemsView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            let (key, value) = dict.item_at(index).expect("index should be valid");
            Ok(Some(super::allocate_tuple(
                smallvec::smallvec![key.clone_with_heap(vm), value.clone_with_heap(vm)],
                vm.heap,
            )?))
        }
        HeapData::DictValuesView(view) => {
            let dict = view.dict(vm.heap);
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(Some(
                dict.value_at(index).expect("index should be valid").clone_with_heap(vm),
            ))
        }
        HeapData::Bytes(bytes) => Ok(Some(Value::Int(i64::from(bytes.as_slice()[index])))),
        HeapData::Set(set) => {
            // Check for set mutation
            if let Some(expected) = expected_len
                && set.len() != expected
            {
                return Err(ExcType::runtime_error_set_changed_size());
            }
            Ok(Some(
                set.storage()
                    .value_at(index)
                    .expect("index should be valid")
                    .clone_with_heap(vm),
            ))
        }
        HeapData::FrozenSet(frozenset) => Ok(Some(
            frozenset
                .storage()
                .value_at(index)
                .expect("index should be valid")
                .clone_with_heap(vm),
        )),
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
        return Err(ExcType::type_error_not_iterable(&iter_value.py_type_name(vm)));
    };

    let result = match vm.heap.read(*iter_id) {
        HeapReadOutput::Iter(mut iter) => iter.advance(vm)?,
        other => {
            let data_type = other.py_type(vm).name(vm.heap, vm.interns);
            return Err(ExcType::type_error(format!("'{data_type}' object is not an iterator")));
        }
    };

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
    /// Delegating iterator: drives another heap-resident iterator by id, so
    /// re-iterating an iterator (`list(iter(x))`) shares its position.
    ///
    /// The parent's `value` holds the same ref (so it is GC-traced) and the
    /// parent's `index` is unused.
    IterHeapRef { iter_id: HeapId },
    /// Iterating over a heap-allocated container (List, Tuple, NamedTuple, Dict, Bytes, Set, FrozenSet).
    ///
    /// - `len`: `None` for List (checked dynamically since lists can mutate during iteration),
    ///   `Some(n)` for other types (captured at construction for exhaustion checking).
    /// - `checks_mutation`: `true` for Dict/Set (raises RuntimeError if size changes),
    ///   `false` for other types.
    HeapRef {
        heap_id: HeapId,
        len: Option<usize>,
        checks_mutation: bool,
    },
}

impl IterValue {
    fn new(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> Option<Self> {
        match &value {
            Value::InternString(string_id) => Some(Self::from_str(vm.interns.get_str(*string_id))),
            Value::InternBytes(bytes_id) => Some(Self::from_intern_bytes(*bytes_id, vm.interns)),
            Value::Ref(heap_id) => Self::from_heap_data(*heap_id, vm.heap),
            _ => None,
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
    fn from_heap_data(heap_id: HeapId, heap: &Heap<impl ResourceTracker>) -> Option<Self> {
        match heap.get(heap_id) {
            // List: no captured len (checked dynamically), no mutation check
            HeapData::List(_) => Some(Self::HeapRef {
                heap_id,
                len: None,
                checks_mutation: false,
            }),
            // Tuple/NamedTuple/Bytes/FrozenSet: captured len, no mutation check
            HeapData::Tuple(tuple) => Some(Self::HeapRef {
                heap_id,
                len: Some(tuple.as_slice().len()),
                checks_mutation: false,
            }),
            HeapData::NamedTuple(namedtuple) => Some(Self::HeapRef {
                heap_id,
                len: Some(namedtuple.len()),
                checks_mutation: false,
            }),
            HeapData::Bytes(b) => Some(Self::HeapRef {
                heap_id,
                len: Some(b.len()),
                checks_mutation: false,
            }),
            HeapData::FrozenSet(frozenset) => Some(Self::HeapRef {
                heap_id,
                len: Some(frozenset.len()),
                checks_mutation: false,
            }),
            // Dict and dict views: captured len, WITH mutation check
            HeapData::Dict(dict) => Some(Self::HeapRef {
                heap_id,
                len: Some(dict.len()),
                checks_mutation: true,
            }),
            HeapData::DictKeysView(view) => Some(Self::HeapRef {
                heap_id,
                len: Some(view.dict(heap).len()),
                checks_mutation: true,
            }),
            HeapData::DictItemsView(view) => Some(Self::HeapRef {
                heap_id,
                len: Some(view.dict(heap).len()),
                checks_mutation: true,
            }),
            HeapData::DictValuesView(view) => Some(Self::HeapRef {
                heap_id,
                len: Some(view.dict(heap).len()),
                checks_mutation: true,
            }),
            HeapData::Set(set) => Some(Self::HeapRef {
                heap_id,
                len: Some(set.len()),
                checks_mutation: true,
            }),
            // String: copy content for iteration
            HeapData::Str(s) => Some(Self::from_str(s.as_str())),
            // Range: copy values for iteration
            HeapData::Range(range) => Some(Self::from_range(range)),
            // An iterator is its own iterator: delegate so consumers share its
            // position rather than restarting it.
            HeapData::Iter(_) => Some(Self::IterHeapRef { iter_id: heap_id }),
            // other types are not iterable
            _ => None,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{resource::NoLimitTracker, types::List};

    /// Builds a delegating iterator pointing at `target`.
    ///
    /// Constructed directly because Python cannot produce one: `iter()` returns
    /// an existing iterator unchanged, so chains only arise from a snapshot.
    fn delegating(target: HeapId) -> MontyIter {
        MontyIter {
            index: 0,
            iter_value: IterValue::IterHeapRef { iter_id: target },
            value: Value::None,
        }
    }

    /// Builds a terminal (non-delegating) iterator over three ints.
    fn terminal() -> MontyIter {
        MontyIter {
            index: 0,
            iter_value: IterValue::Range {
                next: 0,
                step: 1,
                len: 3,
            },
            value: Value::None,
        }
    }

    /// A well-formed chain resolves to the terminal iterator holding the state.
    #[test]
    fn resolve_delegate_walks_to_the_terminal_iterator() {
        let heap = Heap::new(16, NoLimitTracker);
        let end = heap.allocate(HeapData::Iter(terminal())).unwrap();
        let mid = heap.allocate(HeapData::Iter(delegating(end))).unwrap();
        let start = heap.allocate(HeapData::Iter(delegating(mid))).unwrap();
        assert_eq!(resolve_delegate(start, &heap).ok(), Some(end));
    }

    /// A link pointing at a live non-iterator must raise, not panic: the
    /// consuming site's `heap.read` would abort the process on it.
    #[test]
    fn resolve_delegate_rejects_a_non_iterator_target() {
        let heap = Heap::new(16, NoLimitTracker);
        let list = heap.allocate(HeapData::List(List::new(vec![]))).unwrap();
        let start = heap.allocate(HeapData::Iter(delegating(list))).unwrap();
        let err = resolve_delegate(start, &heap).expect_err("a non-iterator target must not resolve");
        assert!(matches!(err, DelegateError::NotAnIterator));
    }

    /// An over-long chain stops at the cap rather than walking forever, which
    /// is what a cyclic chain from a snapshot degrades to.
    #[test]
    fn resolve_delegate_caps_an_over_long_chain() {
        let heap = Heap::new(16, NoLimitTracker);
        let mut current = heap.allocate(HeapData::Iter(terminal())).unwrap();
        for _ in 0..=MAX_DELEGATION_DEPTH {
            current = heap.allocate(HeapData::Iter(delegating(current))).unwrap();
        }
        let err = resolve_delegate(current, &heap).expect_err("a chain past the cap must not resolve");
        assert!(matches!(err, DelegateError::TooDeep));
    }
}
