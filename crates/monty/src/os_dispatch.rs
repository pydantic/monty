//! Interpreter-side plumbing for OS-level operations.
//!
//! [`OsFunctionCall`] itself (and its typed arg structs) lives in
//! `monty-types` so host crates can match on it without linking the
//! interpreter; it is re-exported here. Type methods and builtins return one
//! as [`CallResult::OsCall`](crate::bytecode::CallResult::OsCall); the VM
//! yields [`FrameExit::OsCall`](crate::bytecode::FrameExit::OsCall) so the
//! host decides whether to permit it. The interpreter itself never performs
//! I/O. This module keeps the `pathlib.Path` method dispatcher that builds
//! the calls from VM values.
//!
//! # Adding a new OS call
//!
//! Add a variant carrying a struct in `monty-types` (reuse
//! [`PathStringDataArgs`] etc. if the shape matches, derive `ToArgs` on the
//! struct, update [`OsFunctionCall::name`] and the other inherent methods),
//! add a matching typed arm to `monty.proto`'s `OsCall` and `monty-proto`'s
//! conversions, then wire the new variant into the fs/ dispatcher and any
//! host backends.

use monty_types::{
    ExcType, MkdirCallArgs, MontyPath, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs, RenameCallArgs,
    ResourceTracker,
};

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    exception_private::{ExcTypeExt, RunResult},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData},
    intern::{Interns, StaticStrings},
    value::Value,
};

impl<C: ContainsHeap> DropWithContext<C> for OsFunctionCall {
    // Owned args (String/Vec<u8>/bool/MontyPath/MontyObject) hold no live
    // heap references, so a plain drop is correct.
    fn drop_with(self, _heap: &mut C) {
        drop(self);
    }
}

// =============================================================================
// Path-method dispatcher (used by `types/path.rs`).
// =============================================================================

/// Pre-flight check for [`build_path_os_call`]: lets the caller decide whether
/// to commit ownership of the path/args to the builder.
#[must_use]
pub(crate) fn is_path_os_method(method: StaticStrings) -> bool {
    matches!(
        method,
        StaticStrings::Exists
            | StaticStrings::IsFile
            | StaticStrings::IsDir
            | StaticStrings::IsSymlink
            | StaticStrings::ReadText
            | StaticStrings::ReadBytes
            | StaticStrings::StatMethod
            | StaticStrings::Iterdir
            | StaticStrings::Resolve
            | StaticStrings::Absolute
            | StaticStrings::Unlink
            | StaticStrings::Rmdir
            | StaticStrings::WriteText
            | StaticStrings::AppendText
            | StaticStrings::WriteBytes
            | StaticStrings::AppendBytes
            | StaticStrings::Mkdir
            | StaticStrings::Rename
    )
}

/// Builds an [`OsFunctionCall`] for a `pathlib.Path` method invocation —
/// dispatches on `method` and pulls any extra args out of `args` into the
/// matching typed struct.
///
/// Returns `Ok(None)` if `method` isn't an OS call. Owns `path`/`args` and
/// is responsible for refcount cleanup on every code path.
pub(crate) fn build_path_os_call(
    method: StaticStrings,
    path: MontyPath,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<OsFunctionCall>> {
    // Simple "no extra args" path operations are bundled into one arm to avoid
    // 12 near-identical case lines.
    macro_rules! path_only {
        ($name:literal, $variant:ident) => {{
            args.check_zero_args($name, vm.heap)?;
            OsFunctionCall::$variant(path)
        }};
    }

    let call = match method {
        StaticStrings::Exists => path_only!("exists", Exists),
        StaticStrings::IsFile => path_only!("is_file", IsFile),
        StaticStrings::IsDir => path_only!("is_dir", IsDir),
        StaticStrings::IsSymlink => path_only!("is_symlink", IsSymlink),
        StaticStrings::ReadText => path_only!("read_text", ReadText),
        StaticStrings::ReadBytes => path_only!("read_bytes", ReadBytes),
        StaticStrings::StatMethod => path_only!("stat", Stat),
        StaticStrings::Iterdir => path_only!("iterdir", Iterdir),
        StaticStrings::Resolve => path_only!("resolve", Resolve),
        StaticStrings::Absolute => path_only!("absolute", Absolute),
        StaticStrings::Unlink => path_only!("unlink", Unlink),
        StaticStrings::Rmdir => path_only!("rmdir", Rmdir),
        StaticStrings::WriteText => {
            OsFunctionCall::WriteText(extract_str_data("write_text", path, args, vm.heap, vm.interns)?)
        }
        StaticStrings::AppendText => {
            OsFunctionCall::AppendText(extract_str_data("append_text", path, args, vm.heap, vm.interns)?)
        }
        StaticStrings::WriteBytes => {
            OsFunctionCall::WriteBytes(extract_bytes_data("write_bytes", path, args, vm.heap, vm.interns)?)
        }
        StaticStrings::AppendBytes => {
            OsFunctionCall::AppendBytes(extract_bytes_data("append_bytes", path, args, vm.heap, vm.interns)?)
        }
        StaticStrings::Mkdir => OsFunctionCall::Mkdir(extract_mkdir_args(path, args, vm)?),
        StaticStrings::Rename => OsFunctionCall::Rename(extract_rename_args(path, args, vm.heap, vm.interns)?),
        _ => {
            // Unreachable in practice — callers gate on `is_path_os_method`.
            // Drop the owned inputs anyway so a stray call doesn't leak refs.
            let _ = path;
            args.drop_with(vm.heap);
            return Ok(None);
        }
    };
    Ok(Some(call))
}

/// Extracts the `data` arg for `write_text` / `append_text`. Error wording
/// matches the legacy `fs/` dispatcher so existing tests stay green.
fn extract_str_data(
    method: &'static str,
    path: MontyPath,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<PathStringDataArgs> {
    let data = arg_or_missing_data(method, args, heap)?;
    let data_str = value_to_owned_string(&data, heap, interns);

    let py_type = data.py_type_name_heap(heap, interns);
    data.drop_with(heap);

    match data_str {
        Some(data) => Ok(PathStringDataArgs { path, data }),
        None => Err(ExcType::type_error(format!("data must be str, not {py_type}"))),
    }
}

/// Extracts the `data` arg for `write_bytes` / `append_bytes` — binary
/// companion to [`extract_str_data`].
fn extract_bytes_data(
    method: &'static str,
    path: MontyPath,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<PathBytesDataArgs> {
    let data = arg_or_missing_data(method, args, heap)?;
    let bytes = value_to_owned_bytes(&data, heap, interns);

    let py_type = data.py_type_name_heap(heap, interns);
    data.drop_with(heap);

    match bytes {
        Some(data) => Ok(PathBytesDataArgs { path, data }),
        None => Err(ExcType::type_error(format!(
            "memoryview: a bytes-like object is required, not '{py_type}'"
        ))),
    }
}

/// Python-facing argument shape for `Path.mkdir(mode=0o777, parents=False, exist_ok=False)`.
///
/// `Path.mkdir` is a pure-Python `def` in CPython, hence `style = def` (its
/// duplicate-arg error is `got multiple values for argument`). The
/// too-many-positional count still diverges: CPython counts the bound `self`
/// (`takes from 1 to 4 …`), Monty does not — see `limitations/open.md`.
///
/// Monty parses `mode` for signature compatibility and arity validation, but
/// filesystem backends do not model POSIX permission bits. `parents` and
/// `exist_ok` use [`LaxBool`] so they accept any truth-tested value (matching
/// CPython, which evaluates them via `bool()`).
#[derive(FromArgs)]
#[from_args(name = "Path.mkdir", style = def)]
struct PathMkdirArgs {
    #[from_args(default = 0o777_i64)]
    mode: i64,
    #[from_args(default = LaxBool::new(false))]
    parents: LaxBool,
    #[from_args(default = LaxBool::new(false))]
    exist_ok: LaxBool,
}

/// Extracts `mode`/`parents`/`exist_ok` for `mkdir`, rejecting unknown or
/// excessive arguments before the host sees the OS call.
fn extract_mkdir_args(
    path: MontyPath,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<MkdirCallArgs> {
    let PathMkdirArgs {
        mode,
        parents,
        exist_ok,
    } = PathMkdirArgs::from_args(args, vm)?;
    let _ = mode;
    Ok(MkdirCallArgs {
        path,
        parents: parents.bool(),
        exist_ok: exist_ok.bool(),
    })
}

/// Extracts the `target` arg for `Path.rename(target)`.
fn extract_rename_args(
    src: MontyPath,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<RenameCallArgs> {
    let target = args.get_one_arg("rename", heap)?;
    let dst_str = value_to_owned_string(&target, heap, interns);
    target.drop_with(heap);
    match dst_str {
        Some(dst) => Ok(RenameCallArgs {
            src,
            dst: MontyPath::new(dst),
        }),
        None => Err(ExcType::type_error(
            "Path.rename() argument 'target' must be str or Path".to_owned(),
        )),
    }
}

/// Pulls the single `data` arg out of `args`, raising the CPython-style
/// `missing 1 required positional argument: 'data'` error when absent.
fn arg_or_missing_data(
    method: &'static str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    if matches!(args, ArgValues::Empty) {
        return Err(ExcType::type_error(format!(
            "Path.{method}() missing 1 required positional argument: 'data'"
        )));
    }
    args.get_one_arg(method, heap)
}

/// Owned `String` if `value` is a `str` or `Path`, else `None`. Caller drops
/// the source value afterwards.
fn value_to_owned_string(value: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<String> {
    match value {
        Value::InternString(id) => Some(interns.get_str(*id).to_owned()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str().to_owned()),
            HeapData::Path(p) => Some(p.as_str().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// Owned `Vec<u8>` if `value` is a `bytes` (interned or heap), else `None`.
fn value_to_owned_bytes(value: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<Vec<u8>> {
    match value {
        Value::InternBytes(id) => Some(interns.get_bytes(*id).to_owned()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Bytes(b) => Some(b.as_slice().to_owned()),
            _ => None,
        },
        _ => None,
    }
}
