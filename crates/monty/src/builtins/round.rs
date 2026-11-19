//! Implementation of the round() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::PyTrait,
    value::Value,
};

/// Rounds through `__round__`, leaving precision coercion to the receiving type.
#[py_signature("round(number, ndigits=None)")]
pub fn builtin_round(vm: &mut VM<'_>, args: &RoundArgs) -> RunResult<Value> {
    let RoundArgs { number, ndigits } = args;
    let ndigits = if matches!(ndigits, Value::None) {
        None
    } else {
        Some(ndigits)
    };
    number
        .py_round_impl(ndigits, vm)?
        .ok_or_else(|| ExcType::type_error_missing_numeric_dunder(&number.py_type_name(vm), "__round__"))
}
