//! Implementation of the len() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
#[py_signature("len(obj, /)", style = one_arg)]
pub fn builtin_len(vm: &mut VM<'_>, obj: &Value) -> RunResult<Value> {
    let len = match obj {
        Value::InternString(id) => Some(vm.interns.str_char_len(*id)),
        Value::InlineString(text) => Some(text.char_len()),
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id).len()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => Some(list.len()),
            HeapData::Tuple(tuple) => Some(tuple.as_slice().len()),
            HeapData::Dict(dict) => Some(dict.len()),
            HeapData::Bytes(bytes) => Some(bytes.as_slice().len()),
            HeapData::Str(text) => Some(text.char_len()),
            _ => obj.py_len(vm),
        },
        _ => obj.py_len(vm),
    };
    if let Some(len) = len {
        Ok(Value::Int(
            i64::try_from(len).map_err(|_| ExcType::overflow_c_ssize_t())?,
        ))
    } else {
        let type_name = obj.py_type_name(vm);
        Err(ExcType::type_error_no_len(&type_name))
    }
}
