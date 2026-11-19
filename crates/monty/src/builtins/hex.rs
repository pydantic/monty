//! Implementation of the hex() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::str::allocate_string_no_interning, value::Value};

/// Converts an integer or `__index__` result to a hexadecimal string prefixed with `0x`.
#[py_signature("hex(number: SupportsIndex, /)")]
pub fn builtin_hex(vm: &mut VM<'_>, args: &HexArgs) -> RunResult<Value> {
    let HexArgs { number } = args;
    Ok(allocate_string_no_interning(
        number.view(vm).radix_repr(16, vm),
        vm.heap,
    ))
}
