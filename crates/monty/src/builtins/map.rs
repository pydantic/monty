//! Implementation of the map() builtin function.

use crate::{
    PrintWriter,
    args::{ArgValues, KwargsValues},
    defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait},
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
pub fn builtin_map(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
    print_writer: &mut PrintWriter<'_>,
) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, heap);

    kwargs.not_supported_yet("map", heap)?;

    if positional.len() < 2 {
        return Err(SimpleException::new_msg(ExcType::TypeError, "map() must have at least two arguments.").into());
    }

    let function = positional.next().unwrap();

    // TODO: support user-defined functions here
    let builtin = match function {
        Value::Builtin(b) => b,
        not_supported => {
            let func_type = not_supported.py_type(heap);
            not_supported.drop_with_heap(heap);
            return Err(
                SimpleException::new_msg(ExcType::TypeError, format!("'{func_type}' object is not callable")).into(),
            );
        }
    };

    function.drop_with_heap(heap);

    let first_iterable = positional.next().expect("checked length above");
    let first_iter = MontyIter::new(first_iterable, heap, interns)?;
    defer_drop_mut!(first_iter, heap);

    let extra_iterators: Vec<MontyIter> = Vec::with_capacity(positional.len());
    defer_drop_mut!(extra_iterators, heap);

    for iterable in positional {
        extra_iterators.push(MontyIter::new(iterable, heap, interns)?);
    }

    let mut out = Vec::new();

    // map function over iterables until the shortest iter is exhausted
    match extra_iterators.as_mut_slice() {
        // map(f, iter)
        [] => {
            while let Some(item) = first_iter.for_next(heap, interns)? {
                let args = ArgValues::One(item);
                out.push(builtin.call(heap, args, interns, print_writer)?);
            }
        }
        // map(f, iter1, iter2)
        [single] => {
            while let Some(arg1) = first_iter.for_next(heap, interns)? {
                let Some(arg2) = single.for_next(heap, interns)? else {
                    arg1.drop_with_heap(heap);
                    break;
                };
                let args = ArgValues::Two(arg1, arg2);
                out.push(builtin.call(heap, args, interns, print_writer)?);
            }
        }
        // map(f, iter1, iter2, *iterables)
        multiple => 'outer: loop {
            let mut items = Vec::with_capacity(1 + multiple.len());

            for iter in std::iter::once(&mut *first_iter).chain(multiple.iter_mut()) {
                if let Some(item) = iter.for_next(heap, interns)? {
                    items.push(item);
                } else {
                    items.drop_with_heap(heap);
                    break 'outer;
                }
            }

            let args = ArgValues::ArgsKargs {
                args: items,
                kwargs: KwargsValues::Empty,
            };

            out.push(builtin.call(heap, args, interns, print_writer)?);
        },
    }

    let heap_id = heap.allocate(HeapData::List(List::new(out)))?;
    Ok(Value::Ref(heap_id))
}
