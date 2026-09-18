//! Implementation of the divmod() builtin function.

use crate::{args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, value::Value};

/// Implementation of the divmod() builtin function.
///
/// Returns a tuple `(quotient, remainder)` equivalent to `(a // b, a % b)`.
/// The per-type arithmetic lives with the types themselves, reached through
/// `__divmod__` / `__rdivmod__` like every other numeric operation.
pub fn builtin_divmod(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (a, b) = args.get_two_args("divmod", vm.heap)?;
    defer_drop!(a, vm);
    defer_drop!(b, vm);
    a.py_divmod(b, vm)
}
