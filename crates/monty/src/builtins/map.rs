//! Implementation of the map() builtin function.

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
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
pub fn builtin_map(vm: &mut VM<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, vm);

    kwargs.not_supported_yet("map", vm.heap)?;

    if positional.len() < 2 {
        return Err(SimpleException::new_msg(ExcType::TypeError, "map() must have at least two arguments.").into());
    }

    let function = positional.next().unwrap();
    defer_drop!(function, vm);

    let first_iterable = positional.next().expect("checked length above");
    let first_iter = MontyIter::new(first_iterable, vm.heap, vm.interns)?;
    defer_drop_mut!(first_iter, vm);

    let extra_iterators: Vec<MontyIter> = Vec::with_capacity(positional.len());
    defer_drop_mut!(extra_iterators, vm);

    for iterable in positional {
        extra_iterators.push(MontyIter::new(iterable, vm.heap, vm.interns)?);
    }

    let mut out = Vec::with_capacity(first_iter.size_hint(vm.heap));

    // map function over iterables until the shortest iter is exhausted
    match extra_iterators.as_mut_slice() {
        // map(f, iter)
        [] => {
            while let Some(item) = first_iter.for_next(vm.heap, vm.interns)? {
                let args = ArgValues::One(item);
                out.push(vm.evaluate_function("map()", function, args)?);
            }
        }
        // map(f, iter1, iter2)
        [single] => {
            while let Some(arg1) = first_iter.for_next(vm.heap, vm.interns)? {
                let Some(arg2) = single.for_next(vm.heap, vm.interns)? else {
                    arg1.drop_with_heap(vm.heap);
                    break;
                };
                let args = ArgValues::Two(arg1, arg2);
                out.push(vm.evaluate_function("map()", function, args)?);
            }
        }
        // map(f, iter1, iter2, *iterables)
        multiple => 'outer: loop {
            let mut items = Vec::with_capacity(1 + multiple.len());

            for iter in std::iter::once(&mut *first_iter).chain(multiple.iter_mut()) {
                if let Some(item) = iter.for_next(vm.heap, vm.interns)? {
                    items.push(item);
                } else {
                    items.drop_with_heap(vm.heap);
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
