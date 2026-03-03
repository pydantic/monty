//! Implementation of the `collections` module.
//!
//! Provides a minimal implementation of Python's `collections` module with:
//! - `Counter([iterable-or-mapping], **kwargs)`: Count hashable values into a dict.
//!
//! This first implementation is intentionally minimal and optimized for common
//! counting workloads. It returns a plain dict while preserving Counter-style
//! constructor semantics for positional sources and keyword count updates.

use crate::{
    args::{ArgValues, KwargsValues},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Dict, Module, MontyIter, PyTrait, Type},
    value::Value,
};

/// Functions exported by the `collections` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum CollectionsFunctions {
    /// `collections.Counter`.
    Counter,
}

/// Creates the `collections` module and allocates it on the heap.
///
/// The module currently exports:
/// - `Counter`
///
/// # Returns
/// A `HeapId` pointing to the newly allocated module.
///
/// # Panics
/// Panics if required static strings were not pre-interned during prepare.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Collections);

    module.set_attr(
        StaticStrings::Counter,
        Value::ModuleFunction(ModuleFunctions::Collections(CollectionsFunctions::Counter)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Dispatches calls to `collections` module functions.
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
    function: CollectionsFunctions,
    args: ArgValues,
) -> RunResult<AttrCallResult> {
    match function {
        CollectionsFunctions::Counter => counter(heap, interns, args).map(AttrCallResult::Value),
    }
}

/// Implements `collections.Counter([iterable-or-mapping], **kwargs)`.
///
/// Behavior in this phase:
/// - Accepts at most one positional source.
/// - If the source is a dict, treats values as counts and adds them.
/// - Otherwise treats the source as an iterable of keys and increments by 1.
/// - Keyword arguments are treated as key=count pairs and added.
///
/// Returns a plain dict containing accumulated counts.
fn counter(heap: &mut Heap<impl ResourceTracker>, interns: &Interns, args: ArgValues) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, heap);
    let kwargs_len = kwargs.len();

    let source = positional.next();
    if positional.len() != 0 {
        if let Some(source) = source {
            source.drop_with_heap(heap);
        }
        return Err(ExcType::type_error_counter_init_too_many_args(positional.len() + 1));
    }

    let source_len_hint = source
        .as_ref()
        .map_or(0, |value| counter_source_len_hint(value, heap, interns));
    let mut counts = Dict::with_capacity(source_len_hint + kwargs_len);

    if let Some(source) = source {
        counter_update_from_source(&mut counts, source, heap, interns)?;
    }
    counter_update_from_kwargs(&mut counts, kwargs, heap, interns)?;

    let id = heap.allocate(HeapData::Dict(counts))?;
    Ok(Value::Ref(id))
}

/// Returns a best-effort capacity hint for Counter allocation.
///
/// For dict sources this returns exact entry count.
/// For other sources it uses `__len__` when available, otherwise 0.
fn counter_source_len_hint(source: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> usize {
    if let Value::Ref(id) = source
        && let HeapData::Dict(source_dict) = heap.get(*id)
    {
        return source_dict.len();
    }
    source.py_len(heap, interns).unwrap_or(0)
}

/// Updates Counter state from a positional source.
///
/// If the source is a dict, values are treated as counts.
/// Otherwise the source is treated as an iterable of keys with unit increments.
fn counter_update_from_source(
    counts: &mut Dict,
    source: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    let is_mapping = matches!(&source, Value::Ref(id) if matches!(heap.get(*id), HeapData::Dict(_)));

    if is_mapping {
        let Value::Ref(source_id) = source else {
            unreachable!("is_mapping implies source is a dict ref");
        };
        let HeapData::Dict(source_dict) = heap.get(source_id) else {
            unreachable!("is_mapping implies source heap data is dict");
        };

        // Clone pairs first so we can release the immutable borrow of `source_dict`
        // before mutating `counts` and heap state.
        let pairs: Vec<(Value, Value)> = source_dict
            .iter()
            .map(|(key, count)| (key.clone_with_heap(heap), count.clone_with_heap(heap)))
            .collect();

        let pairs_iter = pairs.into_iter();
        defer_drop_mut!(pairs_iter, heap);
        for (key, count) in pairs_iter {
            counter_add_count(counts, key, count, heap, interns)?;
        }

        // The dict source ref is no longer needed.
        source.drop_with_heap(heap);
        return Ok(());
    }

    let iter = MontyIter::new(source, heap, interns)?;
    defer_drop_mut!(iter, heap);
    while let Some(key) = iter.for_next(heap, interns)? {
        counter_increment(counts, key, heap, interns)?;
    }
    Ok(())
}

/// Updates Counter state from keyword arguments (`key=count` pairs).
fn counter_update_from_kwargs(
    counts: &mut Dict,
    kwargs: KwargsValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, heap);
    for (key, count) in kwargs_iter {
        counter_add_count(counts, key, count, heap, interns)?;
    }
    Ok(())
}

/// Increments a single key by 1 in the Counter output dict.
fn counter_increment(
    counts: &mut Dict,
    key: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    defer_drop!(key, heap);

    // Counter(iterable) uses CPython's generic unhashable message for keys.
    if key.py_hash(heap, interns)?.is_none() {
        return Err(ExcType::type_error_unhashable(key.py_type(heap)));
    }

    let next_count = match counts.get(key, heap, interns)? {
        Some(existing_count) => existing_count
            .py_add(&Value::Int(1), heap, interns)?
            .ok_or_else(|| ExcType::binary_type_error("+", existing_count.py_type(heap), Type::Int))?,
        None => Value::Int(1),
    };

    let key_clone = key.clone_with_heap(heap);
    if let Some(previous_count) = counts.set(key_clone, next_count, heap, interns)? {
        previous_count.drop_with_heap(heap);
    }

    Ok(())
}

/// Adds an arbitrary count value to a key in the Counter output dict.
///
/// If key exists, computes `existing + count`.
/// Otherwise inserts `count` directly.
fn counter_add_count(
    counts: &mut Dict,
    key: Value,
    count: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    defer_drop!(key, heap);
    defer_drop!(count, heap);

    let next_count = match counts.get(key, heap, interns)? {
        Some(existing_count) => existing_count
            .py_add(count, heap, interns)?
            .ok_or_else(|| ExcType::binary_type_error("+", existing_count.py_type(heap), count.py_type(heap)))?,
        None => count.clone_with_heap(heap),
    };

    let key_clone = key.clone_with_heap(heap);
    if let Some(previous_count) = counts.set(key_clone, next_count, heap, interns)? {
        previous_count.drop_with_heap(heap);
    }

    Ok(())
}
