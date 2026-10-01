//! Implementation of the print() builtin function.

use monty_types::PrintStream;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    intern::StaticStrings,
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

    // `sep` and `end` are checked before `file` because CPython reports them in
    // that order: `print(x, sep=1, file=3)` raises about `sep`, not the file.
    let sep_str = extract_string_kwarg(sep, "sep", vm)?;
    let end_str = extract_string_kwarg(end, "end", vm)?;
    let stream = print_stream(file, vm)?;

    let mut first = true;
    for value in objects.as_slice() {
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
        vm.print_writer
            .write(stream, s.to_str_heap(vm.heap, vm.interns)?.into())?;
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
fn extract_string_kwarg(value: &Value, name: &str, vm: &VM<'_>) -> RunResult<Option<String>> {
    match value {
        Value::None => Ok(None),
        Value::InternString(string_id) => Ok(Some(vm.interns.get_str(*string_id).to_owned())),
        Value::Ref(id) => {
            if let HeapData::Str(s) = vm.heap.get(*id) {
                return Ok(Some(s.as_str().to_owned()));
            }
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("{} must be None or a string, not {}", name, value.py_type_name(vm)),
            )
            .into())
        }
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{} must be None or a string, not {}", name, value.py_type_name(vm)),
        )
        .into()),
    }
}
