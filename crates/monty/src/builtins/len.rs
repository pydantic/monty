//! Implementation of the len() builtin function.

use crate::{args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, value::Value};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("len", vm.heap)?;
    defer_drop!(value, vm);
    value.py_len(vm).map(Value::Int)
}
