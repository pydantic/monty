//! Implementation of the bin() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::str::allocate_string_no_interning, value::Value};

/// Converts an integer or `__index__` result to a binary string prefixed with `0b`.
#[py_signature("bin(number: SupportsIndex, /)")]
pub fn builtin_bin(vm: &mut VM<'_>, args: &BinArgs) -> RunResult<Value> {
    let BinArgs { number } = args;
    Ok(allocate_string_no_interning(number.view(vm).radix_repr(2, vm), vm.heap))
}
