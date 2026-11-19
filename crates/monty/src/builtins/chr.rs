//! Implementation of the chr() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::str::allocate_char,
    value::Value,
};

/// Returns the character at an `__index__`-compatible Unicode code point.
#[py_signature("chr(i, /)")]
pub fn builtin_chr(vm: &mut VM<'_>, args: &ChrArgs) -> RunResult<Value> {
    let ChrArgs { i } = args;
    let index = i.saturating_index(vm)?;
    if let Some(c) = u32::try_from(index).ok().and_then(char::from_u32) {
        Ok(allocate_char(c))
    } else {
        Err(ExcType::value_error("chr() arg not in range(0x110000)"))
    }
}
