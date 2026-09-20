//! Interpreter-side plumbing for OS-level operations.
//!
//! [`OsFunctionCall`] itself (and its typed arg structs) lives in
//! `monty-types` so host crates can match on it without linking the
//! interpreter; it is re-exported here. Type methods and builtins return one
//! as [`CallResult::OsCall`](crate::bytecode::CallResult::OsCall); the VM
//! yields [`FrameExit::OsCall`](crate::bytecode::FrameExit::OsCall) so the
//! host decides whether to permit it. The interpreter itself never performs
//! I/O. This module keeps the `pathlib.Path` method dispatcher that builds
//! the calls from VM values, plus [`PendingEffect`] — the VM-side hook that
//! post-processes an OS-call result on resume.
//!
//! # Adding a new OS call
//!
//! Add a variant carrying a struct in `monty-types` (reuse
//! [`PathStringDataArgs`] etc. if the shape matches, derive `ToArgs` on the
//! struct, update [`OsFunctionCall::name`] and the other inherent methods),
//! add a matching typed arm to `monty.proto`'s `OsCall` and `monty-proto`'s
//! conversions, then wire the new variant into the fs/ dispatcher and any
//! host backends.

use std::{borrow::Cow, mem};

use ahash::AHashSet;
use monty_types::{
    ExcType, MkdirCallArgs, MontyObject, MontyPath, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
    RenameCallArgs, ResourceTracker, normalize_virtual_path,
    unstable::{self, MontyNode},
};

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::{random::RandomRetry, time::ClockReading},
    types::{Path, file::FileName, random::RandomTarget},
    value::Value,
    virtual_path::posix_join,
};

impl<C: ContainsHeap> DropWithContext<C> for OsFunctionCall {
    // Owned args (String/Vec<u8>/bool/MontyPath/MontyObject) hold no live
    // heap references, so a plain drop is correct.
    fn drop_with(self, _heap: &mut C) {
        drop(self);
    }
}

/// Work the VM must perform on the result of a paused OS call when it
/// resumes, instead of pushing the raw host value onto the operand stack.
///
/// The two stages are separate types because they run on different data:
/// [`Pre`](Self::Pre) on the raw [`MontyObject`] before heap conversion,
/// [`Post`](Self::Post) on the converted [`Value`] after it. Rides inside the
/// suspension value — [`CallResult::OsCallWithEffect`], then
/// [`FrameExit::OsCall`](crate::bytecode::FrameExit) — and is armed on the
/// VM's single slot (one call in flight per task) only once the call reaches
/// the host, where a `resume` becomes guaranteed; anything discarding the
/// suspension calls [`release_pending_effect`] instead.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PendingEffect {
    /// Reshapes the host's reply before it is converted to a heap value.
    Pre(PreConversionEffect),
    /// Applies the converted value to VM state, possibly pinning a file.
    Post(PostConversionEffect),
}

impl PendingEffect {
    /// Operations whose result `VM::resume` must postprocess before execution
    /// continues, named for the `RuntimeError` a host gets for answering them
    /// with a future (which bypasses `resume`).
    pub(crate) fn immediate_result_name(&self) -> Option<&'static str> {
        match self {
            Self::Pre(effect) => Some(effect.operation_name()),
            Self::Post(PostConversionEffect::OpenName { .. }) => Some("open"),
            Self::Post(PostConversionEffect::SeedRandom { .. }) => Some("os.urandom"),
            // `time.sleep` blocks by definition, so a future would leave the
            // sandbox running before the wait it asked for finished.
            Self::Post(PostConversionEffect::DiscardResult) => Some("time.sleep"),
            // The reading still has to become a `struct_time` or a string, which
            // a future defers past the point the sandbox needs it.
            Self::Post(PostConversionEffect::ClockReading { .. }) => Some("time.time"),
            // A future strands these instead: the awaited value is the raw host reply.
            Self::Post(PostConversionEffect::BufferStore { .. } | PostConversionEffect::WritePosition { .. }) => None,
            // `asyncio.sleep` wants the future: `resume_with_result` moves the
            // result onto the pending awaitable instead.
            Self::Post(PostConversionEffect::SleepResult { .. }) => None,
        }
    }

    /// Discards an effect that will never be applied, releasing whatever it
    /// held across the host yield (see [`PostConversionEffect::release`]).
    pub(crate) fn release(self, heap: &mut impl ContainsHeap) {
        match self {
            Self::Pre(_) => {}
            Self::Post(effect) => effect.release(heap),
        }
    }
}

impl From<PreConversionEffect> for PendingEffect {
    fn from(effect: PreConversionEffect) -> Self {
        Self::Pre(effect)
    }
}

impl From<PostConversionEffect> for PendingEffect {
    fn from(effect: PostConversionEffect) -> Self {
        Self::Post(effect)
    }
}

/// Reshapes the raw host reply before heap conversion: plain data in, plain
/// data out, so no variant holds a heap reference and none needs cleanup.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PreConversionEffect {
    /// `os.listdir`: reduce the host's `Iterdir` result (a list of child
    /// paths) to the list of bare entry names.
    ListdirNames,
    /// `os.chdir`: the target was sent to the host as a `Path.stat` call;
    /// normalize and adopt `path` once the reply proves it is a directory.
    Chdir {
        /// Unnormalized absolute target, retained until the host validates it.
        path: String,
        /// The path as the caller spelled it, for `NotADirectoryError` (CPython
        /// reports the argument, not the resolved path).
        spelled: String,
    },
    /// Rebuild directory entries beneath the caller's original `Path`.
    IterdirPaths { path: String },
    /// `os.urandom(size)`: the reply must be `bytes` of exactly `size`, so a
    /// handler cannot hand the sandbox more than it asked (and preflighted) for.
    UrandomLength { size: usize },
}

impl PreConversionEffect {
    /// Applies the effect to the host's reply, yielding the value the VM
    /// imports and pushes; `Chdir` adopts the directory as a side effect.
    pub(crate) fn reshape(self, value: MontyObject, vm: &mut VM<'_>) -> Result<MontyObject, RunError> {
        match self {
            Self::ListdirNames => listdir_names(value),
            Self::IterdirPaths { path } => iterdir_paths(value, &path, &vm.heap.tracker),
            Self::UrandomLength { size } => urandom_reply(value, size),
            Self::Chdir { path, spelled } => {
                check_chdir_stat(&value, &spelled)?;
                vm.env.cwd = Cow::Owned(normalize_virtual_path(&path).into_owned());
                Ok(MontyObject::none())
            }
        }
    }

    /// The Python operation this effect completes, for error messages.
    fn operation_name(&self) -> &'static str {
        match self {
            Self::ListdirNames => "os.listdir",
            Self::Chdir { .. } => "os.chdir",
            Self::IterdirPaths { .. } => "Path.iterdir",
            Self::UrandomLength { .. } => "os.urandom",
        }
    }
}

/// Applies the converted host value to VM state. The file variants and
/// `SeedRandom`'s instance target own a reference to their heap object across
/// the host yield (see `inc_ref_for_pending_oscall`) and `SleepResult` owns its
/// value; each is released exactly once — on apply, or via [`Self::release`]
/// when the effect is discarded.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PostConversionEffect {
    /// Store a full-file read result into the file buffer, then compute the
    /// pending read/seek slice (see `types/file.rs`).
    BufferStore { file_id: HeapId },
    /// Advance the file's logical position by the successful write result.
    WritePosition {
        /// File whose position is updated.
        file_id: HeapId,
        /// Position before the write was dispatched, used to restore state if
        /// the host raises before returning a count.
        previous_position: u64,
        /// Known file length before dispatch, restored on host exception.
        previous_length: u64,
    },
    /// Preserve `open()`'s filename while the returned handle supplies the I/O target.
    OpenName { name: FileName },
    /// Seed a `random` generator from the host's `os.urandom` reply, then
    /// answer `None` (`seed()`) or re-run the draw that found it unseeded
    /// (`retry`, which owns the call's arguments across the yield).
    SeedRandom {
        target: RandomTarget,
        retry: Option<RandomRetry>,
    },
    /// Drop the host's answer and evaluate to `None` (`time.sleep`, whose
    /// CPython return value is always `None`).
    DiscardResult,
    /// Turn the host's `time.time` answer into what the `time` function that
    /// asked for it returns — a `struct_time`, a formatted string or
    /// nanoseconds. Holds no heap reference, so nothing to release.
    ClockReading { reading: ClockReading },
    /// Drop the host's answer and produce `result` from an awaitable, so
    /// `asyncio.sleep(delay, result)` is awaitable whether the host answered
    /// immediately (a settled awaitable) or with a future (the pending
    /// awaitable takes `result` over; see `ExternalFuture::sleep_result`).
    /// Owns `result`'s reference; released by [`release_pending_effect`].
    SleepResult { result: Value },
}

impl PostConversionEffect {
    /// Releases what the effect held across the yield: the pinned heap object
    /// (a file handle, or a `random.Random` instance), any stashed arguments
    /// and a sleep's result. The single place that knows which variants carry a refcount.
    pub(crate) fn release(self, heap: &mut impl ContainsHeap) {
        match self {
            Self::BufferStore { file_id } | Self::WritePosition { file_id, .. } => heap.heap_mut().dec_ref(file_id),
            Self::OpenName { .. } | Self::DiscardResult | Self::ClockReading { .. } => {}
            Self::SleepResult { result } => result.drop_with(heap),
            Self::SeedRandom { target, retry } => {
                if let RandomTarget::Instance(id) = target {
                    heap.heap_mut().dec_ref(id);
                }
                retry.drop_with(heap);
            }
        }
    }
}

/// Releases an effect that will never be resumed, dropping the heap pin,
/// arguments or sleep result it carried (see `inc_ref_for_pending_oscall`).
///
/// Reached via the owner's `drop_with`, or `Drop for VM` once the effect is
/// armed and no owning value remains.
pub(crate) fn release_pending_effect(effect: Option<PendingEffect>, heap: &mut impl ContainsHeap) {
    if let Some(effect) = effect {
        effect.release(heap);
    }
}

/// Resolves every relative path in `call` against `cwd` (see [`posix_join`]).
///
/// Runs at the VM's single OS-call exit, so builtins and `Path` methods can
/// hand over paths exactly as the user wrote them. Absolute and empty paths
/// are left untouched, not copied.
pub(crate) fn resolve_call_paths(call: &mut OsFunctionCall, cwd: &str) {
    for path in call.fs_paths_mut() {
        if !path.is_empty() && !path.starts_with('/') {
            *path = MontyPath::new(posix_join(cwd, path));
        }
    }
}

/// Checks a host `Path.stat` reply for `os.chdir` — the resume half of
/// [`PreConversionEffect::Chdir`], run on the raw [`MontyObject`] before heap
/// import like [`listdir_names`].
///
/// A directory `st_mode` passes and the caller normalizes and adopts the path (`os.chdir`
/// returns `None`); a file raises `NotADirectoryError` naming `spelled`, the
/// path as the caller wrote it. Hosts that answered `Path.stat` with
/// something other than a stat result get the same `RuntimeError` shape as
/// `os.listdir`.
pub(crate) fn check_chdir_stat(value: &MontyObject, spelled: &str) -> Result<(), RunError> {
    const S_IFMT: i64 = 0o170_000;
    const S_IFDIR: i64 = 0o040_000;
    // Located by name so a host's stat result is accepted whatever its field
    // order, and anything without an integer `st_mode` is refused.
    let st_mode = match unstable::root_node(value) {
        MontyNode::NamedTuple {
            field_names, values, ..
        } => field_names
            .iter()
            .position(|name| name == "st_mode")
            .and_then(|index| values.get(index))
            .and_then(|mode| match unstable::node(unstable::child(value.as_ref(), *mode)) {
                MontyNode::Int(mode) => Some(*mode),
                _ => None,
            }),
        _ => None,
    };
    match st_mode {
        Some(mode) if mode & S_IFMT == S_IFDIR => Ok(()),
        Some(_) => Err(ExcType::not_a_directory_error(spelled)),
        None => Err(SimpleException::new_msg(
            ExcType::RuntimeError,
            format!(
                "invalid return type: os.chdir requires the host to return a stat result, got {}",
                value.as_ref().type_name()
            ),
        )
        .into()),
    }
}

/// Reduces a host `Iterdir` result (list of virtual child paths) to the list
/// of bare entry names `os.listdir` returns — the resume half of
/// [`PreConversionEffect::ListdirNames`].
///
/// Runs on the raw [`MontyObject`] before heap import (see `VM::resume`),
/// so it needs no refcount handling; entries are renamed in place with no new
/// allocations. Virtual paths are always POSIX, so the name is the substring
/// after the last `/`. Hosts answering the `Path.iterdir` callback themselves
/// may return `str` entries instead of paths — both work.
pub(crate) fn listdir_names(value: MontyObject) -> Result<MontyObject, RunError> {
    directory_entries(value, None)
}

/// Accepts an `os.urandom` reply only as `bytes` of the requested length.
fn urandom_reply(value: MontyObject, size: usize) -> Result<MontyObject, RunError> {
    match unstable::root_node(&value) {
        MontyNode::Bytes(bytes) if bytes.len() == size => Ok(value),
        MontyNode::Bytes(bytes) => Err(urandom_reply_error(Ok(bytes.len()), size)),
        _ => Err(urandom_reply_error(Err(value.as_ref().type_name()), size)),
    }
}

/// The `RuntimeError` for an `os.urandom` reply that is not `bytes` of
/// `expected` length: `Ok(len)` for `bytes` of the wrong length, `Err(type)`
/// for any other type. Shared with `random`'s seeding path so the contract
/// and its wording stay identical.
pub(crate) fn urandom_reply_error(actual: Result<usize, &str>, expected: usize) -> RunError {
    let message = match actual {
        Ok(len) => format!("'os.urandom' returned {len} bytes, expected {expected}"),
        Err(type_name) => format!("'os.urandom' must return bytes, not {type_name}"),
    };
    SimpleException::new_msg(ExcType::RuntimeError, message).into()
}

/// Rebuilds host entries using the caller's original relative or absolute directory path.
///
/// The joins repeat the receiver once per host entry, so their total is
/// preflighted against `tracker` in one shot before any is built.
pub(crate) fn iterdir_paths(
    value: MontyObject,
    path: &str,
    tracker: &ResourceTracker,
) -> Result<MontyObject, RunError> {
    directory_entries(value, Some((path, tracker)))
}

/// Reduces host paths to entry names, joining them onto the `Path.iterdir()`
/// receiver when one is given (with the tracker its joins are charged to).
fn directory_entries(value: MontyObject, receiver: Option<(&str, &ResourceTracker)>) -> Result<MontyObject, RunError> {
    let invalid = |type_name: &str| -> RunError {
        let operation = if receiver.is_some() {
            "Path.iterdir"
        } else {
            "os.listdir"
        };
        SimpleException::new_msg(
            ExcType::RuntimeError,
            format!("invalid return type: {operation} requires the host to return a list of paths, got {type_name}"),
        )
        .into()
    };
    let MontyNode::List(ids) = unstable::root_node(&value) else {
        return Err(invalid(value.as_ref().type_name()));
    };
    let ids = ids.clone();
    let (mut graph, root) = unstable::into_graph_parts(value);
    let directory = match receiver {
        Some((path, tracker)) => {
            // Each joined path adds the receiver and a separator on top of the entry.
            tracker.check_allocation(ids.len().saturating_mul(path.len() + 1))?;
            Some(Path::new(path.to_owned()))
        }
        None => None,
    };
    // An entry the host listed twice is one shared node: rewrite it once.
    let mut seen = AHashSet::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        if !matches!(graph.node(id), MontyNode::Path(_) | MontyNode::String(_)) {
            return Err(invalid(graph.type_name(id)));
        }
        let node = graph.node_mut(id);
        let (MontyNode::Path(entry) | MontyNode::String(entry)) = node else {
            unreachable!("checked above");
        };
        if let Some(sep) = entry.rfind('/') {
            entry.drain(..=sep);
        }
        *node = if let Some(directory) = &directory {
            MontyNode::Path(directory.joinpath(entry))
        } else {
            MontyNode::String(mem::take(entry))
        };
    }
    Ok(unstable::object_from_graph(graph, root).expect("root unchanged"))
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
    vm: &mut VM<'_>,
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
    heap: &mut Heap,
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
    heap: &mut Heap,
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
fn extract_mkdir_args(path: MontyPath, args: ArgValues, vm: &mut VM<'_>) -> RunResult<MkdirCallArgs> {
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
    heap: &mut Heap,
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
fn arg_or_missing_data(method: &'static str, args: ArgValues, heap: &mut Heap) -> RunResult<Value> {
    if matches!(args, ArgValues::Empty) {
        return Err(ExcType::type_error(format!(
            "Path.{method}() missing 1 required positional argument: 'data'"
        )));
    }
    args.get_one_arg(method, heap)
}

/// Owned `String` if `value` is a `str` or `Path`, else `None`. Caller drops
/// the source value afterwards. Also used by the `os` module's path-taking
/// functions (`modules/os.rs`).
pub(crate) fn value_to_owned_string(value: &Value, heap: &Heap, interns: &Interns) -> Option<String> {
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
fn value_to_owned_bytes(value: &Value, heap: &Heap, interns: &Interns) -> Option<Vec<u8>> {
    match value {
        Value::InternBytes(id) => Some(interns.get_bytes(*id).to_owned()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Bytes(b) => Some(b.as_slice().to_owned()),
            _ => None,
        },
        _ => None,
    }
}
