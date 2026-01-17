//! Implementation of the oct() builtin function.

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

/// Implementation of the oct() builtin function.
///
/// Converts an integer to an octal string prefixed with '0o'.
/// Supports both i64 and BigInt integers.
pub fn builtin_oct(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("oct", heap)?;

    let result = match &value {
        Value::Int(n) => {
            let abs_digits = format!("{:o}", n.unsigned_abs());
            let prefix = if *n < 0 { "-0o" } else { "0o" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(format!("{prefix}{abs_digits}"))))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Bool(b) => {
            let s = if *b { "0o1" } else { "0o0" };
            let heap_id = heap.allocate(HeapData::Str(Str::new(s.to_string())))?;
            Ok(Value::Ref(heap_id))
        }
        Value::Ref(id) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                let oct_str = format_bigint_oct(li.inner());
                let heap_id = heap.allocate(HeapData::Str(Str::new(oct_str)))?;
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

/// Formats a BigInt as an octal string with '0o' prefix.
fn format_bigint_oct(bi: &BigInt) -> String {
    let is_negative = bi.is_negative();
    let abs_bi = bi.abs();
    let oct_digits = format!("{abs_bi:o}");
    let prefix = if is_negative { "-0o" } else { "0o" };
    format!("{prefix}{oct_digits}")
}
