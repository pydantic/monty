//! Implementation of the min() and max() builtin functions.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapGuard},
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
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, heap);

    // TODO: support kwargs (key, default)
    kwargs.not_supported_yet(func_name, heap)?;

    let Some(first_arg) = positional.next() else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name}() expected at least 1 argument, got 0"),
        )
        .into());
    };

    // decide what to do based on remaining arguments
    if positional.len() == 0 {
        // Single argument: iterate over it
        let iter = MontyIter::new(first_arg, heap, interns)?;
        defer_drop_mut!(iter, heap);

        let Some(result) = iter.for_next(heap, interns)? else {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("{func_name}() iterable argument is empty"),
            )
            .into());
        };

        let mut result_guard = HeapGuard::new(result, heap);
        let (result, heap) = result_guard.as_parts_mut();

        while let Some(item) = iter.for_next(heap, interns)? {
            defer_drop_mut!(item, heap);

            let Some(ordering) = result.py_cmp(item, heap, interns)? else {
                return Err(ord_not_supported(result, item, heap));
            };

            if (is_min && ordering == Ordering::Greater) || (!is_min && ordering == Ordering::Less) {
                std::mem::swap(result, item);
            }
        }

        Ok(result_guard.into_inner())
    } else {
        // Multiple arguments: compare them directly
        let mut result_guard = HeapGuard::new(first_arg, heap);
        let (result, heap) = result_guard.as_parts_mut();

        for item in positional {
            defer_drop_mut!(item, heap);

            let Some(ordering) = result.py_cmp(item, heap, interns)? else {
                return Err(ord_not_supported(result, item, heap));
            };

            if (is_min && ordering == Ordering::Greater) || (!is_min && ordering == Ordering::Less) {
                std::mem::swap(result, item);
            }
        }

        Ok(result_guard.into_inner())
    }
}

#[cold]
fn ord_not_supported(left: &Value, right: &Value, heap: &Heap<impl ResourceTracker>) -> RunError {
    let left_type = left.py_type(heap);
    let right_type = right.py_type(heap);
    ExcType::type_error(format!(
        "'<' not supported between instances of '{left_type}' and '{right_type}'"
    ))
}
