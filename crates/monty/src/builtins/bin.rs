//! Implementation of the bin() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM, defer_drop, exception_private::RunResult, py_value::PyValue,
    types::str::allocate_string_no_interning, value::Value,
};

/// Converts an integer or `__index__` result to a binary string prefixed with `0b`.
#[py_signature("bin(number, /)", style = one_arg)]
pub fn builtin_bin(vm: &mut VM<'_>, number: &Value) -> RunResult<Value> {
    let index = number.py_index(vm)?;
    defer_drop!(index, vm);

    let PyValue::Int(integer) = index.py_value(vm) else {
        unreachable!("py_index returned an integer");
    };
    Ok(allocate_string_no_interning(integer.radix_repr(2, vm), vm.heap))
}
