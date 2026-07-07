//! Implementation of the sum() builtin function.

use std::mem;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::DropGuard,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait, Type},
    value::Value,
};

/// Implementation of the sum() builtin function.
///
/// Sums the items of an iterable from left to right with an optional start value.
/// The default start value is 0. String start values are explicitly rejected
/// (use `''.join(seq)` instead for string concatenation).
pub fn builtin_sum(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, start) = args.get_one_two_args("sum", vm.heap)?;
    defer_drop_mut!(start, vm);

    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);

    // Get the start value, defaulting to 0
    let accumulator = match start.take() {
        Some(v) => {
            // Reject string start values - Python explicitly forbids this
            if matches!(v.py_type(vm), Type::Str) {
                v.drop_with(vm);
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    "sum() can't sum strings [use ''.join(seq) instead]",
                )
                .into());
            }
            v
        }
        None => Value::Int(0),
    };

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
