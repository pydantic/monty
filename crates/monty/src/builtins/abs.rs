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
pub fn builtin_abs(vm: &mut VM<'_>, x: &Value) -> RunResult<Value> {
    x.py_abs_impl(vm)?
        .ok_or_else(|| ExcType::type_error(format!("bad operand type for abs(): '{}'", x.py_type_name(vm))))
}
