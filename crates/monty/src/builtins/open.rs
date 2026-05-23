//! Implementation of the `open()` builtin.
//!
//! `open()` itself allocates no heap object. It validates its arguments and
//! yields an [`OsFunction::Open`] OS call; the host performs the open-time
//! effect (truncate / create / existence-check) and returns a
//! [`MontyObject::FileHandle`](crate::MontyObject::FileHandle), which the
//! generic resume path converts into the heap [`OpenFile`](crate::types::OpenFile)
//! wrapper. `read()`/`write()` then delegate to full-file OS calls, so all
//! filesystem access remains behind `OsFunction`.

use std::str;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData, HeapGuard},
    intern::StringId,
    os::OsFunction,
    resource::ResourceTracker,
    types::{PyTrait, file::FileMode, str::allocate_string},
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
/// Accepts the full CPython positional signature
/// `open(file, mode, buffering, encoding, errors, newline, closefd, opener)`
/// — up to 8 positional args — even though Monty only models `file` and
/// `mode`. The other six are passed through [`validate_ignored_open_kwarg`]
/// and discarded, matching how kwargs of the same names are handled.
fn parse_open_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<OpenArgs> {
    let (mut pos, kwargs) = args.into_parts();
    if pos.len() > 8 {
        let count = pos.len();
        pos.drop_with_heap(vm);
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("open", 8, count));
    }

    let mut file = pos.next();
    let positional_mode = pos.next();
    let mut mode_was_provided = positional_mode.is_some();
    let mut mode = positional_mode.unwrap_or(Value::InternString(StringId::from_ascii(b'r')));

    // Drain positional args 3..=8 through the ignored-kwarg validator, mapped
    // to their CPython kwarg names. Each value is wrapped in a HeapGuard so
    // it is freed on every path — including the validator's error path.
    for (i, value) in pos.enumerate() {
        let name = POSITIONAL_KWARG_NAMES[i];
        let mut value = HeapGuard::new(value, vm);
        let result = {
            let (value, vm) = value.as_parts();
            validate_ignored_open_kwarg(name, value, vm)
        };
        if let Err(err) = result {
            file.drop_with_heap(value.heap());
            mode.drop_with_heap(value.heap());
            return Err(err);
        }
    }

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
            "buffering" | "encoding" | "errors" | "newline" | "closefd" | "opener" => {
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
///
/// Accepts `str` (interned or heap), `bytes` (UTF-8 decoded), and
/// `PurePosixPath`. The error message mentions `os.PathLike` to match
/// CPython, even though full PathLike support is limited to the variants
/// listed above.
fn extract_path_string<'a>(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
    let opt = match value {
        Value::InternString(string_id) => Some(vm.interns.get_str(*string_id)),
        Value::InternBytes(bytes_id) => decode_utf8_path(vm.interns.get_bytes(*bytes_id))?,
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str()),
            HeapData::Path(p) => Some(p.as_str()),
            HeapData::Bytes(b) => decode_utf8_path(b.as_slice())?,
            _ => None,
        },
        _ => None,
    };
    opt.ok_or_else(|| path_type_error(value, vm))
}

/// Decodes a byte path to UTF-8, raising a CPython-shaped error on invalid input.
fn decode_utf8_path(bytes: &[u8]) -> RunResult<Option<&str>> {
    match str::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Err(SimpleException::new_msg(ExcType::UnicodeDecodeError, "can't decode bytes path as UTF-8").into()),
    }
}

/// Extracts the optional mode string.
fn extract_mode_string<'a>(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
    let opt = match value {
        Value::InternString(string_id) => Some(vm.interns.get_str(*string_id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    };
    opt.ok_or_else(|| ExcType::type_error(format!("open() argument 'mode' must be str, not {}", value.py_type(vm))))
}

/// CPython kwarg names for positional `open()` arguments past `mode`. Indexed
/// from 0 = "buffering" (the 3rd positional) through 5 = "opener" (the 8th).
const POSITIONAL_KWARG_NAMES: [&str; 6] = ["buffering", "encoding", "errors", "newline", "closefd", "opener"];

/// Validates `open()` kwargs that Monty accepts but does not otherwise model.
///
/// `buffering`, `closefd`, and `opener` are accepted with any value (the
/// surface area where CPython rejects specific values — e.g. `closefd=False`
/// without an int fd, or a non-callable `opener` — is not modelled because
/// Monty has no int-fd or custom-open mechanism for those refinements to
/// interact with).
fn validate_ignored_open_kwarg(name: &str, value: &Value, vm: &VM<'_, impl ResourceTracker>) -> Result<(), RunError> {
    match name {
        "buffering" | "closefd" | "opener" => Ok(()),
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
