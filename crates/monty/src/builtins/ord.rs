//! Implementation of the ord() builtin function.

use monty_macros::py_signature;

use crate::{
    args::{FromValue, FromValueFail},
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::DropWithContext,
    py_value::PyValue,
    value::Value,
};

/// Returns the code point of a single character, or the value of a single byte.
#[py_signature("ord(c: str | bytes, /) -> int", convert(c = Ordinal))]
pub fn builtin_ord(vm: &mut VM<'_>, params: &OrdArgs) -> RunResult<Value> {
    Ok(Value::Int(i64::from(params.c.0)))
}

struct Ordinal(u32);

impl FromValue for Ordinal {
    fn from_value(value: Value, vm: &mut VM<'_>) -> Result<Self, FromValueFail> {
        let code = match value.py_value(vm) {
            PyValue::Str(text) => {
                let mut chars = text.as_str(vm).chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => Ok(u32::from(ch)),
                    (None, _) => Err(0),
                    _ => Err(2 + chars.count()),
                }
            }
            PyValue::Bytes(bytes) => match bytes.as_slice(vm) {
                [byte] => Ok(u32::from(*byte)),
                bytes => Err(bytes.len()),
            },
            _ => return Err(FromValueFail::WrongType(value)),
        };
        value.drop_with(vm);
        code.map(Self).map_err(|len| {
            FromValueFail::Raise(ExcType::type_error(format!(
                "ord() expected a character, but string of length {len} found"
            )))
        })
    }

    fn type_error(got: &str) -> RunError {
        ExcType::type_error(format!("ord() expected string of length 1, but {got} found"))
    }
}

impl<C> DropWithContext<C> for Ordinal {
    fn drop_with(self, _: &mut C) {}
}
