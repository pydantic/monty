//! The native `_heapq` accelerator: binary heaps over lists, following CPython's `Modules/_heapqmodule.c`.
//!
//! The sift routines mirror the C source step for step, because the order of a heap's list is
//! observable. Every comparison goes through [`less_than`], which is what the interpreter does for
//! `<`, and no list borrow is held across one: a comparison may run user code that resizes the heap,
//! which CPython reports as `RuntimeError: list changed size during iteration`.

use std::mem;

use HeapqFunctions as F;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{ModuleFunctions, bisect::less_than},
    types::{Module, PyTrait, Type},
    value::Value,
};

/// Heaps longer than this are heapified in CPython's cache-friendly order.
const CACHE_FRIENDLY_HEAPIFY_THRESHOLD: usize = 2500;

/// Functions of the `_heapq` module.
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

/// The functions `_heapq` provides, which `heapq` re-exports as the same objects.
pub(super) const FUNCTIONS: [(StaticStrings, HeapqFunctions); 10] = [
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
];

/// Creates the `_heapq` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::UnderscoreHeapq, vm.interns);
    for (name, function) in FUNCTIONS {
        module.set_attr(
            name,
            Value::ModuleFunction(ModuleFunctions::UnderscoreHeapq(function)),
            vm,
        );
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `_heapq` function.
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
    }
}

/// Declares the `FromArgs` struct of a `(heap, item)` function, so arity and keyword errors name it as CPython does.
macro_rules! pair_args {
    ($ty:ident, $name:literal, $kw:literal) => {
        #[doc = concat!("The arguments of `", $kw, "(heap, item, /)`.")]
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
    match heap {
        Value::Ref(id) if matches!(heap.py_type(vm), Type::List) => Ok(*id),
        _ => {
            let name = function.to_string();
            let got = heap.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            Err(if numbered {
                ExcType::type_error_bad_arg_pos(&name, 1, "list", got)
            } else {
                ExcType::type_error_bad_arg(&name, "list", got)
            })
        }
    }
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

/// `heappush`: appends `item` and sifts it into place.
fn push(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<()> {
    append_item(vm, id, item)?;
    let len = list_len(vm, id);
    siftdown(vm, id, 0, len - 1, max)
}

/// `heappop`: removes the last item, puts it at the top and sifts it down, returning the old top.
pub(super) fn pop(vm: &mut VM<'_>, id: HeapId, max: bool) -> RunResult<Value> {
    let last = pop_last(vm, id).ok_or_else(ExcType::index_error_out_of_range)?;
    if list_len(vm, id) == 0 {
        Ok(last)
    } else {
        let mut returned = DropGuard::new(replace_item(vm, id, 0, last), vm);
        let (_, vm) = returned.as_parts_mut();
        siftup(vm, id, 0, max)?;
        Ok(returned.into_inner())
    }
}

/// `heapreplace`: puts `item` at the top and sifts it down, returning the old top.
pub(super) fn replace_top(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<Value> {
    if list_len(vm, id) == 0 {
        item.drop_with(vm);
        Err(ExcType::index_error_out_of_range())
    } else {
        let mut returned = DropGuard::new(replace_item(vm, id, 0, item), vm);
        let (_, vm) = returned.as_parts_mut();
        siftup(vm, id, 0, max)?;
        Ok(returned.into_inner())
    }
}

/// `heappushpop`: returns `item` unchanged when it would come straight back out, else swaps it in for the top.
fn push_pop(vm: &mut VM<'_>, id: HeapId, item: Value, max: bool) -> RunResult<Value> {
    if list_len(vm, id) == 0 {
        Ok(item)
    } else {
        let mut item = DropGuard::new(item, vm);
        let replace = {
            let (value, vm) = item.as_parts_mut();
            let top = item_at(vm, id, 0)?;
            defer_drop!(top, vm);
            // `heappushpop` replaces the top only when `top < item`; the max variant when `item < top`.
            if max {
                less_than(vm, value, top)?
            } else {
                less_than(vm, top, value)?
            }
        };
        let (item, vm) = item.into_parts();
        if replace {
            replace_top(vm, id, item, max)
        } else {
            Ok(item)
        }
    }
}

/// `heapify`: sifts every parent down, last first; long heaps use CPython's cache-friendly order.
///
/// It polls the time limit as `sorted` does, since comparing builtin values runs no bytecode.
pub(super) fn heapify(vm: &mut VM<'_>, id: HeapId, max: bool) -> RunResult<()> {
    let n = list_len(vm, id);
    if n <= CACHE_FRIENDLY_HEAPIFY_THRESHOLD {
        for i in (0..n >> 1).rev() {
            vm.heap.tracker.check_time_every(i)?;
            siftup(vm, id, i, max)?;
        }
        Ok(())
    } else {
        heapify_cache_friendly(vm, id, n, max)
    }
}

/// The same comparisons and the same heap as the simple loop, in CPython's cache-friendly order.
fn heapify_cache_friendly(vm: &mut VM<'_>, id: HeapId, n: usize, max: bool) -> RunResult<()> {
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

/// The highest power of two not above `n`, or 0 for 0.
fn keep_top_bit(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1 << (usize::BITS - 1 - n.leading_zeros())
    }
}

/// CPython's `siftdown`: moves the item at `pos` up towards `start` while it beats its parent.
///
/// A comparison may run user code, so the heap's length is checked again after each one.
fn siftdown(vm: &mut VM<'_>, id: HeapId, start: usize, mut pos: usize, max: bool) -> RunResult<()> {
    let size = list_len(vm, id);
    if pos >= size {
        return Err(ExcType::index_error_out_of_range());
    }
    let newitem = item_at(vm, id, pos)?;
    defer_drop!(newitem, vm);
    while pos > start {
        let parentpos = (pos - 1) >> 1;
        let parent = item_at(vm, id, parentpos)?;
        defer_drop!(parent, vm);
        let cmp = lt(vm, max, parent, newitem)?;
        if size != list_len(vm, id) {
            return Err(ExcType::runtime_error_list_changed_size());
        }
        if !cmp {
            break;
        }
        swap_items(vm, id, parentpos, pos);
        pos = parentpos;
    }
    Ok(())
}

/// CPython's `siftup`: moves the smaller child up into the hole at `pos` until a leaf, then sifts the
/// original item back down into place, which takes fewer comparisons than a direct descent.
fn siftup(vm: &mut VM<'_>, id: HeapId, mut pos: usize, max: bool) -> RunResult<()> {
    let endpos = list_len(vm, id);
    if pos >= endpos {
        return Err(ExcType::index_error_out_of_range());
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
                return Err(ExcType::runtime_error_list_changed_size());
            }
            childpos += usize::from(!cmp);
        }
        swap_items(vm, id, childpos, pos);
        pos = childpos;
    }
    siftdown(vm, id, startpos, pos, max)
}

/// Whether `child` should move above `parent`: `child < parent` for a min-heap, `parent < child` for a max-heap.
fn lt(vm: &mut VM<'_>, max: bool, parent: &Value, child: &Value) -> RunResult<bool> {
    if max {
        less_than(vm, parent, child)
    } else {
        less_than(vm, child, parent)
    }
}

/// The length of the heap list `id`.
pub(super) fn list_len(vm: &VM<'_>, id: HeapId) -> usize {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get(vm.heap).len()
}

/// Clones the item at `index`; the caller has just checked the length.
pub(super) fn item_at(vm: &mut VM<'_>, id: HeapId, index: usize) -> RunResult<Value> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.try_clone_item(index, vm)
        .ok_or_else(ExcType::index_error_out_of_range)
}

/// Swaps two items of the heap list in place, as the C sift loops do.
fn swap_items(vm: &mut VM<'_>, id: HeapId, a: usize, b: usize) {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get_mut(vm.heap).as_vec_mut().swap(a, b);
}

/// Stores `value` at `index` and returns the previous item, which the caller now owns.
pub(super) fn replace_item(vm: &mut VM<'_>, id: HeapId, index: usize, value: Value) -> Value {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    let list = list.get_mut(vm.heap);
    if matches!(value, Value::Ref(_)) {
        list.set_contains_refs();
    }
    mem::replace(&mut list.as_vec_mut()[index], value)
}

/// Removes and returns the last item of the heap list, if any.
fn pop_last(vm: &mut VM<'_>, id: HeapId) -> Option<Value> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.get_mut(vm.heap).as_vec_mut().pop()
}

/// Appends to the list, charging any buffer growth against the memory limit.
pub(super) fn append_item(vm: &mut VM<'_>, id: HeapId, item: Value) -> RunResult<()> {
    let HeapReadOutput::List(list) = vm.heap.read(id) else {
        unreachable!("heap was checked to be a list");
    };
    list.append(vm, item)
}

/// `value[index]`, used to read the fields of a heap entry tuple.
pub(super) fn field(vm: &mut VM<'_>, value: &Value, index: i64) -> RunResult<Value> {
    value.py_getitem(&Value::Int(index), vm)
}
