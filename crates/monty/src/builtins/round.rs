//! Implementation of the round() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::PyTrait,
    value::Value,
};

/// CPython's keyword-capable `round(number, ndigits=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "round", style = c_named, at_most_total)]
struct RoundArgs {
    number: Value,
    #[from_args(default = Value::None)]
    ndigits: Value,
}

/// Rounds through `__round__`, leaving precision coercion to the receiving type.
pub fn builtin_round(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let RoundArgs { number, ndigits } = RoundArgs::from_args(args, vm)?;
    defer_drop!(number, vm);
    defer_drop!(ndigits, vm);
    let ndigits = if matches!(ndigits, Value::None) {
        None
    } else {
        Some(ndigits)
    };
    number
        .py_round_impl(ndigits, vm)?
        .ok_or_else(|| ExcType::type_error_missing_numeric_dunder(&number.py_type_name(vm), "__round__"))
}
