//! Implementation of the ord() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunResult, SimpleException},
    py_value::PyValue,
    value::Value,
};

/// Implementation of the ord() builtin function.
///
/// Returns the Unicode code point of a one-character string.
#[py_signature("ord(c, /)", style = one_arg)]
pub fn builtin_ord(vm: &mut VM<'_>, c: &Value) -> RunResult<Value> {
    if let PyValue::Str(string) = c.py_value(vm) {
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
    } else {
        let type_name = c.py_type_name(vm);
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("ord() expected string of length 1, but {type_name} found"),
        )
        .into())
    }
}
