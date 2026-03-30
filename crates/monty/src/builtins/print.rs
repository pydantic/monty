//! Implementation of the print() builtin function.

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
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
/// - `flush`: whether to flush the stream (accepted but ignored)
///
/// The `file` kwarg is not supported.
pub fn builtin_print(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // Split into positional args and kwargs
    let (positional, kwargs) = args.into_parts();
    defer_drop!(positional, vm);

    // Extract kwargs first
    let (sep, end) = extract_print_kwargs(kwargs, vm)?;

    // Print positional args with separator, dropping each value after use
    let mut first = true;
    for value in positional.as_slice() {
        if first {
            first = false;
        } else if let Some(sep) = &sep {
            vm.print_writer.stdout_write(sep.as_str().into())?;
        } else {
            vm.print_writer.stdout_push(' ')?;
        }
        vm.print_writer.stdout_write(value.py_str(vm)?)?;
    }

    // Append end string
    if let Some(end) = end {
        vm.print_writer.stdout_write(end.into())?;
    } else {
        vm.print_writer.stdout_push('\n')?;
    }

    Ok(Value::None)
}

/// Extracts sep and end kwargs from print() arguments.
///
/// Consumes the kwargs, dropping all values after extraction.
/// Returns (sep, end) where each is Some if provided.
fn extract_print_kwargs(
    kwargs: KwargsValues,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(Option<String>, Option<String>)> {
    let mut sep: Option<String> = None;
    let mut end: Option<String> = None;
    let mut error: Option<RunError> = None;

    for (key, value) in kwargs {
        // defer_drop! ensures key and value are cleaned up on every path through
        // the loop body — including continue, early return, and normal iteration
        defer_drop!(key, vm);
        defer_drop!(value, vm);

        // If we already hit an error, just drop remaining values
        if error.is_some() {
            continue;
        }

        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            error = Some(SimpleException::new_msg(ExcType::TypeError, "keywords must be strings").into());
            continue;
        };

        let key_str = keyword_name.as_str(vm.interns);
        match key_str {
            "sep" => match extract_string_kwarg(value, "sep", vm) {
                Ok(custom_sep) => sep = custom_sep,
                Err(e) => error = Some(e),
            },
            "end" => match extract_string_kwarg(value, "end", vm) {
                Ok(custom_end) => end = custom_end,
                Err(e) => error = Some(e),
            },
            "flush" => {} // Accepted but ignored (we don't buffer output)
            "file" => {
                error = Some(
                    SimpleException::new_msg(ExcType::TypeError, "print() 'file' argument is not supported").into(),
                );
            }
            _ => {
                error = Some(ExcType::type_error_unexpected_keyword("print", key_str));
            }
        }
    }

    if let Some(error) = error {
        Err(error)
    } else {
        Ok((sep, end))
    }
}

/// Extracts a string value from a print() kwarg.
///
/// The kwarg can be None (returns None) or a string (returns Some).
/// Raises TypeError for other types.
fn extract_string_kwarg(value: &Value, name: &str, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<String>> {
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
