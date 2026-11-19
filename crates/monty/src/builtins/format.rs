//! Implementation of the format() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::str::allocate_string, value::Value};

/// Implementation of the format() builtin function.
///
/// Applies a format-spec mini-language string to a value, sharing the runtime
/// path f-strings and `str.format()` use, so every spec and error behaves the
/// same across all three entry points.
///
/// Argument shape for `format(value, format_spec='', /)`.
///
/// CPython parses it with `PyArg_ParseTuple`, so `style = unpack` gives the
/// `format expected at most 2 arguments` arity wording and the blanket
/// `format() takes no keyword arguments` rejection; `bad_arg` supplies
/// `format() argument 2 must be str, not int`.
#[py_signature("format(value, format_spec: str=..., /)", style = unpack, bad_arg)]
pub fn builtin_format(vm: &mut VM<'_>, args: &FormatArgs) -> RunResult<Value> {
    let FormatArgs { value, format_spec } = args;
    // The spec borrows `vm`, which the formatter needs mutably; it is already
    // tracked so copying it out is bounded.
    let spec = format_spec.as_ref().map(|spec| spec.as_str(vm).to_owned());
    let formatted = vm.format_runtime_value(value, 0, spec.as_deref())?;
    Ok(allocate_string(formatted, vm.heap))
}
