//! Implementation of the ord() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::PyTrait,
    value::Value,
};

/// Implementation of the ord() builtin function.
///
/// Returns the Unicode code point of a one-character string.
pub fn builtin_ord(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("ord")?;

    let result = match &value {
        Value::InternString(string_id) => {
            let s = interns.get_str(*string_id);
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(Value::Int(c as i64)),
                _ => {
                    exc_err_fmt!(ExcType::TypeError; "ord() expected a character, but string of length {} found", s.chars().count())
                }
            }
        }
        Value::Ref(id) => {
            if let HeapData::Str(s) = heap.get(*id) {
                let mut chars = s.as_str().chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(Value::Int(c as i64)),
                    _ => {
                        exc_err_fmt!(ExcType::TypeError; "ord() expected a character, but string of length {} found", s.as_str().chars().count())
                    }
                }
            } else {
                exc_err_fmt!(ExcType::TypeError; "ord() expected string of length 1, but {} found", value.py_type(Some(heap)))
            }
        }
        _ => {
            exc_err_fmt!(ExcType::TypeError; "ord() expected string of length 1, but {} found", value.py_type(Some(heap)))
        }
    };

    value.drop_with_heap(heap);
    result
}
