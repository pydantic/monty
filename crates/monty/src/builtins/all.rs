//! Implementation of the all() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, types::PyTrait,
    value::Value,
};

/// Implementation of the all() builtin function.
///
/// Returns True if all elements of the iterable are true (or if the iterable is empty).
/// Short-circuits on the first falsy value.
pub fn builtin_all(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let iterable = args.get_one_arg("all", vm.heap)?;
    defer_drop!(iterable, vm);
    let iter = iterable.py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if !item.py_bool(vm) {
            return Ok(Value::Bool(false));
        }
    }

    Ok(Value::Bool(true))
}
