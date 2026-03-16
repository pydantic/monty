//! Implementation of the min() and max() builtin functions.

use std::cmp::Ordering;

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapGuard},
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
pub fn builtin_min(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    builtin_min_max(vm, args, true)
}

/// Implementation of the max() builtin function.
///
/// Returns the largest item in an iterable or the largest of two or more arguments.
/// Supports two forms:
/// - `max(iterable)` - returns largest item from iterable
/// - `max(arg1, arg2, ...)` - returns largest of the arguments
pub fn builtin_max(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    builtin_min_max(vm, args, false)
}

/// Shared implementation for min() and max().
///
/// When `is_min` is true, returns the minimum; otherwise returns the maximum.
fn builtin_min_max(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues, is_min: bool) -> RunResult<Value> {
    let func_name = if is_min { "min" } else { "max" };
    let key_context = if is_min {
        "min() key argument"
    } else {
        "max() key argument"
    };
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, vm);
    let (key_fn, default_value) = parse_min_max_kwargs(kwargs, func_name, vm)?;
    defer_drop!(key_fn, vm);
    let mut default_guard = HeapGuard::new(default_value, vm);
    let (default_value, vm) = default_guard.as_parts_mut();

    let Some(first_arg) = positional.next() else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name} expected at least 1 argument, got 0"),
        )
        .into());
    };

    // decide what to do based on remaining arguments
    if positional.len() == 0 {
        // Single argument: iterate over it
        let iter = MontyIter::new(first_arg, vm)?;
        defer_drop_mut!(iter, vm);

        let Some(result) = iter.for_next(vm)? else {
            if let Some(default) = default_value.take() {
                return Ok(default);
            }
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("{func_name}() iterable argument is empty"),
            )
            .into());
        };

        if let Some(key_fn) = key_fn {
            let mut result_guard = HeapGuard::new(result, vm);
            {
                let (result, vm) = result_guard.as_parts_mut();
                let result_key = evaluate_key(result.clone_with_heap(vm), key_fn, key_context, vm)?;
                let mut result_key_guard = HeapGuard::new(result_key, vm);
                {
                    let (result_key, vm) = result_key_guard.as_parts_mut();

                    while let Some(item) = iter.for_next(vm)? {
                        defer_drop_mut!(item, vm);
                        let item_key = evaluate_key(item.clone_with_heap(vm), key_fn, key_context, vm)?;
                        defer_drop_mut!(item_key, vm);

                        if candidate_wins(result_key, item_key, is_min, vm)? {
                            std::mem::swap(result, item);
                            std::mem::swap(result_key, item_key);
                        }
                    }
                }

                let result_key = result_key_guard.into_inner();
                result_key.drop_with_heap(vm);
            }
            Ok(result_guard.into_inner())
        } else {
            let mut result_guard = HeapGuard::new(result, vm);
            let (result, vm) = result_guard.as_parts_mut();

            while let Some(item) = iter.for_next(vm)? {
                defer_drop_mut!(item, vm);

                if candidate_wins(result, item, is_min, vm)? {
                    std::mem::swap(result, item);
                }
            }

            Ok(result_guard.into_inner())
        }
    } else {
        // Multiple arguments: compare them directly
        if default_value.is_some() {
            first_arg.drop_with_heap(vm);
            return Err(default_with_multiple_args(func_name));
        }

        if let Some(key_fn) = key_fn {
            let mut result_guard = HeapGuard::new(first_arg, vm);
            {
                let (result, vm) = result_guard.as_parts_mut();
                let result_key = evaluate_key(result.clone_with_heap(vm), key_fn, key_context, vm)?;
                let mut result_key_guard = HeapGuard::new(result_key, vm);
                {
                    let (result_key, vm) = result_key_guard.as_parts_mut();

                    for item in positional {
                        defer_drop_mut!(item, vm);
                        let item_key = evaluate_key(item.clone_with_heap(vm), key_fn, key_context, vm)?;
                        defer_drop_mut!(item_key, vm);

                        if candidate_wins(result_key, item_key, is_min, vm)? {
                            std::mem::swap(result, item);
                            std::mem::swap(result_key, item_key);
                        }
                    }
                }

                let result_key = result_key_guard.into_inner();
                result_key.drop_with_heap(vm);
            }
            Ok(result_guard.into_inner())
        } else {
            let mut result_guard = HeapGuard::new(first_arg, vm);
            let (result, vm) = result_guard.as_parts_mut();

            for item in positional {
                defer_drop_mut!(item, vm);

                if candidate_wins(result, item, is_min, vm)? {
                    std::mem::swap(result, item);
                }
            }

            Ok(result_guard.into_inner())
        }
    }
}

/// Parses `key=` and `default=` for min()/max().
///
/// Returns `(key_fn, default_value)`. Passing `key=None` is normalized to `None`
/// so the comparison logic can treat it the same as omitting the keyword.
fn parse_min_max_kwargs(
    kwargs: KwargsValues,
    func_name: &str,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(Option<Value>, Option<Value>)> {
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, vm);

    let mut default_guard = HeapGuard::new(None::<Value>, vm);
    let (default_value, vm) = default_guard.as_parts_mut();
    let mut key_guard = HeapGuard::new(None::<Value>, vm);
    let (key_fn, vm) = key_guard.as_parts_mut();

    for (kw_key, value) in kwargs {
        defer_drop!(kw_key, vm);
        let mut value = HeapGuard::new(value, vm);

        let Some(keyword_name) = kw_key.as_either_str(value.heap().heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };

        let keyword_name = keyword_name.as_str(value.heap().interns);
        if keyword_name == "key" {
            if key_fn.is_some() {
                return Err(ExcType::type_error_multiple_values(func_name, keyword_name));
            }
            *key_fn = Some(value.into_inner());
        } else if keyword_name == "default" {
            if default_value.is_some() {
                return Err(ExcType::type_error_multiple_values(func_name, keyword_name));
            }
            *default_value = Some(value.into_inner());
        } else {
            return Err(ExcType::type_error_unexpected_keyword(func_name, keyword_name));
        }
    }

    let key_fn = match key_guard.into_inner() {
        Some(value) if matches!(value, Value::None) => {
            value.drop_with_heap(default_guard.heap());
            None
        }
        other => other,
    };

    Ok((key_fn, default_guard.into_inner()))
}

/// Calls the user-provided key function for a single candidate value.
///
/// The caller passes an owned clone of the candidate so this helper can forward it
/// into the function call without changing ownership of the original item being
/// tracked as the eventual min/max result.
fn evaluate_key(
    item: Value,
    key_fn: &Value,
    key_context: &'static str,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Value> {
    vm.evaluate_function(key_context, key_fn, ArgValues::One(item))
}

/// Returns whether `candidate` should replace `current` as the best value seen so far.
///
/// `min()` replaces the current winner when the new candidate compares smaller,
/// while `max()` replaces it when the new candidate compares larger. Equal values
/// keep the existing winner so ties preserve the first-seen item, matching CPython.
fn candidate_wins(
    current: &Value,
    candidate: &Value,
    is_min: bool,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<bool> {
    let Some(ordering) = current.py_cmp(candidate, vm)? else {
        return Err(ord_not_supported(current, candidate, vm.heap));
    };

    Ok((is_min && ordering == Ordering::Greater) || (!is_min && ordering == Ordering::Less))
}

/// Creates the CPython-compatible error for `default=` with multiple positional args.
#[cold]
fn default_with_multiple_args(func_name: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::TypeError,
        format!("Cannot specify a default for {func_name}() with multiple positional arguments"),
    )
    .into()
}

#[cold]
fn ord_not_supported(left: &Value, right: &Value, heap: &Heap<impl ResourceTracker>) -> RunError {
    let left_type = left.py_type(heap);
    let right_type = right.py_type(heap);
    ExcType::type_error(format!(
        "'<' not supported between instances of '{left_type}' and '{right_type}'"
    ))
}
