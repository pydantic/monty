//! Implementation of the pow() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, value::Value};

/// Implementation of the pow() builtin function.
///
/// Dispatches `__pow__` / `__rpow__`, passing an optional modulus unchanged.
/// Builtin numeric types accept a modulus only for integer operands.
///
/// The binder reports missing parameter names and the maximum positional count.
/// Keyword calls raise `NotImplementedError` pending support for `mod=`.
#[py_signature("pow(base, exp, modulus=None)", kwargs_not_supported_yet)]
pub fn builtin_pow(vm: &mut VM<'_>, args: &PowArgs) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = args;
    base.py_pow(exp, Some(modulus), vm)
}
