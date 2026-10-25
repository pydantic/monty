//! Implementation of the hex() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, py_value::PyValue,
    types::str::allocate_string_no_interning, value::Value,
};

/// Converts an integer or `__index__` result to a hexadecimal string prefixed with `0x`.
pub fn builtin_hex(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.py_index(vm)?;
    defer_drop!(index, vm);

    let PyValue::Int(integer) = index.py_value(vm) else {
        unreachable!("py_index returned an integer");
    };
    Ok(allocate_string_no_interning(integer.radix_repr(16, vm), vm.heap))
}
