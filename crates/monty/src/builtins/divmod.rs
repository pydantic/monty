//! Implementation of the divmod() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, value::Value};

/// Implementation of the divmod() builtin function.
///
/// Returns a tuple `(quotient, remainder)` equivalent to `(a // b, a % b)`.
/// The per-type arithmetic lives with the types themselves, reached through
/// `__divmod__` / `__rdivmod__` like every other numeric operation.
#[py_signature("divmod(x, y, /)")]
pub fn builtin_divmod(vm: &mut VM<'_>, args: &DivmodArgs) -> RunResult<Value> {
    let DivmodArgs { x, y } = args;
    x.py_divmod(y, vm)
}
