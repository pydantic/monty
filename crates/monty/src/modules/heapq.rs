//! Binary heaps over lists, following CPython's `_heapq` accelerator and `Lib/heapq.py`.
//!
//! The sift routines mirror `Modules/_heapqmodule.c` step for step, because the order
//! of a heap's list is observable. Every comparison goes through [`less_than`], which
//! is what the interpreter does for `<`, and no list borrow is held across one: a
//! comparison may run user code that resizes the heap, which CPython reports as
//! `RuntimeError: list changed size during iteration`.
//!
//! `merge()` is eager and returns a list, like `map()` and `zip()` in Monty.

use std::mem;

/// Heaps longer than this are heapified in CPython's cache-friendly order.
use HeapqFunctions as F;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::candidate_wins,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    expressions::CmpOperator,
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{ModuleFunctions, bisect::less_than},
    sorting::sort_values,
    types::{List, Module, PyTrait, Type, allocate_tuple, iter::collect_owned_iterable},
    value::Value,
};

const CACHE_FRIENDLY_HEAPIFY_THRESHOLD: usize = 2500;

/// Functions of the `heapq` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum HeapqFunctions {
    Heappush,
    Heappop,
    Heapify,
    Heapreplace,
    Heappushpop,
    HeappushMax,
    HeappopMax,
    HeapifyMax,
    HeapreplaceMax,
    HeappushpopMax,
    Nlargest,
    Nsmallest,
    Merge,
}

impl HeapqFunctions {
    /// Whether this is one of the max-heap variants.
    fn is_max(self) -> bool {
        matches!(
            self,
            Self::HeappushMax | Self::HeappopMax | Self::HeapifyMax | Self::HeapreplaceMax | Self::HeappushpopMax
        )
    }
}

/// Creates the `heapq` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Heapq, vm.interns);
    for (name, function) in [
        (StaticStrings::Heappush, HeapqFunctions::Heappush),
        (StaticStrings::Heappop, HeapqFunctions::Heappop),
        (StaticStrings::Heapify, HeapqFunctions::Heapify),
        (StaticStrings::Heapreplace, HeapqFunctions::Heapreplace),
        (StaticStrings::Heappushpop, HeapqFunctions::Heappushpop),
        (StaticStrings::HeappushMax, HeapqFunctions::HeappushMax),
        (StaticStrings::HeappopMax, HeapqFunctions::HeappopMax),
        (StaticStrings::HeapifyMax, HeapqFunctions::HeapifyMax),
        (StaticStrings::HeapreplaceMax, HeapqFunctions::HeapreplaceMax),
        (StaticStrings::HeappushpopMax, HeapqFunctions::HeappushpopMax),
        (StaticStrings::Nlargest, HeapqFunctions::Nlargest),
        (StaticStrings::Nsmallest, HeapqFunctions::Nsmallest),
        (StaticStrings::Merge, HeapqFunctions::Merge),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Heapq(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `heapq` function.
pub(super) fn call(vm: &mut VM<'_>, function: HeapqFunctions, args: ArgValues) -> RunResult<Value> {
    let max = function.is_max();
    match function {
        F::Heappush | F::HeappushMax => {
            let (heap, item) = pair_args(function, args, vm)?;
            defer_drop!(heap, vm);
            let (id, item) = list_or_drop(heap, item, vm, function)?;
            push(vm, id, item, max)?;
            Ok(Value::None)
        }
        F::Heapreplace | F::HeapreplaceMax => {
            let (heap, item) = pair_args(function, args, vm)?;
            defer_drop!(heap, vm);
            let (id, item) = list_or_drop(heap, item, vm, function)?;
            replace_top(vm, id, item, max)
        }
        F::Heappushpop | F::HeappushpopMax => {
            let (heap, item) = pair_args(function, args, vm)?;
            defer_drop!(heap, vm);
            let (id, item) = list_or_drop(heap, item, vm, function)?;
            push_pop(vm, id, item, max)
        }
        F::Heappop | F::HeappopMax => {
            let heap = single_arg(function, args, vm)?;
            defer_drop!(heap, vm);
            let id = heap_list(heap, vm, function, false)?;
            pop(vm, id, max)
        }
        F::Heapify | F::HeapifyMax => {
            let heap = single_arg(function, args, vm)?;
            defer_drop!(heap, vm);
            let id = heap_list(heap, vm, function, false)?;
            heapify(vm, id, max)?;
            Ok(Value::None)
        }
        F::Nlargest => n_best(vm, args, true),
        F::Nsmallest => n_best(vm, args, false),
        F::Merge => merge(vm, args),
    }
}

// Per-function derives keep each name in CPython's arity and keyword errors.
macro_rules! pair_args {
    ($ty:ident, $name:literal, $kw:literal) => {
        #[derive(FromArgs)]
        #[from_args(name = $name, style = unpack, kwarg_error_name = $kw)]
        struct $ty {
            #[from_args(pos_only)]
            heap: Value,
            #[from_args(pos_only)]
            item: Value,
        }
    };
}
pair_args!(HeappushArgs, "heappush", "_heapq.heappush");
pair_args!(HeapreplaceArgs, "heapreplace", "_heapq.heapreplace");
pair_args!(HeappushpopArgs, "heappushpop", "_heapq.heappushpop");
pair_args!(HeappushMaxArgs, "heappush_max", "_heapq.heappush_max");
pair_args!(HeapreplaceMaxArgs, "heapreplace_max", "_heapq.heapreplace_max");
pair_args!(HeappushpopMaxArgs, "heappushpop_max", "_heapq.heappushpop_max");

/// Binds the `(heap, item)` arguments of the two-argument functions.
fn pair_args(function: HeapqFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<(Value, Value)> {
    Ok(match function {
        F::Heappush => {
            let HeappushArgs { heap, item } = HeappushArgs::from_args(args, vm)?;
            (heap, item)
        }
        F::Heapreplace => {
            let HeapreplaceArgs { heap, item } = HeapreplaceArgs::from_args(args, vm)?;
            (heap, item)
        }
        F::Heappushpop => {
            let HeappushpopArgs { heap, item } = HeappushpopArgs::from_args(args, vm)?;
            (heap, item)
        }
        F::HeappushMax => {
            let HeappushMaxArgs { heap, item } = HeappushMaxArgs::from_args(args, vm)?;
            (heap, item)
        }
        F::HeapreplaceMax => {
            let HeapreplaceMaxArgs { heap, item } = HeapreplaceMaxArgs::from_args(args, vm)?;
            (heap, item)
        }
        F::HeappushpopMax => {
            let HeappushpopMaxArgs { heap, item } = HeappushpopMaxArgs::from_args(args, vm)?;
            (heap, item)
        }
        _ => unreachable!("not a two-argument heap function"),
    })
}

/// Binds the single `heap` argument of `heappop` and `heapify`, which are `METH_O` in CPython.
fn single_arg(function: HeapqFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let name = format!("_heapq.{function}");
    let mut positional = args.into_pos_only(&name, vm.heap)?;
    let count = positional.len();
    if count != 1 {
        positional.drop_with(vm);
        return Err(ExcType::type_error_arg_count(&name, 1, count));
    }
    Ok(positional.next().expect("exactly one positional argument"))
}

/// Returns the heap's list id, or the clinic-style `TypeError` for anything else.
///
/// `numbered` selects the `argument 1` wording of the two-argument functions.
fn heap_list(heap: &Value, vm: &VM<'_>, function: HeapqFunctions, numbered: bool) -> RunResult<HeapId> {
    if let Value::Ref(id) = heap
        && matches!(heap.py_type(vm), Type::List)
    {
        return Ok(*id);
    }
    Err(ExcType::type_error(format!(
        "{function}() argument {}must be list, not {}",
        if numbered { "1 " } else { "" },
        // CPython names the singleton rather than its type.
        if matches!(heap, Value::None) {
            "None".into()
        } else {
            heap.py_type_name(vm).to_string()
        }
    )))
}

/// [`heap_list`] for the two-argument functions, releasing `item` when the heap is rejected.
fn list_or_drop(heap: &Value, item: Value, vm: &mut VM<'_>, function: HeapqFunctions) -> RunResult<(HeapId, Value)> {
    match heap_list(heap, vm, function, true) {
        Ok(id) => Ok((id, item)),
        Err(err) => {
            item.drop_with(vm);
            Err(err)
        }
    }
}

fn index_out_of_range() -> RunError {
    ExcType::index_error("index out of range")
}

fn changed_size() -> RunError {
    SimpleException::new_msg(ExcType::RuntimeError, "list changed size during iteration").into()
}

fn list_len(vm: &VM<'_>, id: HeapId) -> usize {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get(vm.heap).len()
}

/// Clones the item at `index`; the caller has just checked the length.
fn item_at(vm: &mut VM<'_>, id: HeapId, index: usize) -> RunResult<Value> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.try_clone_item(index, vm).ok_or_else(index_out_of_range)
}

fn swap_items(vm: &mut VM<'_>, id: HeapId, a: usize, b: usize) {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get_mut(vm.heap).as_vec_mut().swap(a, b);
}

/// Stores `value` at `index` and returns the previous item, which the caller now owns.
fn replace_item(vm: &mut VM<'_>, id: HeapId, index: usize, value: Value) -> Value {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    let list = list.get_mut(vm.heap);
    if matches!(value, Value::Ref(_)) {
        list.set_contains_refs();
    }
    mem::replace(&mut list.as_vec_mut()[index], value)
}

fn pop_last(vm: &mut VM<'_>, id: HeapId) -> Option<Value> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get_mut(vm.heap).as_vec_mut().pop()
}

/// Appends to the list, charging any buffer growth against the memory limit.
fn append_item(vm: &mut VM<'_>, id: HeapId, item: Value) -> RunResult<()> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.append(vm, item)
}

fn field(vm: &mut VM<'_>, value: &Value, index: i64) -> RunResult<Value> {
    value.py_getitem(&Value::Int(index), vm)
}

fn lt(vm: &mut VM<'_>, max: bool, parent: &Value, child: &Value) -> RunResult<bool> {
    // Min-heaps sift while the child is smaller; max-heaps while the parent is.
    if max {
        less_than(vm, parent, child)
    } else {
        less_than(vm, child, parent)
    }
}

fn siftdown(vm: &mut VM<'_>, id: HeapId, start: usize, mut pos: usize, max: bool) -> RunResult<()> {
    let size = list_len(vm, id);
    if pos >= size {
        return Err(index_out_of_range());
    }
    let newitem = item_at(vm, id, pos)?;
    defer_drop!(newitem, vm);
    while pos > start {
        let parentpos = (pos - 1) >> 1;
        let parent = item_at(vm, id, parentpos)?;
        defer_drop!(parent, vm);
        let cmp = lt(vm, max, parent, newitem)?;
        if size != list_len(vm, id) {
            return Err(changed_size());
        }
        if !cmp {
            break;
        }
        swap_items(vm, id, parentpos, pos);
        pos = parentpos;
    }
    Ok(())
}

fn siftup(vm: &mut VM<'_>, id: HeapId, mut pos: usize, max: bool) -> RunResult<()> {
    let endpos = list_len(vm, id);
    if pos >= endpos {
        return Err(index_out_of_range());
    }
    let startpos = pos;
    let limit = endpos >> 1;
    while pos < limit {
        let mut childpos = 2 * pos + 1;
        if childpos + 1 < endpos {
            let left = item_at(vm, id, childpos)?;
            defer_drop!(left, vm);
            let right = item_at(vm, id, childpos + 1)?;
            defer_drop!(right, vm);
            // `lt(max, parent, child)` is `child < parent` for a min-heap.
            let cmp = if max {
                less_than(vm, right, left)?
            } else {
                less_than(vm, left, right)?
            };
            if endpos != list_len(vm, id) {
                return Err(changed_size());
            }
            childpos += usize::from(!cmp);
        }
        swap_items(vm, id, childpos, pos);
        pos = childpos;
    }
    siftdown(vm, id, startpos, pos, max)
}

fn push(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<()> {
    append_item(vm, id, item)?;
    let len = list_len(vm, id);
    siftdown(vm, id, 0, len - 1, max)
}

fn pop(vm: &mut VM<'_>, id: HeapId, max: bool) -> RunResult<Value> {
    let Some(last) = pop_last(vm, id) else {
        return Err(index_out_of_range());
    };
    if list_len(vm, id) == 0 {
        return Ok(last);
    }
    let returned = replace_item(vm, id, 0, last);
    if let Err(err) = siftup(vm, id, 0, max) {
        returned.drop_with(vm);
        return Err(err);
    }
    Ok(returned)
}

fn replace_top(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<Value> {
    if list_len(vm, id) == 0 {
        item.drop_with(vm);
        return Err(index_out_of_range());
    }
    let returned = replace_item(vm, id, 0, item);
    if let Err(err) = siftup(vm, id, 0, max) {
        returned.drop_with(vm);
        return Err(err);
    }
    Ok(returned)
}

fn push_pop(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<Value> {
    if list_len(vm, id) == 0 {
        return Ok(item);
    }
    let top = match item_at(vm, id, 0) {
        Ok(top) => top,
        Err(err) => {
            item.drop_with(vm);
            return Err(err);
        }
    };
    // `heappushpop` replaces the top only when `top < item`; the max variant when `item < top`.
    let cmp = if max {
        less_than(vm, &item, &top)
    } else {
        less_than(vm, &top, &item)
    };
    top.drop_with(vm);
    match cmp {
        Ok(true) => replace_top(vm, id, item, max),
        Ok(false) => Ok(item),
        Err(err) => {
            item.drop_with(vm);
            Err(err)
        }
    }
}

/// The highest power of two not above `n`, or 0 for 0.
fn keep_top_bit(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1 << (usize::BITS - 1 - n.leading_zeros())
    }
}

fn heapify(vm: &mut VM<'_>, id: HeapId, max: bool) -> RunResult<()> {
    let n = list_len(vm, id);
    if n <= CACHE_FRIENDLY_HEAPIFY_THRESHOLD {
        for i in (0..n >> 1).rev() {
            vm.heap.tracker.check_time_every(i)?;
            siftup(vm, id, i, max)?;
        }
        return Ok(());
    }
    // Same comparisons and same heap as the simple loop, in CPython's cache-friendly order.
    let m = n >> 1;
    let leftmost = keep_top_bit(m + 1) - 1;
    let mhalf = m >> 1;
    for i in (mhalf..leftmost).rev().chain((leftmost..m).rev()) {
        vm.heap.tracker.check_time_every(i)?;
        let mut j = i;
        loop {
            siftup(vm, id, j, max)?;
            if j & 1 == 0 {
                break;
            }
            j >>= 1;
        }
    }
    Ok(())
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

#[derive(FromArgs)]
#[from_args(name = "merge")]
struct MergeArgs {
    #[from_args(varargs)]
    iterables: Vec<Value>,
    #[from_args(kw_only, default = Value::None)]
    key: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    reverse: Value,
}

/// `merge(*iterables, key=None, reverse=False)`, run to completion and returned as a list.
///
/// Heap entries are `[rank, order * direction]` plus `value` when a key is given. CPython
/// also keeps the iterator's `next` in the entry; here `abs(order)` indexes `iterators`.
fn merge(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let MergeArgs {
        iterables,
        key,
        reverse,
    } = MergeArgs::from_args(args, vm)?;
    defer_drop_mut!(iterables, vm);
    defer_drop!(key, vm);
    defer_drop!(reverse, vm);
    let max = reverse.py_bool(vm)?;
    let direction: i64 = if max { -1 } else { 1 };
    let key_fn = some_key(key);

    let iterators: Vec<Value> = Vec::with_capacity(iterables.len());
    defer_drop_mut!(iterators, vm);
    let heap = new_list(vm, Vec::new());
    defer_drop!(heap, vm);
    let Value::Ref(heap_id) = *heap else {
        unreachable!("new_list returns a reference");
    };
    let out = new_list(vm, Vec::new());
    defer_drop!(out, vm);
    let Value::Ref(out_id) = *out else {
        unreachable!("new_list returns a reference");
    };

    for (order, iterable) in iterables.drain(..).enumerate() {
        iterators.push(iterable.into_py_iter(vm)?.require_synchronous(vm)?);
        let Some(value) = iterators[order].py_next(vm)? else {
            continue;
        };
        let order = i64::try_from(order).expect("argument count fits i64") * direction;
        let entry = match key_fn {
            Some(function) => {
                let argument = value.clone_with_heap(vm);
                match vm.evaluate_function(function, ArgValues::One(argument)) {
                    Ok(rank) => vec![rank, Value::Int(order), value],
                    Err(err) => {
                        value.drop_with(vm);
                        return Err(err);
                    }
                }
            }
            None => vec![value, Value::Int(order)],
        };
        append_item(vm, heap_id, new_list(vm, entry))?;
    }
    heapify(vm, heap_id, max)?;

    let value_index = if key_fn.is_some() { 2 } else { 0 };
    let mut steps = 0;
    while list_len(vm, heap_id) > 1 {
        loop {
            vm.heap.tracker.check_time_every(steps)?;
            steps += 1;
            let top = item_at(vm, heap_id, 0)?;
            defer_drop!(top, vm);
            let Value::Ref(top_id) = *top else {
                unreachable!("heap entries are lists");
            };
            let value = field(vm, top, value_index)?;
            append_item(vm, out_id, value)?;
            let order = field(vm, top, 1)?;
            let source = usize::try_from(order.as_int(vm)?.unsigned_abs()).expect("order indexes the iterators");
            let Some(next) = iterators[source].py_next(vm)? else {
                pop(vm, heap_id, max)?.drop_with(vm);
                break;
            };
            match key_fn {
                Some(function) => {
                    let argument = next.clone_with_heap(vm);
                    let rank = match vm.evaluate_function(function, ArgValues::One(argument)) {
                        Ok(rank) => rank,
                        Err(err) => {
                            next.drop_with(vm);
                            return Err(err);
                        }
                    };
                    replace_item(vm, top_id, 0, rank).drop_with(vm);
                    replace_item(vm, top_id, 2, next).drop_with(vm);
                }
                None => replace_item(vm, top_id, 0, next).drop_with(vm),
            }
            replace_top(vm, heap_id, top.clone_with_heap(vm), max)?.drop_with(vm);
        }
    }
    if list_len(vm, heap_id) > 0 {
        let top = item_at(vm, heap_id, 0)?;
        defer_drop!(top, vm);
        let value = field(vm, top, value_index)?;
        append_item(vm, out_id, value)?;
        let order = field(vm, top, 1)?;
        let source = usize::try_from(order.as_int(vm)?.unsigned_abs()).expect("order indexes the iterators");
        while let Some(next) = iterators[source].py_next(vm)? {
            append_item(vm, out_id, next)?;
        }
    }
    Ok(out.clone_with_heap(vm))
}
