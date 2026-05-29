//! Implementation of the map() builtin function.

use std::{iter, mem};

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData},
    resource::ResourceTracker,
    types::{List, MontyIter},
    value::Value,
};

/// Implementation of the map() builtin function.
///
/// Applies a function to every item of one or more iterables and returns a list of results.
/// With multiple iterables, stops when the shortest iterable is exhausted.
///
/// Note: In Python this returns an iterator, but we return a list for simplicity.
/// Note: The `strict=` parameter is not yet supported.
///
/// Examples:
/// ```python
/// map(abs, [-1, 0, 1, 2])           # [1, 0, 1, 2]
/// map(pow, [2, 3], [3, 2])          # [8, 9]
/// map(str, [1, 2, 3])               # ['1', '2', '3']
/// ```
pub fn builtin_map(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // CPython's map() uses a bespoke arity message
    // (`map() must have at least two arguments.`) rather than the generic
    // "missing N required positional arguments" wording the macro would
    // otherwise produce. Pre-check before delegating to MapArgs so we
    // match byte-for-byte; cleanup is handled by the early-return drop.
    //
    // Only fire the arity check when no kwargs are present — otherwise
    // `map(abs, bogus=1)` would report the arity error when CPython reports
    // the unknown-kwarg error. Delegating to the macro produces the
    // correct `got an unexpected keyword argument` message instead.
    let kwargs_empty = match &args {
        ArgValues::Kwargs(kwargs) => kwargs.is_empty(),
        ArgValues::ArgsKargs { kwargs, .. } => kwargs.is_empty(),
        _ => true,
    };
    if args.count() < 2 && kwargs_empty {
        args.drop_with_heap(vm.heap);
        return Err(ExcType::type_error_map_arity());
    }
    let MapArgs {
        function,
        first_iterable,
        extra_iterables,
    } = MapArgs::from_args(args, vm)?;
    defer_drop!(function, vm);

    let first_iter = MontyIter::new(first_iterable, vm)?;
    defer_drop_mut!(first_iter, vm);

    let extra_iterators: Vec<MontyIter> = Vec::with_capacity(extra_iterables.len());
    defer_drop_mut!(extra_iterators, vm);

    for iterable in extra_iterables {
        extra_iterators.push(MontyIter::new(iterable, vm)?);
    }

    // `preallocation_hint` validates the requested capacity against the
    // resource tracker and clamps it so an attacker-controlled iterable length
    // cannot drive an unbounded native pre-allocation.
    let mut out = Vec::with_capacity(first_iter.preallocation_hint(mem::size_of::<Value>(), vm)?);

    // map function over iterables until the shortest iter is exhausted
    match extra_iterators.as_mut_slice() {
        // map(f, iter)
        [] => {
            while let Some(item) = first_iter.for_next(vm)? {
                let args = ArgValues::One(item);
                out.push(vm.evaluate_function("map()", function, args)?);
            }
        }
        // map(f, iter1, iter2)
        [single] => {
            while let Some(arg1) = first_iter.for_next(vm)? {
                let Some(arg2) = single.for_next(vm)? else {
                    arg1.drop_with_heap(vm);
                    break;
                };
                let args = ArgValues::Two(arg1, arg2);
                out.push(vm.evaluate_function("map()", function, args)?);
            }
        }
        // map(f, iter1, iter2, *iterables)
        multiple => 'outer: loop {
            let mut items = Vec::with_capacity(1 + multiple.len());

            for iter in iter::once(&mut *first_iter).chain(multiple.iter_mut()) {
                if let Some(item) = iter.for_next(vm)? {
                    items.push(item);
                } else {
                    items.drop_with_heap(vm);
                    break 'outer;
                }
            }

            let args = ArgValues::ArgsKargs {
                args: items,
                kwargs: KwargsValues::Empty,
            };

            out.push(vm.evaluate_function("map()", function, args)?);
        },
    }

    let heap_id = vm.heap.allocate(HeapData::List(List::new(out)))?;
    Ok(Value::Ref(heap_id))
}

/// Argument shape for `map(function, iterable, *iterables)`.
///
/// `function` and the first `iterable` are required; any further iterables
/// are collected by `extra_iterables`. `map` doesn't accept kwargs, so the
/// macro's default unknown-kwarg error path is exactly what we want.
#[derive(FromArgs)]
#[from_args(name = "map")]
struct MapArgs {
    #[from_args(pos_only)]
    function: Value,
    #[from_args(pos_only)]
    first_iterable: Value,
    #[from_args(varargs)]
    extra_iterables: Vec<Value>,
}
