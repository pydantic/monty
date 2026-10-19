//! Implementation of the oct() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, types::str::allocate_string_no_interning,
    value::Value,
};

/// Implementation of the oct() builtin function.
///
/// Converts an integer to an octal string prefixed with '0o'.
/// Supports both i64 and BigInt integers.
pub fn builtin_oct(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("oct", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.py_index(vm)?;
    defer_drop!(index, vm);

    match index {
        Value::Int(n) => {
            let abs_digits = format!("{:o}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0o" } else { "0o" };
            Ok(allocate_string_no_interning(format!("{prefix}{abs_digits}"), vm.heap))
        }
        _ if let Some(n) = index.as_long_int(vm) => {
            let oct_str = format_bigint_oct(n);
            Ok(allocate_string_no_interning(oct_str, vm.heap))
        }
        _ => unreachable!("py_index returned an integer"),
    }
}

/// Formats a BigInt as an octal string with '0o' prefix.
fn format_bigint_oct(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let oct_digits = format!("{abs_bi:o}");
    let prefix = if is_negative { "-0o" } else { "0o" };
    format!("{prefix}{oct_digits}")
}
