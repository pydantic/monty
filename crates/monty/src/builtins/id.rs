//! Implementation of the id() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, value::Value,
};

/// Implementation of the id() builtin function.
///
/// Returns the identity of an object (unique integer for the object's lifetime).
pub fn builtin_id(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("id", vm.heap)?;
    defer_drop!(value, vm);

    Ok(value.id(vm).into_value(vm.heap)?)
}
