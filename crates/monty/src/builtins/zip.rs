//! Implementation of the zip() builtin function.

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{HeapData, HeapGuard},
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait, allocate_tuple, tuple::TupleVec},
    value::Value,
};

/// Implementation of the zip() builtin function.
///
/// Returns a list of tuples, where the i-th tuple contains the i-th element
/// from each of the argument iterables. Stops when the shortest iterable is exhausted.
/// When `strict=True`, raises `ValueError` if any iterable has a different length.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_zip(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, vm);

    let strict = extract_zip_strict(kwargs, vm)?;

    if positional.len() == 0 {
        // zip() with no arguments returns empty list
        let heap_id = vm.heap.allocate(HeapData::List(List::new(Vec::new())))?;
        return Ok(Value::Ref(heap_id));
    }

    // Create iterators for each iterable
    let iterators: Vec<MontyIter> = Vec::with_capacity(positional.len());
    defer_drop_mut!(iterators, vm);
    for iterable in positional {
        iterators.push(MontyIter::new(iterable, vm)?);
    }

    let mut result_guard = HeapGuard::new(Vec::new(), vm);
    let (result, vm) = result_guard.as_parts_mut();

    // Zip until shortest iterator is exhausted
    'outer: loop {
        let mut items_guard = HeapGuard::new(TupleVec::with_capacity(iterators.len()), vm);
        let (tuple_items, vm) = items_guard.as_parts_mut();

        for (i, iter) in iterators.iter_mut().enumerate() {
            if let Some(item) = iter.for_next(vm)? {
                tuple_items.push(item);
            } else {
                // This iterator is exhausted - stop zipping

                if strict {
                    // In strict mode, if i > 0 then argument i+1 ran out before
                    // the earlier ones, so it is "shorter."
                    if i > 0 {
                        return Err(strict_length_error(i + 1, i, "shorter"));
                    }
                    // i == 0: first iterator exhausted — verify every remaining
                    // iterator is also exhausted. If any still yields a value,
                    // that argument is "longer" than all preceding exhausted ones.
                    // j is the 0-based index; iterators 0..j are all exhausted,
                    // so j gives the count for the error message.
                    for (j, remaining) in iterators.iter_mut().enumerate().skip(1) {
                        if let Some(extra) = remaining.for_next(vm)? {
                            extra.drop_with_heap(vm);
                            return Err(strict_length_error(j + 1, j, "longer"));
                        }
                    }
                }

                break 'outer;
            }
        }

        // Create tuple from collected items
        let (tuple_items, vm) = items_guard.into_parts();
        let tuple_val = allocate_tuple(tuple_items, vm.heap)?;
        result.push(tuple_val);
    }

    let (result, vm) = result_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}

/// Extracts the `strict` keyword argument from `zip()`.
///
/// Accepts any truthy/falsy value for `strict`, matching CPython behavior.
/// Raises `TypeError` for unexpected keyword arguments.
fn extract_zip_strict(kwargs: KwargsValues, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<bool> {
    let mut strict = false;
    let mut error: Option<RunError> = None;

    for (key, value) in kwargs {
        defer_drop!(key, vm);
        defer_drop!(value, vm);

        if error.is_some() {
            continue;
        }

        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            error = Some(SimpleException::new_msg(ExcType::TypeError, "keywords must be strings").into());
            continue;
        };

        let key_str = keyword_name.as_str(vm.interns);
        match key_str {
            "strict" => {
                strict = value.py_bool(vm);
            }
            _ => {
                error = Some(ExcType::type_error_unexpected_keyword("zip", key_str));
            }
        }
    }

    if let Some(error) = error {
        Err(error)
    } else {
        Ok(strict)
    }
}

/// Builds the `ValueError` for `zip(strict=True)` when iterables have different lengths.
///
/// Matches CPython's error format:
/// - `"zip() argument 2 is shorter than argument 1"` (singular)
/// - `"zip() argument 4 is shorter than arguments 1-3"` (plural)
fn strict_length_error(exhausted_arg: usize, num_longer_args: usize, relation: &str) -> RunError {
    let others = if num_longer_args == 1 {
        "argument 1".to_owned()
    } else {
        format!("arguments 1-{num_longer_args}")
    };
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("zip() argument {exhausted_arg} is {relation} than {others}"),
    )
    .into()
}
