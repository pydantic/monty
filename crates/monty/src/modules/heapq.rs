//! `heapq`: `_heapq`'s functions plus native `nlargest` and `nsmallest`.
//!
//! CPython's `heapq.py` does `from _heapq import *`, so the heap functions here are the very
//! objects `_heapq` provides. `nlargest` and `nsmallest` follow `Lib/heapq.py` and stand in for the
//! pure-Python versions until that file is vendored.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::candidate_wins,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    expressions::CmpOperator,
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{
        _heapq::{FUNCTIONS, append_item, field, heapify, item_at, replace_top},
        ModuleFunctions,
        bisect::less_than,
    },
    sorting::sort_values,
    types::{List, Module, PyTrait, allocate_tuple, iter::collect_owned_iterable},
    value::{Value, ValueRead},
};

/// The functions only `heapq` provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum HeapqFunctions {
    Nlargest,
    Nsmallest,
}

/// Creates the `heapq` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Heapq, vm.interns);
    for (name, function) in FUNCTIONS {
        module.set_attr(
            name,
            Value::ModuleFunction(ModuleFunctions::UnderscoreHeapq(function)),
            vm,
        );
    }
    for (name, function) in [
        (StaticStrings::Nlargest, HeapqFunctions::Nlargest),
        (StaticStrings::Nsmallest, HeapqFunctions::Nsmallest),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Heapq(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `heapq` function.
pub(super) fn call(vm: &mut VM<'_>, function: HeapqFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        HeapqFunctions::Nlargest => n_best(vm, args, true),
        HeapqFunctions::Nsmallest => n_best(vm, args, false),
    }
}

/// `nlargest` / `nsmallest`, choosing among the three strategies of `Lib/heapq.py` in its order.
///
/// `n` is only compared until the heap strategy needs it as a count, so `nsmallest(1.0, data)` is `min` and
/// a non-numeric `n` raises the comparison's `TypeError`, as in CPython.
fn n_best(vm: &mut VM<'_>, args: ArgValues, largest: bool) -> RunResult<Value> {
    let (n, iterable, key) = if largest {
        let NlargestArgs { n, iterable, key } = NlargestArgs::from_args(args, vm)?;
        (n, iterable, key)
    } else {
        let NsmallestArgs { n, iterable, key } = NsmallestArgs::from_args(args, vm)?;
        (n, iterable, key)
    };
    defer_drop!(n, vm);
    defer_drop!(iterable, vm);
    defer_drop!(key, vm);
    let key_fn = (!matches!(key, Value::None)).then_some(key);
    if vm.cmp_values(CmpOperator::Eq, n, &Value::Int(1))? {
        best_one(iterable, key_fn, largest, vm)
    } else if let Some(size) = iterable.py_len(vm)
        && vm.cmp_values(
            CmpOperator::GtE,
            n,
            &Value::Int(i64::try_from(size).unwrap_or(i64::MAX)),
        )?
    {
        sorted_prefix(iterable, n, key_fn, largest, vm)
    } else {
        heap_select(iterable, n, key_fn, largest, vm)
    }
}

/// Declares the `FromArgs` struct of `nlargest` or `nsmallest`, so argument errors name the function called.
macro_rules! n_best_args {
    ($ty:ident, $name:literal) => {
        #[doc = concat!("The arguments of `heapq.", $name, "(n, iterable, key=None)`.")]
        #[derive(FromArgs)]
        #[from_args(name = $name)]
        struct $ty {
            n: Value,
            #[from_args(static_string = "IterableArg")]
            iterable: Value,
            #[from_args(default = Value::None)]
            key: Value,
        }
    };
}
n_best_args!(NlargestArgs, "nlargest");
n_best_args!(NsmallestArgs, "nsmallest");

/// `n == 1`: `max(iter(iterable), default=sentinel, key=key)` (or `min`), as a list of zero or one items.
fn best_one(iterable: &Value, key_fn: Option<&Value>, largest: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let source = iterable.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(source, vm);
    let mut source = source.read(vm);
    // The winner so far, split by `split_elem`.
    let best: Option<(Value, Option<Value>)> = None;
    defer_drop_mut!(best, vm);
    // `ValueRead::py_next` polls the time limit, so these loops need no check of their own.
    while let Some(elem) = source.py_next(vm)? {
        let mut candidate = DropGuard::new(split_elem(vm, key_fn, elem)?, vm);
        let ((rank, _), vm) = candidate.as_parts_mut();
        let wins = match &*best {
            Some((best_rank, _)) => candidate_wins(best_rank, rank, !largest, vm)?,
            None => true,
        };
        if wins {
            let (candidate, vm) = candidate.into_parts();
            best.replace(candidate).drop_with(vm);
        }
    }
    let winner = match best.take() {
        Some((key, Some(elem))) => {
            key.drop_with(vm);
            vec![elem]
        }
        Some((elem, None)) => vec![elem],
        None => Vec::new(),
    };
    Ok(list_value(winner, vm))
}

/// `sorted(iterable, key=key, reverse=largest)[:n]`, `heapq.py`'s path once `n` covers the whole iterable.
fn sorted_prefix(
    iterable: &Value,
    n: &Value,
    key_fn: Option<&Value>,
    largest: bool,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let items: Vec<Value> = collect_owned_iterable(iterable.clone_with_heap(vm), vm)?;
    let mut items = DropGuard::new(items, vm);
    let (values, vm) = items.as_parts_mut();
    sort_values(values, key_fn, largest, vm)?;
    let stop = slice_stop(n, values.len(), vm)?;
    let (mut values, vm) = items.into_parts();
    values.split_off(stop).drop_with(vm);
    Ok(list_value(values, vm))
}

/// Where `[:n]` stops on a sequence of `len` items: `n` must be an integer or have `__index__`, and a negative
/// one counts from the end.
fn slice_stop(n: &Value, len: usize, vm: &mut VM<'_>) -> RunResult<usize> {
    let Some(index) = n.py_index_impl(vm)? else {
        return Err(ExcType::type_error_slice_indices());
    };
    defer_drop!(index, vm);
    let stop = index.saturating_index(vm)?;
    let len_i64 = i64::try_from(len).unwrap_or(i64::MAX);
    let stop = if stop < 0 {
        stop.saturating_add(len_i64).max(0)
    } else {
        stop.min(len_i64)
    };
    Ok(usize::try_from(stop).expect("clamped to 0..=len"))
}

/// `heapq.py`'s heap strategy: the first `n` items become `(elem, order)` or `(key, order, elem)` entries, heapified
/// so the worst kept entry is on top, and each later item replaces it when it ranks better.
///
/// `order` breaks ties toward the earliest item, so the elements themselves are never compared with one another.
fn heap_select(
    iterable: &Value,
    n: &Value,
    key_fn: Option<&Value>,
    largest: bool,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let source = iterable.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(source, vm);
    // `zip(range(n), it)`: `range` reads `n` through `__index__`, after the iterator exists.
    let n = n.saturating_index(vm)?;
    let mut source = source.read(vm);
    let result = list_value(Vec::new(), vm);
    defer_drop!(result, vm);
    let Value::Ref(result_id) = *result else {
        unreachable!("list_value returns a reference");
    };
    let sign: i64 = if largest { -1 } else { 1 };
    let mut taken = 0;
    while taken < n
        && let Some(elem) = source.py_next(vm)?
    {
        let (rank, extra) = split_elem(vm, key_fn, elem)?;
        let entry = heap_entry(rank, sign * taken, extra, vm);
        append_item(vm, result_id, entry)?;
        taken += 1;
    }
    if taken > 0 {
        replace_worse(&mut source, result_id, n, key_fn, largest, vm)?;
    }
    sorted_elements(result_id, key_fn.is_some(), largest, vm)
}

/// The second loop of the heap strategy: each remaining item replaces the top entry when it ranks better.
///
/// `nsmallest` keeps a max-heap of the `n` smallest, and `nlargest` a min-heap of the `n` largest.
fn replace_worse<'h>(
    source: &mut ValueRead<'h, '_>,
    result_id: HeapId,
    n: i64,
    key_fn: Option<&Value>,
    largest: bool,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    let max = !largest;
    let sign: i64 = if largest { -1 } else { 1 };
    heapify(vm, result_id, max)?;
    let top = top_rank(vm, result_id)?;
    defer_drop_mut!(top, vm);
    let mut order = sign.saturating_mul(n);
    while let Some(elem) = source.py_next(vm)? {
        let mut candidate = DropGuard::new(split_elem(vm, key_fn, elem)?, vm);
        let ((rank, _), vm) = candidate.as_parts_mut();
        let better = if largest {
            less_than(vm, top, rank)?
        } else {
            less_than(vm, rank, top)?
        };
        if better {
            let ((rank, extra), vm) = candidate.into_parts();
            let entry = heap_entry(rank, order, extra, vm);
            replace_top(vm, result_id, entry, max)?.drop_with(vm);
            let next = top_rank(vm, result_id)?;
            mem::replace(top, next).drop_with(vm);
            order = order.saturating_add(sign);
        }
    }
    Ok(())
}

/// Sorts the kept entries and returns their elements: `result.sort()` then `[elem for (elem, order) in result]`.
fn sorted_elements(result_id: HeapId, keyed: bool, largest: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let entries = list_items(vm, result_id);
    defer_drop_mut!(entries, vm);
    sort_values(entries, None, largest, vm)?;
    let element = if keyed { 2 } else { 0 };
    let mut out = DropGuard::new(Vec::with_capacity(entries.len()), vm);
    let (values, vm) = out.as_parts_mut();
    for entry in entries.iter() {
        values.push(field(vm, entry, element)?);
    }
    let (values, vm) = out.into_parts();
    Ok(list_value(values, vm))
}

/// A heap entry: `(rank, order)` without a key, or `(key, order, elem)` with one.
fn heap_entry(rank: Value, order: i64, extra: Option<Value>, vm: &VM<'_>) -> Value {
    let mut items = vec![rank, Value::Int(order)];
    items.extend(extra);
    allocate_tuple(items.into(), vm.heap)
}

/// Splits an owned element into its rank and, when a key is used, the element itself.
fn split_elem(vm: &mut VM<'_>, key_fn: Option<&Value>, elem: Value) -> RunResult<(Value, Option<Value>)> {
    match key_fn {
        None => Ok((elem, None)),
        Some(function) => {
            let mut elem = DropGuard::new(elem, vm);
            let (value, vm) = elem.as_parts();
            let argument = value.clone_with_heap(vm);
            let rank = vm.evaluate_function(function, ArgValues::One(argument))?;
            Ok((rank, Some(elem.into_inner())))
        }
    }
}

/// The rank stored in the first entry of the heap list.
fn top_rank(vm: &mut VM<'_>, id: HeapId) -> RunResult<Value> {
    let first = item_at(vm, id, 0)?;
    defer_drop!(first, vm);
    field(vm, first, 0)
}

/// Clones every item of a heap list, so it can be sorted while comparisons run user code.
fn list_items(vm: &VM<'_>, id: HeapId) -> Vec<Value> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get(vm.heap)
        .as_slice()
        .iter()
        .map(|item| item.clone_with_heap(vm.heap))
        .collect()
}

/// Allocates `items` as a new `list`.
fn list_value(items: Vec<Value>, vm: &VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
}
