//! Implementation of the oct() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::HeapData,
    resource::ResourceTracker,
    types::str::allocate_string_no_interning,
    value::Value,
};

/// Implementation of the oct() builtin function.
///
/// Converts an integer to an octal string prefixed with '0o'.
/// Supports both i64 and BigInt integers.
pub fn builtin_oct(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("oct", vm.heap)?;
    defer_drop!(value, vm);

    match value {
        Value::Int(n) => {
            let abs_digits = format!("{:o}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0o" } else { "0o" };
            Ok(allocate_string_no_interning(format!("{prefix}{abs_digits}"), vm.heap)?)
        }
        Value::Bool(b) => {
            let s = if *b { "0o1" } else { "0o0" };
            Ok(allocate_string_no_interning(s.to_string(), vm.heap)?)
        }
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
            let oct_str = format_bigint_oct(li.inner());
            Ok(allocate_string_no_interning(oct_str, vm.heap)?)
        }
        _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
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
