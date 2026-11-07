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
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{
        _heapq::{FUNCTIONS, append_item, field, heapify, item_at, replace_top},
        ModuleFunctions,
        bisect::less_than,
    },
    sorting::sort_values,
    types::{List, Module, PyTrait, allocate_tuple, iter::collect_owned_iterable},
    value::Value,
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

/// Converts `n` the way `range(n)` and a slice bound would: an integer or `__index__`.
///
/// Integers beyond `i64` clamp, since either end behaves the same for any real iterable.
fn count_arg(n: &Value, vm: &mut VM<'_>) -> RunResult<i64> {
    let Some(index) = n.py_index_impl(vm)? else {
        return Err(ExcType::type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            n.py_type_name(vm)
        )));
    };
    let result = match index.as_int(vm) {
        Ok(count) => Ok(count),
        Err(_) => vm
            .cmp_values(CmpOperator::Lt, &index, &Value::Int(0))
            .map(|negative| if negative { i64::MIN } else { i64::MAX }),
    };
    index.drop_with(vm);
    result
}

fn new_list(vm: &VM<'_>, items: Vec<Value>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
}

fn some_key(key: &Value) -> Option<&Value> {
    if matches!(key, Value::None) { None } else { Some(key) }
}

macro_rules! n_best_args {
    ($ty:ident, $name:literal) => {
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

/// `nlargest` / `nsmallest`, following the three strategies of `Lib/heapq.py`.
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
    let key_fn = some_key(key);
    let n = count_arg(n, vm)?;

    if n == 1 {
        return best_one(vm, iterable, key_fn, largest);
    }

    // Small inputs are cheaper to sort outright, as in CPython.
    if let Some(size) = iterable.py_len(vm)
        && i64::try_from(size).map_or(true, |size| n >= size)
    {
        let items: Vec<Value> = collect_owned_iterable(iterable.clone_with_heap(vm), vm)?;
        let mut items = items;
        let sorted = sort_values(&mut items, key_fn, largest, vm);
        if let Err(err) = sorted {
            items.drop_with(vm);
            return Err(err);
        }
        return Ok(new_list(vm, items));
    }

    let source = iterable.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(source, vm);
    let mut source = source.read(vm);
    let result = new_list(vm, Vec::new());
    defer_drop!(result, vm);
    let Value::Ref(result_id) = *result else {
        unreachable!("new_list returns a reference");
    };

    // Entries are `(elem, order)` or `(key, order, elem)`. `order` breaks ties so that
    // nsmallest keeps the earliest and nlargest the earliest of equal items, and the
    // elements themselves are never compared with one another.
    let sign: i64 = if largest { -1 } else { 1 };
    let entry = |vm: &mut VM<'_>, rank: Value, order: i64, extra: Option<Value>| -> Value {
        let mut items = vec![rank, Value::Int(order)];
        items.extend(extra);
        allocate_tuple(items.into(), vm.heap)
    };
    let mut taken = 0;
    while taken < n {
        let Some(elem) = source.py_next(vm)? else { break };
        if let Err(err) = vm.heap.tracker.check_time_every(usize::try_from(taken).unwrap_or(0)) {
            elem.drop_with(vm);
            return Err(err.into());
        }
        let (rank, extra) = split_elem(vm, key_fn, elem)?;
        let tuple = entry(vm, rank, sign * taken, extra);
        append_item(vm, result_id, tuple)?;
        taken += 1;
    }
    if taken == 0 {
        return Ok(new_list(vm, Vec::new()));
    }
    // nsmallest keeps a max-heap of the n smallest; nlargest a min-heap of the n largest.
    let max = !largest;
    heapify(vm, result_id, max)?;
    let top = top_rank(vm, result_id)?;
    defer_drop_mut!(top, vm);
    let mut order = sign.saturating_mul(n);
    let mut steps = 0;
    while let Some(elem) = source.py_next(vm)? {
        steps += 1;
        if let Err(err) = vm.heap.tracker.check_time_every(steps) {
            elem.drop_with(vm);
            return Err(err.into());
        }
        let (rank, extra) = split_elem(vm, key_fn, elem)?;
        let better = if largest {
            less_than(vm, top, &rank)
        } else {
            less_than(vm, &rank, top)
        };
        match better {
            Ok(true) => {
                let tuple = entry(vm, rank, order, extra);
                replace_top(vm, result_id, tuple, max)?.drop_with(vm);
                let next = top_rank(vm, result_id)?;
                mem::replace(top, next).drop_with(vm);
                order = order.saturating_add(sign);
            }
            Ok(false) => {
                rank.drop_with(vm);
                extra.drop_with(vm);
            }
            Err(err) => {
                rank.drop_with(vm);
                extra.drop_with(vm);
                return Err(err);
            }
        }
    }

    let mut entries = list_items(vm, result_id);
    let sorted = sort_values(&mut entries, None, largest, vm);
    let out = sorted.and_then(|()| {
        let element = if key_fn.is_some() { 2 } else { 0 };
        let mut out = Vec::with_capacity(entries.len());
        for tuple in &entries {
            match field(vm, tuple, element) {
                Ok(value) => out.push(value),
                Err(err) => {
                    out.drop_with(vm);
                    return Err(err);
                }
            }
        }
        Ok(out)
    });
    entries.drop_with(vm);
    Ok(new_list(vm, out?))
}

/// The value an element is ranked by: its key when there is one, else itself.
fn rank_of(vm: &mut VM<'_>, key_fn: Option<&Value>, elem: &Value) -> RunResult<Value> {
    match key_fn {
        Some(function) => {
            let argument = elem.clone_with_heap(vm);
            vm.evaluate_function(function, ArgValues::One(argument))
        }
        None => Ok(elem.clone_with_heap(vm)),
    }
}

/// Splits an owned element into its rank and, when a key is used, the element itself.
fn split_elem(vm: &mut VM<'_>, key_fn: Option<&Value>, elem: Value) -> RunResult<(Value, Option<Value>)> {
    match key_fn {
        None => Ok((elem, None)),
        Some(_) => match rank_of(vm, key_fn, &elem) {
            Ok(rank) => Ok((rank, Some(elem))),
            Err(err) => {
                elem.drop_with(vm);
                Err(err)
            }
        },
    }
}

/// The rank stored in the first entry of the heap list.
fn top_rank(vm: &mut VM<'_>, id: HeapId) -> RunResult<Value> {
    let first = item_at(vm, id, 0)?;
    let rank = field(vm, &first, 0);
    first.drop_with(vm);
    rank
}

/// Clones every item of a heap list.
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

/// `nlargest(1, ...)` / `nsmallest(1, ...)`: `max` / `min` of the items, as a list.
fn best_one(vm: &mut VM<'_>, iterable: &Value, key_fn: Option<&Value>, largest: bool) -> RunResult<Value> {
    let source = iterable.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(source, vm);
    let mut source = source.read(vm);
    // Holds `[item, rank]` of the winner so far, or nothing.
    let best: Vec<Value> = Vec::new();
    defer_drop_mut!(best, vm);
    let mut steps = 0;
    while let Some(item) = source.py_next(vm)? {
        steps += 1;
        if let Err(err) = vm.heap.tracker.check_time_every(steps) {
            item.drop_with(vm);
            return Err(err.into());
        }
        let rank = match rank_of(vm, key_fn, &item) {
            Ok(rank) => rank,
            Err(err) => {
                item.drop_with(vm);
                return Err(err);
            }
        };
        if best.is_empty() {
            best.push(item);
            best.push(rank);
            continue;
        }
        match candidate_wins(&best[1], &rank, !largest, vm) {
            Ok(true) => {
                mem::take(best).drop_with(vm);
                best.push(item);
                best.push(rank);
            }
            Ok(false) => {
                item.drop_with(vm);
                rank.drop_with(vm);
            }
            Err(err) => {
                item.drop_with(vm);
                rank.drop_with(vm);
                return Err(err);
            }
        }
    }
    let mut items = Vec::new();
    if let Some(rank) = best.pop() {
        rank.drop_with(vm);
        items.extend(best.pop());
    }
    Ok(new_list(vm, items))
}
