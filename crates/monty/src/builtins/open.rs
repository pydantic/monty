//! Implementation of the `open()` builtin.
//!
//! `open()` itself allocates no heap object. It validates its arguments and
//! yields an [`OsFunction::Open`] OS call; the host performs the open-time
//! effect (truncate / create / existence-check) and returns a
//! [`MontyObject::FileHandle`](crate::MontyObject::FileHandle), which the
//! generic resume path converts into the heap [`OpenFile`](crate::types::OpenFile)
//! wrapper. `read()`/`write()` then delegate to full-file OS calls, so all
//! filesystem access remains behind `OsFunction`.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData, HeapGuard},
    intern::StringId,
    object::FileMode,
    os::OsFunction,
    resource::ResourceTracker,
    types::{PyTrait, str::allocate_string},
    value::Value,
};

/// Opens a file for reading, writing, or appending.
///
/// `open()` validates its arguments and the mode string, then returns a
/// [`CallResult::OsCall`] for [`OsFunction::Open`] with arguments
/// `[path, mode]`. The host performs the open-time effect — truncate for
/// `w`/`w+`, create-if-missing for `a`/`a+`, existence check (raising
/// `FileNotFoundError`) for `r`/`r+` — and returns a `MontyObject::FileHandle`.
/// The generic resume path converts that into the `OpenFile` heap wrapper, so
/// `open()` needs no special resume handling.
pub(crate) fn builtin_open(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
    let OpenArgs { file, mode } = parse_open_args(args, vm)?;
    defer_drop!(file, vm);
    defer_drop!(mode, vm);
    let path = extract_path_string(file, vm)?.to_owned();
    let mode_str = extract_mode_string(mode, vm)?;
    // Parse here purely to reject malformed modes before the OS round-trip;
    // the file wrapper itself is built from the host's returned FileHandle.
    let file_mode = mode_str
        .parse::<FileMode>()
        .map_err(|e| RunError::from(SimpleException::new_msg(ExcType::ValueError, e)))?;

    let path_value = allocate_string(path, vm.heap)?;
    let mode_value = allocate_string(file_mode.as_str().to_owned(), vm.heap)?;
    Ok(CallResult::OsCall(
        OsFunction::Open,
        ArgValues::Two(path_value, mode_value),
    ))
}

/// Owned `open()` arguments after positional/keyword parsing.
struct OpenArgs {
    file: Value,
    mode: Value,
}

/// Parses `open(file, mode='r', ...)` arguments.
///
/// Unsupported optional arguments are accepted only when they are semantically
/// neutral for Monty's UTF-8, one-shot file wrappers.
fn parse_open_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<OpenArgs> {
    let (mut pos, kwargs) = args.into_parts();
    if pos.len() > 2 {
        let count = pos.len();
        pos.drop_with_heap(vm);
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("open", 2, count));
    }

    let mut file = pos.next();
    let positional_mode = pos.next();
    let mut mode_was_provided = positional_mode.is_some();
    let mut mode = positional_mode.unwrap_or(Value::InternString(StringId::from_ascii(b'r')));
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);

    for (key, value) in kwargs_iter {
        defer_drop!(key, vm);
        let mut value = HeapGuard::new(value, vm);
        let Some(keyword) = key.as_either_str(value.heap().heap) else {
            file.drop_with_heap(value.heap());
            mode.drop_with_heap(value.heap());
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let keyword = keyword.as_str(value.heap().interns).to_owned();
        match keyword.as_str() {
            "file" => {
                if file.is_some() {
                    file.drop_with_heap(value.heap());
                    mode.drop_with_heap(value.heap());
                    return Err(ExcType::type_error_multiple_values("open", "file"));
                }
                file = Some(value.into_inner());
            }
            "mode" => {
                if mode_was_provided {
                    file.drop_with_heap(value.heap());
                    mode.drop_with_heap(value.heap());
                    return Err(ExcType::type_error_multiple_values("open", "mode"));
                }
                mode = value.into_inner();
                mode_was_provided = true;
            }
            "buffering" | "encoding" | "errors" | "newline" => {
                let result = {
                    let (value, vm) = value.as_parts();
                    validate_ignored_open_kwarg(&keyword, value, vm)
                };
                if let Err(err) = result {
                    file.drop_with_heap(value.heap());
                    mode.drop_with_heap(value.heap());
                    return Err(err);
                }
            }
            other => {
                file.drop_with_heap(value.heap());
                mode.drop_with_heap(value.heap());
                return Err(ExcType::type_error_unexpected_keyword("open", other));
            }
        }
    }

    let Some(file) = file else {
        mode.drop_with_heap(vm);
        return Err(ExcType::type_error_missing_positional_with_names("open", &["file"]));
    };

    Ok(OpenArgs { file, mode })
}

/// Extracts a path string accepted by `open()`.
fn extract_path_string<'a>(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
    match value {
        Value::InternString(string_id) => Ok(vm.interns.get_str(*string_id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Ok(s.as_str()),
            HeapData::Path(p) => Ok(p.as_str()),
            _ => Err(path_type_error(value, vm)),
        },
        _ => Err(path_type_error(value, vm)),
    }
}

/// Extracts the optional mode string.
fn extract_mode_string<'a>(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
    match value {
        Value::InternString(string_id) => Ok(vm.interns.get_str(*string_id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Ok(s.as_str()),
            _ => Err(ExcType::type_error(format!(
                "open() argument 'mode' must be str, not {}",
                value.py_type(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "open() argument 'mode' must be str, not {}",
            value.py_type(vm)
        ))),
    }
}

/// Validates `open()` kwargs that Monty accepts but does not otherwise model.
fn validate_ignored_open_kwarg(name: &str, value: &Value, vm: &VM<'_, impl ResourceTracker>) -> Result<(), RunError> {
    match name {
        "buffering" => Ok(()),
        "encoding" | "errors" | "newline" if matches!(value, Value::None) || value.is_str(vm.heap) => Ok(()),
        "encoding" | "errors" | "newline" => Err(ExcType::type_error(format!(
            "open() argument '{name}' must be str or None, not {}",
            value.py_type(vm)
        ))),
        _ => unreachable!("validated open keyword name"),
    }
}

/// Creates the path type error used by `open()`.
fn path_type_error(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunError {
    ExcType::type_error(format!(
        "expected str, bytes or os.PathLike object, not {}",
        value.py_type(vm)
    ))
}
