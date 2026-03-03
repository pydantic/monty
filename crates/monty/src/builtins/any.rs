//! Implementation of the any() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::RunResult,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the any() builtin function.
///
/// Returns True if any element of the iterable is true.
/// Returns False for an empty iterable. Short-circuits on the first truthy value.
pub fn builtin_any(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let iterable = args.get_one_arg("any", vm.heap)?;
    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);

    while let Some(item) = iter.for_next(vm)? {
        defer_drop!(item, vm);
        let is_truthy = item.py_bool(vm.heap, vm.interns);
        if is_truthy {
            return Ok(Value::Bool(true));
        }
    }

    Ok(Value::Bool(false))
}
