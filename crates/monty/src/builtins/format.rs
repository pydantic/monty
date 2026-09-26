//! Implementation of the format() builtin function.

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    types::str::allocate_string,
    value::Value,
};

/// Argument shape for `format(value, format_spec='', /)`.
///
/// CPython parses it with `PyArg_ParseTuple`, so `style = unpack` gives the
/// `format expected at most 2 arguments` arity wording and the blanket
/// `format() takes no keyword arguments` rejection; `bad_arg` supplies
/// `format() argument 2 must be str, not int`.
#[derive(FromArgs)]
#[from_args(name = "format", style = unpack, bad_arg)]
struct FormatArgs {
    #[from_args(pos_only)]
    value: Value,
    #[from_args(pos_only, default)]
    format_spec: Option<StrArg>,
}

/// Implementation of the format() builtin function.
///
/// Applies a format-spec mini-language string to a value, sharing the runtime
/// path f-strings and `str.format()` use, so every spec and error behaves the
/// same across all three entry points.
pub fn builtin_format(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FormatArgs { value, format_spec } = FormatArgs::from_args(args, vm)?;
    defer_drop!(value, vm);
    defer_drop!(format_spec, vm);
    // The spec borrows `vm`, which the formatter needs mutably; it is already
    // tracked so copying it out is bounded.
    let spec = format_spec.as_ref().map(|spec| spec.as_str(vm).to_owned());
    let formatted = vm.format_runtime_value(value, 0, spec.as_deref())?;
    Ok(allocate_string(formatted, vm.heap))
}
