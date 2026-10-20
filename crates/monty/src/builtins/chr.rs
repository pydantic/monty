//! Implementation of the chr() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::RuntimeRef,
    types::str::allocate_char,
    value::Value,
};

/// Returns the character at an `__index__`-compatible Unicode code point.
pub fn builtin_chr<H: RuntimeRef>(vm: &mut VM<'_, H>, args: ArgValues<H>) -> RunResult<Value<H>> {
    let value = args.get_one_arg("chr", vm.heap)?;
    defer_drop!(value, vm);
    let index = value.saturating_index(vm)?;
    if let Some(c) = u32::try_from(index).ok().and_then(char::from_u32) {
        Ok(allocate_char(c, vm.heap))
    } else {
        Err(ExcType::value_error("chr() arg not in range(0x110000)"))
    }
}
