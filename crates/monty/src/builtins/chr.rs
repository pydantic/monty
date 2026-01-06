//! Implementation of the chr() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Str},
    value::Value,
};

/// Implementation of the chr() builtin function.
///
/// Returns a string representing a character whose Unicode code point is the integer.
/// The valid range for the argument is from 0 through 1,114,111 (0x10FFFF).
pub fn builtin_chr(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("chr")?;

    let result = match &value {
        Value::Int(n) => {
            if *n < 0 || *n > 0x0010_FFFF {
                exc_err_fmt!(ExcType::ValueError; "chr() arg not in range(0x110000)")
            } else if let Some(c) = char::from_u32(*n as u32) {
                let s = c.to_string();
                let heap_id = heap.allocate(HeapData::Str(Str::new(s)))?;
                Ok(Value::Ref(heap_id))
            } else {
                // This shouldn't happen for valid Unicode range, but handle it
                exc_err_fmt!(ExcType::ValueError; "chr() arg not in range(0x110000)")
            }
        }
        Value::Bool(b) => {
            // bool is subclass of int
            let c = if *b { '\x01' } else { '\x00' };
            let s = c.to_string();
            let heap_id = heap.allocate(HeapData::Str(Str::new(s)))?;
            Ok(Value::Ref(heap_id))
        }
        _ => {
            exc_err_fmt!(ExcType::TypeError; "an integer is required (got type {})", value.py_type(Some(heap)))
        }
    };

    value.drop_with_heap(heap);
    result
}
