//! Implementation of the min() and max() builtin functions.

use std::{cmp::Ordering, mem};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::HeapGuard,
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
pub fn builtin_min(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let MinArgs { args, key, default } = MinArgs::from_args(args, vm)?;
    run_min_max(vm, args, key, default, true)
}

/// Implementation of the max() builtin function.
///
/// Returns the largest item in an iterable or the largest of two or more arguments.
/// Supports two forms:
/// - `max(iterable)` - returns largest item from iterable
/// - `max(arg1, arg2, ...)` - returns largest of the arguments
pub fn builtin_max(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let MaxArgs { args, key, default } = MaxArgs::from_args(args, vm)?;
    run_min_max(vm, args, key, default, false)
}

/// Shared implementation for min() and max() after argument extraction.
///
/// When `is_min` is true, returns the minimum; otherwise returns the maximum.
fn run_min_max(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: Vec<Value>,
    key: Value,
    default: Option<Value>,
    is_min: bool,
) -> RunResult<Value> {
    let func_name = if is_min { "min" } else { "max" };
    let key_context = if is_min {
        "min() key argument"
    } else {
        "max() key argument"
    };

    // Normalise `key=None` to "no key function" so the comparison path can
    // skip the call entirely.
    let key_fn = match key {
        Value::None => {
            key.drop_with_heap(vm);
            None
        }
        _ => Some(key),
    };
    defer_drop!(key_fn, vm);

    // `default_value` is `Option<Value>` — we may consume it on the "empty
    // iterable" path, or drop it on error paths. `HeapGuard` ensures cleanup
    // on every `?`-style early return.
    let mut default_guard = HeapGuard::new(default, vm);
    let (default_value, vm) = default_guard.as_parts_mut();

    // Wrap the remaining positional args in a guard so any unconsumed items
    // are released on early error returns (e.g. the user-supplied `key`
    // function raising mid-iteration).
    let mut args_guard = HeapGuard::new(args, vm);
    let (args, vm) = args_guard.as_parts_mut();

    if args.is_empty() {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name} expected at least 1 argument, got 0"),
        )
        .into());
    }

    let first_arg = args.remove(0);

    if args.is_empty() {
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
                            mem::swap(result, item);
                            mem::swap(result_key, item_key);
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
                    mem::swap(result, item);
                }
            }

            Ok(result_guard.into_inner())
        }
    } else {
        // Multiple arguments: compare them directly
        if default_value.is_some() {
            first_arg.drop_with_heap(vm);
            // `default_value` and `args` are owned by their respective guards
            // — their Drop impls release the held values when the function
            // returns.
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

                    for item in args.drain(..) {
                        defer_drop_mut!(item, vm);
                        let item_key = evaluate_key(item.clone_with_heap(vm), key_fn, key_context, vm)?;
                        defer_drop_mut!(item_key, vm);

                        if candidate_wins(result_key, item_key, is_min, vm)? {
                            mem::swap(result, item);
                            mem::swap(result_key, item_key);
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

            for item in args.drain(..) {
                defer_drop_mut!(item, vm);

                if candidate_wins(result, item, is_min, vm)? {
                    mem::swap(result, item);
                }
            }

            Ok(result_guard.into_inner())
        }
    }
}

/// Argument shape for `min(*args, key=None, default=...)`.
///
/// `key` is held as `Value` so `key=None` can be normalised to "no key
/// function". `default` is `Option<Value>` (with `default` attr) so the
/// implementation can distinguish "not provided" from "provided with any
/// value" — that distinction matters because `default=` is only valid with a
/// single iterable argument.
#[derive(FromArgs)]
#[from_args(name = "min")]
struct MinArgs {
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(default = Value::None)]
    key: Value,
    #[from_args(default)]
    default: Option<Value>,
}

/// Argument shape for `max(*args, key=None, default=...)`.
///
/// See [`MinArgs`] for field semantics; the only difference is the function
/// name used in error messages.
#[derive(FromArgs)]
#[from_args(name = "max")]
struct MaxArgs {
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(default = Value::None)]
    key: Value,
    #[from_args(default)]
    default: Option<Value>,
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
    vm: &mut VM<'_, impl ResourceTracker>,
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
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<bool> {
    let Some(ordering) = candidate.py_cmp(current, vm)? else {
        return Err(ord_not_supported(candidate, current, is_min, vm));
    };

    Ok((is_min && ordering == Ordering::Less) || (!is_min && ordering == Ordering::Greater))
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
fn ord_not_supported(left: &Value, right: &Value, is_min: bool, vm: &VM<'_, impl ResourceTracker>) -> RunError {
    let left_type = left.py_type(vm);
    let right_type = right.py_type(vm);
    let operator = if is_min { '<' } else { '>' };
    ExcType::type_error(format!(
        "'{operator}' not supported between instances of '{left_type}' and '{right_type}'"
    ))
}
