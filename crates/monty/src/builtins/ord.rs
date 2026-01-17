//! Implementation of the ord() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the ord() builtin function.
///
/// Returns the Unicode code point of a one-character string.
pub fn builtin_ord(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("ord", heap)?;

    let result = match &value {
        Value::InternString(string_id) => {
            let s = interns.get_str(*string_id);
            let mut chars = s.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                Ok(Value::Int(c as i64))
            } else {
                let len = s.chars().count();
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("ord() expected a character, but string of length {len} found"),
                )
                .into())
            }
        }
        Value::Ref(id) => {
            if let HeapData::Str(s) = heap.get(*id) {
                let mut chars = s.as_str().chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    Ok(Value::Int(c as i64))
                } else {
                    let len = s.as_str().chars().count();
                    Err(SimpleException::new_msg(
                        ExcType::TypeError,
                        format!("ord() expected a character, but string of length {len} found"),
                    )
                    .into())
                }
            } else {
                let type_name = value.py_type(heap);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("ord() expected string of length 1, but {type_name} found"),
                )
                .into())
            }
        }
        _ => {
            let type_name = value.py_type(heap);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("ord() expected string of length 1, but {type_name} found"),
            )
            .into())
        }
    };

    value.drop_with_heap(heap);
    result
}
