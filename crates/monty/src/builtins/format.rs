//! Implementation of the format() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::str::allocate_string, value::Value};

/// Implementation of the format() builtin function.
///
/// Applies a format-spec mini-language string to a value, sharing the runtime
/// path f-strings and `str.format()` use, so every spec and error behaves the
/// same across all three entry points.
///
/// The converter reports the expected type and position for an invalid format spec.
#[py_signature("format(value, format_spec: str=..., /)", bad_arg)]
pub fn builtin_format(vm: &mut VM<'_>, params: &FormatArgs) -> RunResult<Value> {
    // The spec borrows `vm`, which the formatter needs mutably; it is already
    // tracked so copying it out is bounded.
    let spec = params.format_spec.as_ref().map(|spec| spec.as_str(vm).to_owned());
    let formatted = vm.format_runtime_value(&params.value, 0, spec.as_deref())?;
    Ok(allocate_string(formatted, vm.heap))
}
