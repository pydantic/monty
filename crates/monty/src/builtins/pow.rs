//! Implementation of the pow() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Dispatches `__pow__` / `__rpow__`, passing an optional modulus unchanged.
/// Builtin numeric types accept a modulus only for integer operands.
pub fn builtin_pow(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = PowArgs::from_args(args, vm)?;
    defer_drop!(base, vm);
    defer_drop!(exp, vm);
    defer_drop!(modulus, vm);
    base.py_pow(exp, Some(modulus), vm)
}

/// CPython accepts keywords in `pow(base, exp, mod=None)`. Monty raises
/// `NotImplementedError` for keywords. Rename `modulus` to `r#mod` when adding support.
#[derive(FromArgs)]
#[from_args(name = "pow", kwargs_not_supported_yet)]
struct PowArgs {
    base: Value,
    exp: Value,
    #[from_args(default = Value::None)]
    modulus: Value,
}
