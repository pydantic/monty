//! Implementation of the any() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, types::PyTrait,
    value::Value,
};

/// Implementation of the any() builtin function.
///
/// Returns True if any element of the iterable is true.
/// Returns False for an empty iterable. Short-circuits on the first truthy value.
pub fn builtin_any(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let iterable = args.get_one_arg("any", vm.heap)?;
    let iter = iterable.into_py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if item.py_bool(vm) {
            return Ok(Value::Bool(true));
        }
    }

    Ok(Value::Bool(false))
}
