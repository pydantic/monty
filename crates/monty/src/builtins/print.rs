//! Implementation of the print() builtin function.

use monty_types::PrintStream;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    py_value::PyValue,
    symbols::StaticStrings,
    types::{PyTrait, io::is_memory_stream, str::allocate_string},
    value::{Marker, Value},
};

/// Implementation of the print() builtin function.
///
/// Supports the following keyword arguments:
/// - `sep`: separator between values (default: " ")
/// - `end`: string appended after the last value (default: "\n")
/// - `file`: `sys.stdout` (the default), `sys.stderr`, or an `io.StringIO` /
///   `io.BytesIO`, whose `write()` receives each piece in turn as in CPython;
///   anything else raises `TypeError`
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
    let target = print_target(file, vm)?;

    let mut first = true;
    for value in objects.as_slice() {
        if first {
            first = false;
        } else {
            target.write_or_push(sep_str.as_deref(), ' ', vm)?;
        }
        let s = value.py_str(vm)?;
        defer_drop!(s, vm);
        match &target {
            // Resolve the `str` `Value` against the heap and symbol table directly so
            // the `&str` borrow stays disjoint from the `&mut vm.print_writer` write.
            PrintTarget::Output(stream) => vm.print_writer.write(
                *stream,
                s.as_str_heap(vm.heap, vm.symbols).expect("py_str returns str").into(),
            )?,
            PrintTarget::Stream(stream) => write_to_stream(stream, s.clone_with_heap(vm.heap), vm)?,
        }
    }
    target.write_or_push(end_str.as_deref(), '\n', vm)?;
    Ok(Value::None)
}

/// Where `print()` writes: the host's output streams, or an in-memory stream.
enum PrintTarget<'a> {
    /// `sys.stdout` or `sys.stderr`, written through the print writer.
    Output(PrintStream),
    /// An `io.StringIO` / `io.BytesIO`, borrowed from the `file` argument.
    Stream(&'a Value),
}

impl PrintTarget<'_> {
    /// Writes a `sep` / `end` piece: the given text, or `default` when it is
    /// `None`, which the host's writer receives as a single pushed char.
    fn write_or_push(&self, text: Option<&str>, default: char, vm: &mut VM<'_>) -> RunResult<()> {
        match (self, text) {
            (Self::Output(stream), Some(text)) => Ok(vm.print_writer.write(*stream, text.into())?),
            (Self::Output(stream), None) => Ok(vm.print_writer.push(*stream, default)?),
            (Self::Stream(stream), Some(text)) => write_to_stream(stream, allocate_string(text, vm.heap), vm),
            (Self::Stream(stream), None) => {
                write_to_stream(stream, allocate_string(default.encode_utf8(&mut [0; 4]), vm.heap), vm)
            }
        }
    }
}

/// Calls an in-memory stream's `write()` with an owned `str`, discarding what it returns.
///
/// A `BytesIO` raises the `TypeError` CPython's `print()` meets on its first write.
fn write_to_stream(stream: &Value, text: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let name = vm.symbols.get_or_insert_static_symbol(StaticStrings::Write);
    let written = vm
        .call_attr(stream.clone_with_heap(vm.heap), name, ArgValues::One(text))?
        .require_synchronous(vm)?;
    written.drop_with(vm);
    Ok(())
}

/// Resolves the `file` kwarg to where `print()` should write.
///
/// `sys.stdout` and `sys.stderr` are opaque markers rather than file objects
/// (see `limitations/sys.md`), so they are matched by identity. The in-memory
/// streams are the only objects whose `write()` Monty calls; anything else,
/// including an `open()` file or a class defining `write()`, raises `TypeError`.
fn print_target<'a>(file: &'a Value, vm: &VM<'_>) -> RunResult<PrintTarget<'a>> {
    match file {
        Value::None | Value::Marker(Marker(StaticStrings::Stdout)) => Ok(PrintTarget::Output(PrintStream::Stdout)),
        Value::Marker(Marker(StaticStrings::Stderr)) => Ok(PrintTarget::Output(PrintStream::Stderr)),
        file if is_memory_stream(file, vm.heap) => Ok(PrintTarget::Stream(file)),
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!(
                "print() 'file' argument must be sys.stdout, sys.stderr, io.StringIO or io.BytesIO, not {}",
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
