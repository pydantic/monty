//! Implementation of the hex() builtin function.

use num_traits::Signed;

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, types::str::allocate_string_no_interning,
    value::Value,
};

/// Implementation of the hex() builtin function.
///
/// Converts an integer to a lowercase hexadecimal string prefixed with '0x'.
/// Supports both i64 and BigInt integers.
pub fn builtin_hex(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.py_index(vm)?;
    defer_drop!(index, vm);

    let s = match index {
        Value::Int(n) => {
            format!("{}0x{:x}", if *n < 0 { "-" } else { "" }, n.unsigned_abs())
        }
        _ if let Some(bi) = index.as_long_int(vm) => {
            format!("{}0x{:x}", if bi.is_negative() { "-" } else { "" }, bi.magnitude())
        }
        _ => unreachable!("py_index returned an integer"),
    };
    Ok(allocate_string_no_interning(s, vm.heap))
}
