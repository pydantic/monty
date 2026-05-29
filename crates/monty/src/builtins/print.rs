//! Implementation of the print() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the print() builtin function.
///
/// Supports the following keyword arguments:
/// - `sep`: separator between values (default: " ")
/// - `end`: string appended after the last value (default: "\n")
/// - `flush`: whether to flush the stream (accepted but ignored — Monty
///   doesn't buffer stdout)
///
/// The `file` keyword is recognised so it can produce a *specific* error
/// (`"print() 'file' argument is not supported"`) rather than the generic
/// "unexpected keyword" produced by leaving it off the struct.
pub fn builtin_print(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let PrintArgs {
        objects,
        sep,
        end,
        file,
        flush: _,
    } = PrintArgs::from_args(args, vm)?;
    defer_drop!(objects, vm);
    defer_drop!(sep, vm);
    defer_drop!(end, vm);
    defer_drop!(file, vm);

    if !matches!(file, Value::None) {
        return Err(SimpleException::new_msg(ExcType::TypeError, "print() 'file' argument is not supported").into());
    }

    let sep_str = extract_string_kwarg(sep, "sep", vm)?;
    let end_str = extract_string_kwarg(end, "end", vm)?;

    let mut first = true;
    for value in objects.as_slice() {
        if first {
            first = false;
        } else if let Some(sep) = &sep_str {
            vm.print_writer.stdout_write(sep.as_str().into())?;
        } else {
            vm.print_writer.stdout_push(' ')?;
        }
        let s = value.py_str(vm)?;
        vm.print_writer.stdout_write(s)?;
    }

    if let Some(end) = end_str {
        vm.print_writer.stdout_write(end.into())?;
    } else {
        vm.print_writer.stdout_push('\n')?;
    }

    Ok(Value::None)
}

/// Argument shape for `print(*objects, sep=' ', end='\n', file=sys.stdout, flush=False)`.
///
/// Every kwarg is held as a raw `Value` so the caller can do the
/// "must be None or str" coercion inline, and so `flush` can be accepted
/// without forcing a type check. Explicit `file` rejection lives in
/// `builtin_print`.
#[derive(FromArgs)]
#[from_args(name = "print")]
struct PrintArgs {
    #[from_args(varargs)]
    objects: Vec<Value>,
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::None)]
    end: Value,
    #[from_args(default = Value::None)]
    file: Value,
    /// Accepted from Python for CPython compatibility but never consumed:
    /// Monty doesn't buffer stdout, so there is nothing to flush.
    #[expect(dead_code, reason = "accepted but ignored — Monty doesn't buffer stdout")]
    #[from_args(default = Value::None)]
    flush: Value,
}

/// Extracts a string value from a print() kwarg.
///
/// The kwarg can be None (returns None) or a string (returns Some).
/// Raises TypeError for other types.
fn extract_string_kwarg(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Option<String>> {
    match value {
        Value::None => Ok(None),
        Value::InternString(string_id) => Ok(Some(vm.interns.get_str(*string_id).to_owned())),
        Value::Ref(id) => {
            if let HeapData::Str(s) = vm.heap.get(*id) {
                return Ok(Some(s.as_str().to_owned()));
            }
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("{} must be None or a string, not {}", name, value.py_type(vm)),
            )
            .into())
        }
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{} must be None or a string, not {}", name, value.py_type(vm)),
        )
        .into()),
    }
}
