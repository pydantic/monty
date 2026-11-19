//! Implementation of the oct() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::str::allocate_string_no_interning, value::Value};

/// Converts an integer or `__index__` result to an octal string prefixed with `0o`.
#[py_signature("oct(number: SupportsIndex, /)")]
pub fn builtin_oct(vm: &mut VM<'_>, args: &OctArgs) -> RunResult<Value> {
    let OctArgs { number } = args;
    Ok(allocate_string_no_interning(number.view(vm).radix_repr(8, vm), vm.heap))
}
