//! Implementation of the ord() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    py_value::PyValue,
    value::Value,
};

/// Implementation of the ord() builtin function.
///
/// Returns the code point of one character, or the value of one byte in bytes/bytearray.
pub fn builtin_ord(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("ord", vm.heap)?;
    defer_drop!(value, vm);

    if let PyValue::Str(string) = value.py_value(vm) {
        let s = string.as_str(vm);
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
    } else if let Some(bytes) = value.as_buffer(vm) {
        match bytes {
            [byte] => Ok(Value::Int(i64::from(*byte))),
            _ => Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("ord() expected a character, but string of length {} found", bytes.len()),
            )
            .into()),
        }
    } else {
        let type_name = value.py_type_name(vm);
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("ord() expected string of length 1, but {type_name} found"),
        )
        .into())
    }
}
