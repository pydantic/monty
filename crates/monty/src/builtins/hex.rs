//! Implementation of the hex() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    types::{PyTrait, Str},
    value::Value,
};

/// Implementation of the hex() builtin function.
///
/// Converts an integer to a lowercase hexadecimal string prefixed with '0x'.
/// Supports both i64 and BigInt integers.
pub fn builtin_hex(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex", heap)?;

    let result = match &value {
        Value::Int(n) => {
            let abs_digits = format!("{:x}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0x" } else { "0x" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(format!("{prefix}{abs_digits}"))))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Bool(b) => {
            let s = if *b { "0x1" } else { "0x0" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(s.to_string())))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Ref(id) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                let hex_str = format_bigint_hex(li.inner());
                let heap_id = heap.allocate(HeapData::Str(Str::new(hex_str)))?;
                Ok(Value::Ref(heap_id))
            } else {
                Err(ExcType::type_error_not_integer(value.py_type(heap)))
            }
        }
        _ => Err(ExcType::type_error_not_integer(value.py_type(heap))),
    };

    value.drop_with_heap(heap);
    result
}

/// Formats a BigInt as a hexadecimal string with '0x' prefix.
fn format_bigint_hex(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let hex_digits = format!("{abs_bi:x}");
    let prefix = if is_negative { "-0x" } else { "0x" };
    format!("{prefix}{hex_digits}")
}
