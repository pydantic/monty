//! Implementation of the repr() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    resource::ResourceTracker,
    types::{PyTrait, str::allocate_string},
    value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
pub fn builtin_repr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    let s = value.py_repr(vm)?.into_owned();
    Ok(allocate_string(s, vm.heap)?)
}
