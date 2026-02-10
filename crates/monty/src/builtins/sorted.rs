//! Implementation of the sorted() builtin function.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::{DepthGuard, ResourceTracker},
    types::{List, MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable.
/// Note: Currently does not support key or reverse arguments.
pub fn builtin_sorted(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, heap);

    kwargs.not_supported_yet("sorted", heap)?;

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
    let iter = MontyIter::new(iterable, heap, interns)?;
    let mut items: Vec<_> = iter.collect(heap, interns)?;

    // Sort using insertion sort (simple, stable, works with py_cmp)
    // For small lists this is fine; for large lists we'd want a better algorithm
    let mut guard = DepthGuard::default();
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 {
            match items[j - 1].py_cmp(&items[j], heap, &mut guard, interns)? {
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
