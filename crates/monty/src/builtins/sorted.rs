//! Implementation of the sorted() builtin function.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable.
/// Note: Currently does not support key or reverse arguments.
pub fn builtin_sorted(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (mut positional, kwargs) = args.into_parts();

    // Check for unsupported kwargs
    if !kwargs.is_empty() {
        for (k, v) in kwargs {
            k.drop_with_heap(heap);
            v.drop_with_heap(heap);
        }
        for v in positional {
            v.drop_with_heap(heap);
        }
        return Err(
            SimpleException::new_msg(ExcType::TypeError, "sorted() does not support keyword arguments yet").into(),
        );
    }

    let positional_len = positional.len();
    if positional_len != 1 {
        for v in positional {
            v.drop_with_heap(heap);
        }
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("sorted expected 1 argument, got {positional_len}"),
        )
        .into());
    }

    let iterable = positional.next().unwrap();
    let mut iter = MontyIter::new(iterable, heap, interns)?;
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
                    let left_type = items[j - 1].py_type(heap);
                    let right_type = items[j].py_type(heap);
                    for item in items {
                        item.drop_with_heap(heap);
                    }
                    return Err(SimpleException::new_msg(
                        ExcType::TypeError,
                        format!("'<' not supported between instances of '{left_type}' and '{right_type}'"),
                    )
                    .into());
                }
            }
        }
    }

    let heap_id = heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}
