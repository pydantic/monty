//! Implementation of the bin() builtin function.

use monty_types::ResourceTracker;
use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    types::str::allocate_string_no_interning,
    value::Value,
};

/// Implementation of the bin() builtin function.
///
/// Converts an integer to a binary string prefixed with '0b'.
/// Supports both i64 and BigInt integers.
pub fn builtin_bin(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("bin", vm.heap)?;
    defer_drop!(value, vm);

    match value {
        Value::Int(n) => {
            let abs_digits = format!("{:b}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0b" } else { "0b" };
            Ok(allocate_string_no_interning(format!("{prefix}{abs_digits}"), vm.heap)?)
        }
        Value::Bool(b) => {
            let s = if *b { "0b1" } else { "0b0" };
            Ok(allocate_string_no_interning(s.to_string(), vm.heap)?)
        }
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
            let bin_str = format_bigint_bin(li.inner());
            Ok(allocate_string_no_interning(bin_str, vm.heap)?)
        }
        _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
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
