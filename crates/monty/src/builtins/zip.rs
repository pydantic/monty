//! Implementation of the zip() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{List, MontyIter, Tuple},
    value::Value,
};

/// Implementation of the zip() builtin function.
///
/// Returns a list of tuples, where the i-th tuple contains the i-th element
/// from each of the argument iterables. Stops when the shortest iterable is exhausted.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_zip(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();

    // Check for unsupported kwargs (strict not yet implemented)
    if !kwargs.is_empty() {
        for (k, v) in kwargs {
            k.drop_with_heap(heap);
            v.drop_with_heap(heap);
        }
        for v in positional {
            v.drop_with_heap(heap);
        }
        return Err(
            SimpleException::new_msg(ExcType::TypeError, "zip() does not support keyword arguments yet").into(),
        );
    }

    if positional.len() == 0 {
        // zip() with no arguments returns empty list
        let heap_id = heap.allocate(HeapData::List(List::new(Vec::new())))?;
        return Ok(Value::Ref(heap_id));
    }

    // Create iterators for each iterable
    let mut iterators: Vec<MontyIter> = Vec::with_capacity(positional.len());
    for iterable in positional {
        match MontyIter::new(iterable, heap, interns) {
            Ok(iter) => iterators.push(iter),
            Err(e) => {
                // Clean up already-created iterators
                for iter in iterators {
                    iter.drop_with_heap(heap);
                }
                return Err(e);
            }
        }
    }

    let mut result: Vec<Value> = Vec::new();

    // Zip until shortest iterator is exhausted
    'outer: loop {
        let mut tuple_items: Vec<Value> = Vec::with_capacity(iterators.len());

        for iter in &mut iterators {
            if let Some(item) = iter.for_next(heap, interns)? {
                tuple_items.push(item);
            } else {
                // This iterator is exhausted - drop partial tuple items and stop
                for item in tuple_items {
                    item.drop_with_heap(heap);
                }
                break 'outer;
            }
        }

        // Create tuple from collected items
        let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(tuple_items)))?;
        result.push(Value::Ref(tuple_id));
    }

    // Clean up iterators
    for iter in iterators {
        iter.drop_with_heap(heap);
    }

    let heap_id = heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}
