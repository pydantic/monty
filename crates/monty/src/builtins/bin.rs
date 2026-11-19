//! Implementation of the bin() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, py_value::PyValue,
    types::str::allocate_string_no_interning, value::Value,
};

/// Converts an integer or `__index__` result to a binary string prefixed with `0b`.
pub fn builtin_bin(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("bin", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.py_index(vm)?;
    defer_drop!(index, vm);

    let PyValue::Int(integer) = index.py_value(vm) else {
        unreachable!("py_index returned an integer");
    };
    Ok(allocate_string_no_interning(integer.radix_repr(2, vm), vm.heap))
}
