//! Implementation of the sum() builtin function.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::DropGuard,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait, Type},
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
pub fn builtin_sum(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let SumArgs { iterable, start } = SumArgs::from_args(args, vm)?;
    defer_drop_mut!(start, vm);

    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);

    // Reject str/bytes start values - Python explicitly forbids these
    match start.py_type(vm) {
        Type::Str => return Err(ExcType::type_error_sum_start("strings", "''")),
        Type::Bytes => return Err(ExcType::type_error_sum_start("bytes", "b''")),
        _ => {}
    }
    // Take the start value out of its guard (dropping `None` is a no-op).
    let accumulator = mem::replace(start, Value::None);

    // DropGuard for accumulator: on success we extract it via into_inner(),
    // on any error path it's dropped automatically
    let mut acc_guard = DropGuard::new(accumulator, vm);
    let (accumulator, vm) = acc_guard.as_parts_mut();

    // Sum all items
    while let Some(item) = iter.for_next(vm)? {
        defer_drop!(item, vm);

        // Try to add the item to accumulator
        if let Some(new_value) = accumulator.py_add(item, vm)? {
            // Replace the old accumulator with the new value, dropping the old one
            let old = mem::replace(accumulator, new_value);
            old.drop_with(vm);
        } else {
            // Types don't support addition
            let acc_type = accumulator.py_type(vm);
            let acc_name = acc_type.name(vm.heap, vm.interns);
            return Err(ExcType::binary_type_error(
                "+",
                acc_type,
                acc_name,
                item.py_type_name(vm),
            ));
        }
    }

    Ok(acc_guard.into_inner())
}
