//! Implementation of the abs() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::PyTrait,
    value::Value,
};

/// Returns the absolute value through the operand's `__abs__` implementation.
pub fn builtin_abs(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("abs", vm.heap)?;
    defer_drop!(value, vm);
    value
        .py_abs_impl(vm)?
        .ok_or_else(|| ExcType::type_error(format!("bad operand type for abs(): '{}'", value.py_type_name(vm))))
}
