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
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{HeapData, HeapGuard},
    os::{MontyPath, OpenCallArgs, OsFunctionCall},
    resource::ResourceTracker,
    types::{PyTrait, file::FileMode},
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
    let OpenArgs {
        file,
        mode,
        buffering,
        encoding,
        errors,
        newline,
        closefd,
        opener,
    } = OpenArgs::from_args(args, vm)?;

    // `file` and the unsupported kwargs are raw `Value`s; `mode` holds a
    // borrowed str — all need cleanup on every path.
    let mut file = HeapGuard::new(file, vm);
    let (file, vm) = file.as_parts_mut();
    defer_drop!(mode, vm);
    defer_drop!(buffering, vm);
    defer_drop!(encoding, vm);
    defer_drop!(errors, vm);
    defer_drop!(newline, vm);
    defer_drop!(closefd, vm);
    defer_drop!(opener, vm);

    // Reject non-default values for ignored kwargs so caller code can't
    // silently rely on (e.g.) `buffering=0` semantics Monty doesn't model.
    validate_ignored_open_kwarg("buffering", buffering, vm)?;
    validate_ignored_open_kwarg("encoding", encoding, vm)?;
    validate_ignored_open_kwarg("errors", errors, vm)?;
    validate_ignored_open_kwarg("newline", newline, vm)?;
    validate_ignored_open_kwarg("closefd", closefd, vm)?;
    validate_ignored_open_kwarg("opener", opener, vm)?;

    let path = extract_path_string(file, vm)?.to_owned();
    // Parse here purely to reject malformed modes before the OS round-trip;
    // the file wrapper itself is built from the host's returned FileHandle.
    let file_mode = mode
        .as_ref()
        .map_or("r", |m| m.as_str(vm))
        .parse::<FileMode>()
        .map_err(|e| RunError::from(SimpleException::new_msg(ExcType::ValueError, e)))?;

    Ok(CallResult::OsCall(OsFunctionCall::Open(OpenCallArgs {
        path: MontyPath::new(path),
        mode: file_mode,
    })))
}

/// Argument shape for `open(file, mode='r', buffering=-1, encoding=None,
/// errors=None, newline=None, closefd=True, opener=None)`.
///
/// `mode` is a zero-copy [`StrArg`] (absent → `"r"`) so wrong-type errors
/// flow through the macro's `bad_arg_named` path and match CPython's
/// `open() argument 'mode' must be str, not …` wording verbatim, without
/// copying the mode string. The other kwargs stay as raw `Value`
/// because they have monty-specific validation (`validate_ignored_open_kwarg`)
/// that the macro doesn't model — Monty rejects any *non-default* value to
/// avoid silently dropping semantics it doesn't honour (e.g. `buffering=0`,
/// `opener=my_opener`). `file` is also raw because `open()`'s file-path
/// error wording (`expected str, bytes or os.PathLike object, not …`) doesn't
/// follow the `_PyArg_BadArgument` shape that `bad_arg_named` emits.
#[derive(FromArgs)]
#[from_args(name = "open", bad_arg_named)]
struct OpenArgs {
    file: Value,
    #[from_args(default)]
    mode: Option<StrArg>,
    #[from_args(default = Value::Int(-1))]
    buffering: Value,
    #[from_args(default = Value::None)]
    encoding: Value,
    #[from_args(default = Value::None)]
    errors: Value,
    #[from_args(default = Value::None)]
    newline: Value,
    #[from_args(default = Value::Bool(true))]
    closefd: Value,
    #[from_args(default = Value::None)]
    opener: Value,
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

/// Decodes a byte path as strict UTF-8, raising `UnicodeDecodeError` on
/// invalid input.
///
/// # Divergence from CPython
///
/// CPython routes `bytes` paths through `os.fsdecode`, which on most hosts
/// uses UTF-8 with PEP 383 `surrogateescape` — invalid bytes become lone
/// surrogates `U+DC80`–`U+DCFF` so they round-trip back to the original
/// byte sequence. Monty rejects non-UTF-8 paths outright instead.
///
/// The choice is deliberate, not a "not yet implemented" gap:
///
/// 1. **Rust's `String` is strictly valid UTF-8.** Lone surrogates are not
///    Unicode scalar values, so they cannot live in a `String` without
///    `unsafe` code (or a parallel `Vec<u8>` path representation, which
///    would require refactoring `Path`, mount dispatch, and the host
///    boundary — see `crates/monty/src/types/path.rs`).
/// 2. **Monty paths are virtual POSIX strings**, not host-OS filenames.
///    The mount table maps them to real host paths only at the boundary;
///    there is no meaningful "filesystem encoding" to apply inside the
///    sandbox.
/// 3. **Hard rejection is predictable.** A lossy fallback (e.g.
///    `from_utf8_lossy`'s `U+FFFD` replacement) would not round-trip and
///    could silently re-route an `open()` call to a different file than
///    the caller asked for.
///
/// See `limitations/open.md` for the user-facing description.
fn decode_utf8_path(bytes: &[u8]) -> RunResult<Option<&str>> {
    match str::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Err(SimpleException::new_msg(ExcType::UnicodeDecodeError, "can't decode bytes path as UTF-8").into()),
    }
}

/// Validates `open()` kwargs that Monty does not actually honor.
///
/// Monty only models the `file` and `mode` arguments. Any other argument set
/// to a non-default value would silently be ignored if accepted, hiding bugs
/// in user code that passes (for example) `buffering=0` expecting an
/// unbuffered file or `opener=my_opener` expecting a custom open hook. To
/// avoid that, the only accepted values are the CPython defaults plus, for
/// `encoding`, the literal `"utf-8"` — which is what Monty already uses.
///
/// Non-default values raise `TypeError` ("'<name>' argument is not yet
/// supported"). A wrong *type* (e.g. `encoding=123`) is reported as a
/// dedicated type error so it remains diagnosable.
fn validate_ignored_open_kwarg(name: &str, value: &Value, vm: &VM<'_, impl ResourceTracker>) -> Result<(), RunError> {
    let is_default = match name {
        // CPython default is -1 (sentinel for "interpreter picks the
        // buffer size"). Monty has no buffering layer to tune.
        "buffering" => matches!(value, Value::Int(-1)),
        // None is the CPython default; "utf-8" is the encoding Monty
        // already uses, so accept it as a documented no-op.
        "encoding" => {
            if matches!(value, Value::None) {
                true
            } else if value.is_str(vm.heap) {
                let s = match value {
                    Value::InternString(id) => vm.interns.get_str(*id),
                    Value::Ref(id) => match vm.heap.get(*id) {
                        HeapData::Str(s) => s.as_str(),
                        _ => "",
                    },
                    _ => "",
                };
                s.eq_ignore_ascii_case("utf-8") || s.eq_ignore_ascii_case("utf8")
            } else {
                return Err(ExcType::type_error(format!(
                    "open() argument '{name}' must be str or None, not {}",
                    value.py_type(vm).cpython_arg_name()
                )));
            }
        }
        // `errors` and `newline` accept str or None in CPython; only the
        // default (None) is honored by Monty.
        "errors" | "newline" => {
            if matches!(value, Value::None) {
                true
            } else if value.is_str(vm.heap) {
                false
            } else {
                return Err(ExcType::type_error(format!(
                    "open() argument '{name}' must be str or None, not {}",
                    value.py_type(vm).cpython_arg_name()
                )));
            }
        }
        // CPython default is True; False requires int-fd open semantics
        // Monty does not model.
        "closefd" => matches!(value, Value::Bool(true)),
        // CPython default is None; a custom opener would run host-side code
        // outside the sandbox boundary, which Monty does not support.
        "opener" => matches!(value, Value::None),
        _ => unreachable!("validated open keyword name"),
    };
    if is_default {
        Ok(())
    } else {
        Err(ExcType::type_error(format!("'{name}' argument is not yet supported")))
    }
}

/// Creates the path type error used by `open()`.
fn path_type_error(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunError {
    ExcType::type_error(format!(
        "expected str, bytes or os.PathLike object, not {}",
        value.py_type(vm)
    ))
}
