//! Implementation of the oct() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Str},
    value::Value,
};

/// Implementation of the oct() builtin function.
///
/// Converts an integer to an octal string prefixed with '0o'.
pub fn builtin_oct(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("oct")?;

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
        _ => {
            exc_err_fmt!(ExcType::TypeError; "'{}' object cannot be interpreted as an integer", value.py_type(Some(heap)))
        }
    };

    value.drop_with_heap(heap);
    result
}
