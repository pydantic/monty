//! Implementation of the `open()` builtin.
//!
//! `open()` itself allocates no heap object. It validates its arguments and
//! yields an [`OsFunction::Open`] OS call; the host performs the open-time
//! effect (truncate / create / existence-check) and returns a
//! [`MontyNode::FileHandle`](monty_types::MontyNode::FileHandle), which the
//! resume path converts into the heap [`OpenFile`](crate::types::OpenFile)
//! wrapper and attaches the original filename and `newline` mode. `read()`/`write()` use the host's
//! returned path for full-file OS calls, so filesystem access remains behind `OsFunction`.

use std::str;

use monty_macros::py_signature;
use monty_types::{MontyPath, OpenCallArgs, OsFunctionCall};

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    os_dispatch::{PostConversionEffect, value_as_path_str},
    py_value::PyValue,
    types::{
        PyTrait, Type,
        file::{FileMode, FileName},
    },
    value::Value,
};

/// Opens a file for reading, writing, or appending.
///
/// The host performs open-time validation and creation, then returns a file handle.
/// The resume effect preserves the supplied filename for `.name` and `repr()`;
/// subsequent I/O uses the handle's path, independent of later directory changes.
#[py_signature(
    "open(file, mode: str=..., buffering=-1, encoding: str | None=None, errors: str | None=None, newline: str | None=None, closefd=True, opener=None)",
    bad_arg_named
)]
pub(crate) fn builtin_open(vm: &mut VM<'_>, params: &OpenArgs) -> RunResult<CallResult> {
    if !matches!(&params.buffering, Value::Int(-1)) {
        return Err(unsupported_kwarg("buffering"));
    }
    if let Some(encoding) = &params.encoding {
        let text = encoding.as_str(vm);
        if !text.eq_ignore_ascii_case("utf-8") && !text.eq_ignore_ascii_case("utf8") && text != "locale" {
            return Err(unsupported_kwarg("encoding"));
        }
    }
    if params.errors.is_some() {
        return Err(unsupported_kwarg("errors"));
    }
    if !matches!(&params.closefd, Value::Bool(true)) {
        return Err(unsupported_kwarg("closefd"));
    }
    if !matches!(&params.opener, Value::None) {
        return Err(unsupported_kwarg("opener"));
    }

    let path = extract_path_string(&params.file, vm)?;
    // Parse here purely to reject malformed modes before the OS round-trip;
    // the file wrapper itself is built from the host's returned FileHandle.
    let file_mode = params
        .mode
        .as_ref()
        .map_or("r", |m| m.as_str(vm))
        .parse::<FileMode>()
        .map_err(|e| RunError::from(SimpleException::new_msg(ExcType::ValueError, e)))?;
    // CPython rejects this pairing before opening; the value itself it checks
    // only after (see `apply_open_name`).
    if file_mode.is_binary() && params.newline.is_some() {
        return Err(ExcType::value_error("binary mode doesn't take a newline argument"));
    }
    if vm.env.legacy_binary_writes && matches!(file_mode, FileMode::WriteUpdate(true)) {
        return Err(ExcType::not_implemented("binary update mode requires positional filesystem writes").into());
    }

    let name = if params.file.py_type(vm) == Type::Bytes {
        FileName::Bytes(path.as_bytes().to_vec())
    } else {
        FileName::Str(path.clone())
    };
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Open(OpenCallArgs {
            path: MontyPath::new(path),
            mode: file_mode,
        }),
        effect: PostConversionEffect::OpenName {
            name,
            newline: params.newline.as_ref().map(|s| s.as_str(vm).to_owned()),
        }
        .into(),
    })
}

/// Extracts a path string accepted by `open()`.
///
/// Accepts `str` (interned or heap), `bytes` (UTF-8 decoded), `PurePosixPath`
/// and `os.DirEntry`. The error message mentions `os.PathLike` to match
/// CPython, even though full PathLike support is limited to the variants
/// listed above.
fn extract_path_string(value: &Value, vm: &VM<'_>) -> RunResult<String> {
    let path = match value.py_value(vm) {
        PyValue::Str(text) => Some(text.as_str(vm).to_owned()),
        PyValue::Bytes(bytes) => decode_utf8_path(bytes.as_slice(vm))?.map(str::to_owned),
        _ => value_as_path_str(value, vm.heap, vm.interns).map(str::to_owned),
    };
    path.ok_or_else(|| path_type_error(value, vm))
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

/// Creates the path type error used by `open()`.
fn path_type_error(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error_fspath(&value.py_type_name(vm))
}

fn unsupported_kwarg(name: &str) -> RunError {
    ExcType::type_error(format!("'{name}' argument is not yet supported"))
}
