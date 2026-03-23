//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, types::PyTrait,
    value::Value,
};

/// Implementation of the type() builtin function.
///
/// Returns the type of an object.
pub fn builtin_type(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("type", vm.heap)?;
    defer_drop!(value, vm);
    Ok(Value::Builtin(Builtins::Type(value.py_type(vm))))
}
