//! Implementation of the min() and max() builtin functions.

use std::{cmp::Ordering, mem};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    owned::{Owned, OwnedIntoIter},
    types::{CmpOrder, PyTrait},
    value::Value,
};

/// Implementation of the min() builtin function.
///
/// Returns the smallest item in an iterable or the smallest of two or more arguments.
/// Supports two forms:
/// - `min(iterable)` - returns smallest item from iterable
/// - `min(arg1, arg2, ...)` - returns smallest of the arguments
pub fn builtin_min(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let MinArgs { args, key, default } = MinArgs::from_args(args, vm)?;
    run_min_max(vm, args, key, default, true)
}

/// Implementation of the max() builtin function.
///
/// Returns the largest item in an iterable or the largest of two or more arguments.
/// Supports two forms:
/// - `max(iterable)` - returns largest item from iterable
/// - `max(arg1, arg2, ...)` - returns largest of the arguments
pub fn builtin_max(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let MaxArgs { args, key, default } = MaxArgs::from_args(args, vm)?;
    run_min_max(vm, args, key, default, false)
}

/// Shared implementation for min() and max() after argument extraction.
///
/// When `is_min` is true, returns the minimum; otherwise returns the maximum.
/// Values are held as [`Owned`], so every early return releases them without guards.
fn run_min_max(
    vm: &mut VM<'_>,
    mut args: OwnedIntoIter,
    key: Owned,
    default: Option<Owned>,
    is_min: bool,
) -> RunResult<Value> {
    let func_name = if is_min { "min" } else { "max" };

    // Normalise `key=None` to "no key function" so the comparison path can
    // skip the call entirely.
    let key_fn = (!matches!(*key, Value::None)).then_some(key);

    let Some(first_arg) = args.next() else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{func_name} expected at least 1 argument, got 0"),
        )
        .into());
    };

    if args.len() == 0 {
        // Single argument: iterate over it
        let iter = first_arg.into_value().into_py_iter(vm)?.require_synchronous(vm)?;
        defer_drop!(iter, vm);
        let mut iter = iter.read(vm);

        let Some(first) = iter.py_next(vm)? else {
            return match default {
                Some(default) => Ok(default.into_value()),
                None => Err(SimpleException::new_msg(
                    ExcType::ValueError,
                    format!("{func_name}() iterable argument is empty"),
                )
                .into()),
            };
        };
        let first = Owned::new(first);
        select_best(first, key_fn.as_ref(), is_min, vm, |vm| {
            Ok(iter.py_next(vm)?.map(Owned::new))
        })
    } else if default.is_some() {
        Err(default_with_multiple_args(func_name))
    } else {
        // Multiple arguments: compare them directly
        select_best(first_arg, key_fn.as_ref(), is_min, vm, |_| Ok(args.next()))
    }
}

/// Folds the candidates from `next` into the min/max, starting from `best`.
///
/// With a key function, candidates are compared by their keys; ties keep the
/// first-seen item, matching CPython. Replaced candidates drop as [`Owned`].
fn select_best<'h>(
    mut best: Owned,
    key_fn: Option<&Owned>,
    is_min: bool,
    vm: &mut VM<'h>,
    mut next: impl FnMut(&mut VM<'h>) -> RunResult<Option<Owned>>,
) -> RunResult<Value> {
    // The explicit `release` calls are only a fast path; dropping would also be correct.
    if let Some(key_fn) = key_fn {
        let mut best_key = evaluate_key(&best, key_fn, vm)?;
        while let Some(item) = next(vm)? {
            let item_key = evaluate_key(&item, key_fn, vm)?;
            if candidate_wins(&best_key, &item_key, is_min, vm)? {
                mem::replace(&mut best, item).release(vm);
                mem::replace(&mut best_key, item_key).release(vm);
            } else {
                item.release(vm);
                item_key.release(vm);
            }
        }
    } else {
        while let Some(item) = next(vm)? {
            if candidate_wins(&best, &item, is_min, vm)? {
                mem::replace(&mut best, item).release(vm);
            } else {
                item.release(vm);
            }
        }
    }
    Ok(best.into_value())
}

/// Argument shape for `min(*args, key=None, default=...)`.
///
/// `key` is held as a raw value so `key=None` can be normalised to "no key
/// function". `default` is an `Option` (with `default` attr) so the
/// implementation can distinguish "not provided" from "provided with any
/// value" — that distinction matters because `default=` is only valid with a
/// single iterable argument.
#[derive(FromArgs)]
#[from_args(name = "min")]
struct MinArgs {
    #[from_args(varargs)]
    args: OwnedIntoIter,
    #[from_args(default = Owned::new(Value::None))]
    key: Owned,
    #[from_args(default)]
    default: Option<Owned>,
}

/// Argument shape for `max(*args, key=None, default=...)`.
///
/// See [`MinArgs`] for field semantics; the only difference is the function
/// name used in error messages.
#[derive(FromArgs)]
#[from_args(name = "max")]
struct MaxArgs {
    #[from_args(varargs)]
    args: OwnedIntoIter,
    #[from_args(default = Owned::new(Value::None))]
    key: Owned,
    #[from_args(default)]
    default: Option<Owned>,
}

/// Calls the user-provided key function for a single candidate value.
///
/// Passes a clone so the candidate itself stays owned by the caller.
fn evaluate_key(item: &Owned, key_fn: &Owned, vm: &mut VM<'_>) -> RunResult<Owned> {
    let arg = item.clone_with_heap(vm);
    let key = vm.evaluate_function(key_fn, ArgValues::One(arg))?;
    Ok(Owned::new(key))
}

/// Returns whether `candidate` should replace `current` as the best value seen so far.
///
/// `min()` replaces the current winner when the new candidate compares smaller,
/// while `max()` replaces it when the new candidate compares larger. Equal values
/// keep the existing winner so ties preserve the first-seen item, matching CPython.
pub(crate) fn candidate_wins(current: &Value, candidate: &Value, is_min: bool, vm: &mut VM<'_>) -> RunResult<bool> {
    let ordering = match candidate.py_cmp(current, vm)? {
        CmpOrder::Ordered(ordering) => ordering,
        // A `NaN` candidate (or `NaN`-carrying container) is neither smaller nor
        // larger, so it never displaces the incumbent — matching CPython, where
        // `min`/`max` only swap on a strict `<`/`>` and `NaN` yields neither.
        CmpOrder::Unordered => return Ok(false),
        CmpOrder::Incomparable => return Err(ord_not_supported(candidate, current, is_min, vm)),
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
fn ord_not_supported(left: &Value, right: &Value, is_min: bool, vm: &VM<'_>) -> RunError {
    let left_type = left.py_type_name(vm);
    let right_type = right.py_type_name(vm);
    let operator = if is_min { "<" } else { ">" };
    ExcType::type_error_ordering(operator, &left_type, &right_type)
}
