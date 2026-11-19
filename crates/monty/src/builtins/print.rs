//! Implementation of the print() builtin function.

use monty_types::PrintStream;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    intern::StaticStrings,
    py_value::PyValue,
    types::PyTrait,
    value::{Marker, Value},
};

/// Implementation of the print() builtin function.
///
/// Supports the following keyword arguments:
/// - `sep`: separator between values (default: " ")
/// - `end`: string appended after the last value (default: "\n")
/// - `file`: `sys.stdout` (the default) or `sys.stderr`; anything else raises
///   `TypeError`, since Monty has no file objects to write into
/// - `flush`: whether to flush the stream (accepted but ignored — Monty
///   doesn't buffer stdout)
pub fn builtin_print(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    vm.with_args(args, print_objects)
}

fn print_objects(args: &PrintArgs, vm: &mut VM<'_>) -> RunResult<Value> {
    // `sep` and `end` are checked before `file` because CPython reports them in
    // that order: `print(x, sep=1, file=3)` raises about `sep`, not the file.
    let sep_str = extract_string_kwarg(&args.sep, "sep", vm)?;
    let end_str = extract_string_kwarg(&args.end, "end", vm)?;
    let stream = print_stream(&args.file, vm)?;

    let mut first = true;
    for value in &args.objects {
        if first {
            first = false;
        } else if let Some(sep) = &sep_str {
            vm.print_writer.write(stream, sep.as_str().into())?;
        } else {
            vm.print_writer.push(stream, ' ')?;
        }
        let s = value.py_str(vm)?;
        defer_drop!(s, vm);
        // Resolve the `str` `Value` against the heap/interns tables directly so
        // the `&str` borrow stays disjoint from the `&mut vm.print_writer` write.
        vm.print_writer.write(
            stream,
            s.as_str_heap(vm.heap, vm.interns).expect("py_str returns str").into(),
        )?;
    }

    if let Some(end) = end_str {
        vm.print_writer.write(stream, end.into())?;
    } else {
        vm.print_writer.push(stream, '\n')?;
    }

    Ok(Value::None)
}

/// Resolves the `file` kwarg to the stream `print()` should write to.
///
/// `sys.stdout` and `sys.stderr` are opaque markers rather than file objects
/// (see `limitations/sys.md`), so they are matched by identity. Any other
/// value, including a class defining `write()`, has nothing to write into.
fn print_stream(file: &Value, vm: &VM<'_>) -> RunResult<PrintStream> {
    match file {
        Value::None | Value::Marker(Marker(StaticStrings::Stdout)) => Ok(PrintStream::Stdout),
        Value::Marker(Marker(StaticStrings::Stderr)) => Ok(PrintStream::Stderr),
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!(
                "print() 'file' argument must be sys.stdout or sys.stderr, not {}",
                file.py_type_name(vm)
            ),
        )
        .into()),
    }
}

/// Argument shape for `print(*objects, sep=' ', end='\n', file=sys.stdout, flush=False)`.
///
/// Every kwarg is held as a raw `Value` so the caller can do the
/// "must be None or str" coercion inline, and so `flush` can be accepted
/// without forcing a type check. `file` is resolved by `print_stream`.
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
    /// Monty accepts `flush` but writes to the stream immediately.
    #[from_args(default = Value::None)]
    flush: Value,
}

/// Extracts a string value from a print() kwarg.
///
/// The kwarg can be None (returns None) or a string (returns Some).
/// Raises TypeError for other types.
fn extract_string_kwarg(value: &Value, name: &str, vm: &VM<'_>) -> RunResult<Option<String>> {
    match value.py_value(vm) {
        PyValue::None => Ok(None),
        PyValue::Str(string) => Ok(Some(string.as_str(vm).to_owned())),
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{} must be None or a string, not {}", name, value.py_type_name(vm)),
        )
        .into()),
    }
}
