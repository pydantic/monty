//! Implementation of the sum() builtin function.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, NativeCont, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropGuard,
    types::{PyTrait, Type, iter::is_user_iterator},
    value::Value,
};

/// Argument shape for `sum(iterable, /, start=0)` — Argument Clinic in
/// CPython, so `start` is keyword-capable while `iterable` is positional-only.
/// `at_most_total` reproduces `_PyArg_UnpackKeywords`' total pre-count
/// (`sum() takes at most 2 arguments (3 given)`).
#[derive(FromArgs)]
#[from_args(name = "sum", at_most_total)]
struct SumArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(default = Value::Int(0))]
    start: Value,
}

/// Implementation of the sum() builtin function.
///
/// Sums the items of an iterable from left to right with an optional start value.
/// The default start value is 0. Str and bytes start values are explicitly
/// rejected, pointing at `''.join(seq)` / `b''.join(seq)` instead.
///
/// A user iterator is folded through a native continuation, so its `__next__` may suspend.
pub fn builtin_sum(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let SumArgs { iterable, start } = SumArgs::from_args(args, vm)?;
    defer_drop_mut!(start, vm);

    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    let mut iter_guard = DropGuard::new(iter, vm);
    let (iter, vm) = iter_guard.as_parts();

    // Reject str/bytes start values - Python explicitly forbids these
    match start.py_type(vm).native() {
        Some(Type::Str) => return Err(ExcType::type_error_sum_start("strings", "''")),
        Some(Type::Bytes) => return Err(ExcType::type_error_sum_start("bytes", "b''")),
        _ => {}
    }
    // Take the start value out of its guard (dropping `None` is a no-op).
    let accumulator = mem::replace(start, Value::None);

    if is_user_iterator(iter, vm) {
        let (iterator, vm) = iter_guard.into_parts();
        return vm.start_native(NativeCont::Sum {
            iterator,
            total: accumulator,
        });
    }
    let mut iter = iter.read(vm);

    // DropGuard for accumulator: on success we extract it via into_inner(),
    // on any error path it's dropped automatically
    let mut acc_guard = DropGuard::new(accumulator, vm);
    let (accumulator, vm) = acc_guard.as_parts_mut();

    // Sum all items
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);

        let new_value = accumulator.py_add(item, vm)?;
        let old = mem::replace(accumulator, new_value);
        old.drop_with(vm);
    }

    Ok(CallResult::Value(acc_guard.into_inner()))
}
