//! Implementation of the sorted() builtin function.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    for_iterator::ForIterator,
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{List, PyTrait},
    value::Value,
};

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable.
/// Note: Currently does not support key or reverse arguments.
pub fn builtin_sorted(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (positional, kwargs) = args.split();

    // Check for unsupported kwargs
    if !kwargs.is_empty() {
        for (k, v) in kwargs {
            k.drop_with_heap(heap);
            v.drop_with_heap(heap);
        }
        for v in positional {
            v.drop_with_heap(heap);
        }
        return exc_err_fmt!(ExcType::TypeError; "sorted() does not support keyword arguments yet");
    }

    let positional_len = positional.len();
    if positional_len != 1 {
        for v in positional {
            v.drop_with_heap(heap);
        }
        return exc_err_fmt!(ExcType::TypeError; "sorted expected 1 argument, got {positional_len}");
    }

    let iterable = positional.into_iter().next().unwrap();
    let mut iter = ForIterator::new(iterable, heap, interns)?;
    let mut items = iter.collect(heap, interns)?;
    iter.drop_with_heap(heap);

    // Sort using insertion sort (simple, stable, works with py_cmp)
    // For small lists this is fine; for large lists we'd want a better algorithm
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 {
            match items[j - 1].py_cmp(&items[j], heap, interns) {
                Some(Ordering::Greater) => {
                    items.swap(j - 1, j);
                    j -= 1;
                }
                Some(_) => break,
                None => {
                    let left_type = items[j - 1].py_type(Some(heap));
                    let right_type = items[j].py_type(Some(heap));
                    for item in items {
                        item.drop_with_heap(heap);
                    }
                    return exc_err_fmt!(ExcType::TypeError; "'<' not supported between instances of '{}' and '{}'", left_type, right_type);
                }
            }
        }
    }

    let heap_id = heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}
