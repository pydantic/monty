//! Implementation of the hex() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    py_value::PyValue,
    types::str::allocate_string_no_interning,
    value::Value,
};

/// Converts a Python integer to a signed hexadecimal string prefixed with `0x`.
pub fn builtin_hex(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex", vm.heap)?;
    defer_drop!(value, vm);

    let text = match value.py_value(vm) {
        PyValue::Int(integer) => integer.radix_repr(16, vm),
        PyValue::Bool(value) => if value { "0x1" } else { "0x0" }.to_owned(),
        _ => return Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
    };
    Ok(allocate_string_no_interning(text, vm.heap))
}
