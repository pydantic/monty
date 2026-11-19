//! Implementation of the print() builtin function.

use monty_macros::py_signature;
use monty_types::PrintStream;

use crate::{
    args::DefaultedStringArg,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    intern::StaticStrings,
    types::PyTrait,
    value::{Marker, Value},
};

#[py_signature(r"print(*args, sep=' ', end='\n', file=None, flush=False)")]
pub fn builtin_print(vm: &mut VM<'_>, bound: &PrintArgs) -> RunResult<Value> {
    // `sep` and `end` are checked before `file` because CPython reports them in
    // that order: `print(x, sep=1, file=3)` raises about `sep`, not the file.
    let sep = delimiter(&bound.sep, "sep", ' ', vm)?;
    let end = delimiter(&bound.end, "end", '\n', vm)?;
    let stream = print_stream(&bound.file, vm)?;

    let mut first = true;
    for value in &bound.args {
        if first {
            first = false;
        } else {
            sep.write(stream, vm)?;
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

    end.write(stream, vm)?;
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

enum Delimiter<'a> {
    Char(char),
    Text(&'a str),
    Value(&'a Value),
}

impl Delimiter<'_> {
    fn write(&self, stream: PrintStream, vm: &mut VM<'_>) -> RunResult<()> {
        match self {
            Self::Char(c) => vm.print_writer.push(stream, *c)?,
            Self::Text(text) => vm.print_writer.write(stream, (*text).into())?,
            Self::Value(value) => {
                let text = value
                    .as_str_heap(vm.heap, vm.interns)
                    .expect("validated print delimiter");
                vm.print_writer.write(stream, text.into())?;
            }
        }
        Ok(())
    }
}

fn delimiter<'a>(arg: &'a DefaultedStringArg, name: &str, none: char, vm: &VM<'_>) -> RunResult<Delimiter<'a>> {
    match arg {
        DefaultedStringArg::Omitted(text) => {
            let mut chars = text.chars();
            Ok(match (chars.next(), chars.next()) {
                (Some(c), None) => Delimiter::Char(c),
                _ => Delimiter::Text(text),
            })
        }
        DefaultedStringArg::Supplied(Value::None) => Ok(Delimiter::Char(none)),
        DefaultedStringArg::Supplied(value) if value.is_str(vm.heap) => Ok(Delimiter::Value(value)),
        DefaultedStringArg::Supplied(value) => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("{name} must be None or a string, not {}", value.py_type_name(vm)),
        )
        .into()),
    }
}
