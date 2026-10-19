//! Implementation of the bin() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, types::str::allocate_string_no_interning,
    value::Value,
};

/// Implementation of the bin() builtin function.
///
/// Converts an integer to a binary string prefixed with '0b'.
/// Supports both i64 and BigInt integers.
pub fn builtin_bin(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("bin", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.py_index(vm)?;
    defer_drop!(index, vm);

    match index {
        Value::Int(n) => {
            let abs_digits = format!("{:b}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0b" } else { "0b" };
            Ok(allocate_string_no_interning(format!("{prefix}{abs_digits}"), vm.heap))
        }
        _ if let Some(n) = index.as_long_int(vm) => {
            let bin_str = format_bigint_bin(n);
            Ok(allocate_string_no_interning(bin_str, vm.heap))
        }
        _ => unreachable!("py_index returned an integer"),
    }
}

/// Formats a BigInt as a binary string with '0b' prefix.
fn format_bigint_bin(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let bin_digits = format!("{abs_bi:b}");
    let prefix = if is_negative { "-0b" } else { "0b" };
    format!("{prefix}{bin_digits}")
}
