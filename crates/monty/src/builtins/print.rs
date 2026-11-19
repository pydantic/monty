//! Implementation of the print() builtin function.

use monty_macros::py_signature;
use monty_types::PrintStream;

use crate::{
    args::{ArgValues, DefaultedStringArg},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    intern::StaticStrings,
    py_value::PyValue,
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
#[py_signature(r"print(*args, sep=' ', end='\n', file=None, flush=False)")]
pub fn builtin_print(vm: &mut VM<'_>, params: &PrintArgs) -> RunResult<Value> {
    let sep_str = extract_string_kwarg(&params.sep, "sep", " ", vm)?;
    let end_str = extract_string_kwarg(&params.end, "end", "\n", vm)?;
    let target = print_target(&params.file, vm)?;

    let mut first = true;
    for value in &params.args {
        if first {
            first = false;
        } else {
            sep_str.write_to(&target, vm)?;
        }
        let s = value.py_str(vm)?;
        defer_drop!(s, vm);
        match &target {
            // Resolve the `str` `Value` against the heap/interns tables directly so
            // the `&str` borrow stays disjoint from the `&mut vm.print_writer` write.
            PrintTarget::Output(stream) => vm.print_writer.write(
                *stream,
                s.as_str_heap(vm.heap, vm.interns).expect("py_str returns str").into(),
            )?,
            PrintTarget::Stream(stream) => write_to_stream(stream, s.clone_with_heap(vm.heap), vm)?,
        }
    }
    end_str.write_to(&target, vm)?;
    Ok(Value::None)
}

/// Where `print()` writes: the host's output streams, or an in-memory stream.
enum PrintTarget<'a> {
    /// `sys.stdout` or `sys.stderr`, written through the print writer.
    Output(PrintStream),
    /// An `io.StringIO` / `io.BytesIO`, borrowed from the `file` argument.
    Stream(&'a Value),
}

enum PrintPiece {
    Default(&'static str),
    Supplied(String),
}

impl PrintPiece {
    fn write_to(&self, target: &PrintTarget<'_>, vm: &mut VM<'_>) -> RunResult<()> {
        let text = match self {
            Self::Default(text) => {
                // The host receives default separators and terminators through its single-character callback.
                if let (PrintTarget::Output(stream), [byte]) = (target, text.as_bytes()) {
                    return Ok(vm.print_writer.push(*stream, char::from(*byte))?);
                }
                *text
            }
            Self::Supplied(text) => text.as_str(),
        };
        match target {
            PrintTarget::Output(stream) => Ok(vm.print_writer.write(*stream, text.into())?),
            PrintTarget::Stream(stream) => write_to_stream(stream, allocate_string(text, vm.heap), vm),
        }
    }
}

/// Calls an in-memory stream's `write()` with an owned `str`, discarding what it returns.
///
/// A `BytesIO` raises the `TypeError` CPython's `print()` meets on its first write.
fn write_to_stream(stream: &Value, text: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let name = vm.interns.intern_static(StaticStrings::Write);
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

/// CPython treats an explicit `None` as the default separator or terminator.
fn extract_string_kwarg(
    arg: &DefaultedStringArg,
    name: &str,
    default: &'static str,
    vm: &VM<'_>,
) -> RunResult<PrintPiece> {
    let value = match arg {
        DefaultedStringArg::Omitted(default) => return Ok(PrintPiece::Default(default)),
        DefaultedStringArg::Supplied(value) => value,
    };
    match value.py_value(vm) {
        PyValue::None => Ok(PrintPiece::Default(default)),
        PyValue::Str(string) => Ok(PrintPiece::Supplied(string.as_str(vm).to_owned())),
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{} must be None or a string, not {}", name, value.py_type_name(vm)),
        )
        .into()),
    }
}
