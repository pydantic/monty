//! Implementation of the hex() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Str},
    value::Value,
};

/// Implementation of the hex() builtin function.
///
/// Converts an integer to a lowercase hexadecimal string prefixed with '0x'.
pub fn builtin_hex(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hex")?;

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
        _ => {
            exc_err_fmt!(ExcType::TypeError; "'{}' object cannot be interpreted as an integer", value.py_type(Some(heap)))
        }
    };

    value.drop_with_heap(heap);
    result
}
