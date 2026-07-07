//! Implementation of the repr() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, types::PyTrait,
    value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
/// `py_repr` already yields a heap `str` `Value`, so it is returned as-is
/// without an intermediate `String` round-trip.
pub fn builtin_repr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    value.py_repr(vm)
}
