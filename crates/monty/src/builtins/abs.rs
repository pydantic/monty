//! Implementation of the abs() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::PyTrait,
    value::Value,
};

/// Returns the absolute value through the operand's `__abs__` implementation.
#[py_signature("abs(x, /)")]
pub fn builtin_abs(vm: &mut VM<'_>, params: &AbsArgs) -> RunResult<Value> {
    params
        .x
        .py_abs_impl(vm)?
        .ok_or_else(|| ExcType::type_error(format!("bad operand type for abs(): '{}'", params.x.py_type_name(vm))))
}
