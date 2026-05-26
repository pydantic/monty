//! Implementation of the hex() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::HeapData,
    resource::ResourceTracker,
    types::{PyTrait, str::allocate_string_no_interning},
    value::Value,
};

/// Implementation of the hex() builtin function.
///
/// Converts an integer to a lowercase hexadecimal string prefixed with '0x'.
/// Supports both i64 and BigInt integers.
pub fn builtin_hex(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex", vm.heap)?;
    defer_drop!(value, vm);
    let heap = &mut *vm.heap;

    match value {
        Value::Int(n) => {
            let abs_digits = format!("{:x}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0x" } else { "0x" };
            Ok(allocate_string_no_interning(format!("{prefix}{abs_digits}"), heap)?)
        }
        Value::Bool(b) => {
            let s = if *b { "0x1" } else { "0x0" };
            Ok(allocate_string_no_interning(s.to_string(), heap)?)
        }
        Value::Ref(id) if let HeapData::LongInt(li) = heap.get(*id) => {
            let hex_str = format_bigint_hex(li.inner());
            Ok(allocate_string_no_interning(hex_str, heap)?)
        }
        _ => Err(ExcType::type_error_not_integer(value.py_type(vm))),
    }
}

/// Formats a BigInt as a hexadecimal string with '0x' prefix.
fn format_bigint_hex(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let hex_digits = format!("{abs_bi:x}");
    let prefix = if is_negative { "-0x" } else { "0x" };
    format!("{prefix}{hex_digits}")
}
