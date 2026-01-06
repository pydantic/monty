//! Implementation of the print() builtin function.

use crate::{
    args::{ArgValues, KwargsValues},
    exception_private::{exc_err_fmt, exc_fmt, ExcType, RunError},
    heap::{Heap, HeapData},
    intern::Interns,
    io::PrintWriter,
    resource::ResourceTracker,
    run_frame::RunResult,
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
pub fn builtin_print(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
    print: &mut impl PrintWriter,
) -> RunResult<Value> {
    // Split into positional args and kwargs
    let (positional, kwargs) = args.split();

    // Extract kwargs first, consuming them - this handles cleanup on error
    let (sep, end) = match extract_print_kwargs(kwargs, heap, interns) {
        Ok(se) => se,
        Err(err) => {
            for value in positional {
                value.drop_with_heap(heap);
            }
            return Err(err);
        }
    };

    // Print positional args with separator
    let mut iter = positional.iter();
    if let Some(value) = iter.next() {
        print.stdout_write(value.py_str(heap, interns))?;
        for value in iter {
            if let Some(sep) = &sep {
                print.stdout_write(sep.as_str().into())?;
            } else {
                print.stdout_push(' ')?;
            }
            print.stdout_write(value.py_str(heap, interns))?;
        }
    }

    // Append end string
    if let Some(end) = end {
        print.stdout_write(end.into())?;
    } else {
        print.stdout_push('\n')?;
    }

    // Drop positional args
    for value in positional {
        value.drop_with_heap(heap);
    }

    Ok(Value::None)
}

/// Extracts sep and end kwargs from print() arguments.
///
/// Consumes the kwargs, dropping all values after extraction.
/// Returns (sep, end, error) where error is Some if a kwarg error occurred.
fn extract_print_kwargs(
    kwargs: KwargsValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Option<String>, Option<String>)> {
    let mut sep: Option<String> = None;
    let mut end: Option<String> = None;
    let mut error: Option<RunError> = None;

    for (key, value) in kwargs {
        // If we already hit an error, just drop remaining values
        if error.is_some() {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            continue;
        }

        let Some(keyword_name) = key.as_either_str(heap) else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            error = Some(exc_fmt!(ExcType::TypeError; "keywords must be strings").into());
            continue;
        };

        let key_str = keyword_name.as_str(interns);
        match key_str {
            "sep" => match extract_string_kwarg(&value, "sep", heap, interns) {
                Ok(custom_sep) => sep = custom_sep,
                Err(e) => error = Some(e),
            },
            "end" => match extract_string_kwarg(&value, "end", heap, interns) {
                Ok(custom_end) => end = custom_end,
                Err(e) => error = Some(e),
            },
            "flush" => {} // Accepted but ignored (we don't buffer output)
            "file" => {
                error = Some(exc_fmt!(ExcType::TypeError; "print() 'file' argument is not supported").into());
            }
            _ => {
                error = Some(
                    exc_fmt!(ExcType::TypeError; "'{}' is an invalid keyword argument for print()", key_str).into(),
                );
            }
        }
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }

    if let Some(error) = error {
        Err(error)
    } else {
        Ok((sep, end))
    }
}

/// Extracts a string value from a print() kwarg.
///
/// The kwarg can be None (returns empty string) or a string.
/// Raises TypeError for other types.
fn extract_string_kwarg(
    value: &Value,
    name: &str,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Option<String>> {
    match value {
        Value::None => Ok(None),
        Value::InternString(string_id) => Ok(Some(interns.get_str(*string_id).to_owned())),
        Value::Ref(id) => {
            if let HeapData::Str(s) = heap.get(*id) {
                return Ok(Some(s.as_str().to_owned()));
            }
            exc_err_fmt!(ExcType::TypeError; "{} must be None or a string, not {}", name, value.py_type(Some(heap)))
        }
        _ => {
            exc_err_fmt!(ExcType::TypeError; "{} must be None or a string, not {}", name, value.py_type(Some(heap)))
        }
    }
}
