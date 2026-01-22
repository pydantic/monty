//! Implementation of the min() and max() builtin functions.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the min() builtin function.
///
/// Returns the smallest item in an iterable or the smallest of two or more arguments.
/// Supports two forms:
/// - `min(iterable)` - returns smallest item from iterable
/// - `min(arg1, arg2, ...)` - returns smallest of the arguments
pub fn builtin_min(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    builtin_min_max(heap, args, interns, true)
}

/// Implementation of the max() builtin function.
///
/// Returns the largest item in an iterable or the largest of two or more arguments.
/// Supports two forms:
/// - `max(iterable)` - returns largest item from iterable
/// - `max(arg1, arg2, ...)` - returns largest of the arguments
pub fn builtin_max(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    builtin_min_max(heap, args, interns, false)
}

/// Shared implementation for min() and max().
///
/// When `is_min` is true, returns the minimum; otherwise returns the maximum.
fn builtin_min_max(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
    is_min: bool,
) -> RunResult<Value> {
    let func_name = if is_min { "min" } else { "max" };
    let (mut positional, kwargs) = args.into_parts();

    // Check for unsupported kwargs (key, default not yet implemented)
    if !kwargs.is_empty() {
        for (k, v) in kwargs {
            k.drop_with_heap(heap);
            v.drop_with_heap(heap);
        }
        for v in positional {
            v.drop_with_heap(heap);
        }
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name}() does not support keyword arguments yet"),
        )
        .into());
    }

    match positional.len() {
        0 => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name}() expected at least 1 argument, got 0"),
        )
        .into()),
        1 => {
            // Single argument: iterate over it
            let iterable = positional.next().unwrap();
            let mut iter = MontyIter::new(iterable, heap, interns)?;

            let Some(mut result) = iter.for_next(heap, interns)? else {
                iter.drop_with_heap(heap);
                return Err(SimpleException::new_msg(
                    ExcType::ValueError,
                    format!("{func_name}() iterable argument is empty"),
                )
                .into());
            };

            while let Some(item) = iter.for_next(heap, interns)? {
                let should_replace = match result.py_cmp(&item, heap, interns) {
                    Some(Ordering::Greater) => is_min,
                    Some(Ordering::Less) => !is_min,
                    Some(Ordering::Equal) => false,
                    None => {
                        let result_type = result.py_type(heap);
                        let item_type = item.py_type(heap);
                        result.drop_with_heap(heap);
                        item.drop_with_heap(heap);
                        iter.drop_with_heap(heap);
                        return Err(SimpleException::new_msg(
                            ExcType::TypeError,
                            format!("'<' not supported between instances of '{result_type}' and '{item_type}'"),
                        )
                        .into());
                    }
                };

                if should_replace {
                    result.drop_with_heap(heap);
                    result = item;
                } else {
                    item.drop_with_heap(heap);
                }
            }

            iter.drop_with_heap(heap);
            Ok(result)
        }
        _ => {
            // Multiple arguments: compare them directly
            let mut result = positional.next().unwrap();

            for item in positional {
                let should_replace = match result.py_cmp(&item, heap, interns) {
                    Some(Ordering::Greater) => is_min,
                    Some(Ordering::Less) => !is_min,
                    Some(Ordering::Equal) => false,
                    None => {
                        let result_type = result.py_type(heap);
                        let item_type = item.py_type(heap);
                        result.drop_with_heap(heap);
                        item.drop_with_heap(heap);
                        return Err(SimpleException::new_msg(
                            ExcType::TypeError,
                            format!("'<' not supported between instances of '{result_type}' and '{item_type}'"),
                        )
                        .into());
                    }
                };

                if should_replace {
                    result.drop_with_heap(heap);
                    result = item;
                } else {
                    item.drop_with_heap(heap);
                }
            }

            Ok(result)
        }
    }
}
