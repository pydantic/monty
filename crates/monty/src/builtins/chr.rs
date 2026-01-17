//! Implementation of the chr() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the chr() builtin function.
///
/// Returns a string representing a character whose Unicode code point is the integer.
/// The valid range for the argument is from 0 through 1,114,111 (0x10FFFF).
pub fn builtin_chr(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("chr", heap)?;

    let result = match &value {
        Value::Int(n) => {
            if *n < 0 || *n > 0x0010_FFFF {
                Err(SimpleException::new_msg(ExcType::ValueError, "chr() arg not in range(0x110000)").into())
            } else if let Some(c) = char::from_u32(u32::try_from(*n).expect("chr() range check failed")) {
                Ok(heap.allocate_char(c)?)
            } else {
                // This shouldn't happen for valid Unicode range, but handle it
                Err(SimpleException::new_msg(ExcType::ValueError, "chr() arg not in range(0x110000)").into())
            }
        }
        Value::Bool(b) => {
            // bool is subclass of int
            let c = if *b { '\x01' } else { '\x00' };
            Ok(heap.allocate_char(c)?)
        }
        _ => {
            let type_name = value.py_type(heap);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("an integer is required (got type {type_name})"),
            )
            .into())
        }
    };

    value.drop_with_heap(heap);
    result
}
