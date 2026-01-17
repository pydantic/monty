//! Implementation of the bin() builtin function.

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

/// Implementation of the bin() builtin function.
///
/// Converts an integer to a binary string prefixed with '0b'.
/// Supports both i64 and BigInt integers.
pub fn builtin_bin(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("bin", heap)?;

    let result = match &value {
        Value::Int(n) => {
            let abs_digits = format!("{:b}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0b" } else { "0b" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(format!("{prefix}{abs_digits}"))))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Bool(b) => {
            let s = if *b { "0b1" } else { "0b0" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(s.to_string())))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Ref(id) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                let bin_str = format_bigint_bin(li.inner());
                let heap_id = heap.allocate(HeapData::Str(Str::new(bin_str)))?;
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

/// Formats a BigInt as a binary string with '0b' prefix.
fn format_bigint_bin(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let bin_digits = format!("{abs_bi:b}");
    let prefix = if is_negative { "-0b" } else { "0b" };
    format!("{prefix}{bin_digits}")
}
