//! OS-level operations that require host system access.
//!
//! Defines [`OsFunctionCall`] — a tagged dispatch value whose variants carry
//! the typed args each OS-call accepts. Type methods and builtins return one
//! as [`CallResult::OsCall`](crate::bytecode::CallResult::OsCall); the VM
//! yields [`FrameExit::OsCall`](crate::bytecode::FrameExit::OsCall) so the
//! host decides whether to permit it. The interpreter itself never performs
//! I/O.
//!
//! The fs/ layer matches on the enum directly (no `MontyObject` introspection);
//! host bindings get a generic `(positional, keyword)` view via
//! [`OsFunctionCall::to_args`].
//!
//! # Adding a new OS call
//!
//! Add a variant carrying a struct (reuse [`PathStringDataArgs`] etc. if the
//! shape matches), derive `ToArgs` on the struct, update
//! [`OsFunctionCall::name`] and the other inherent methods, add a matching
//! typed arm to `monty.proto`'s `OsCall` and `monty-proto`'s conversions,
//! then wire the new variant into the fs/ dispatcher and any host backends.

use std::{fmt, ops::Deref};

use crate::{
    ExcType, MontyException, MontyObject,
    args::{ArgValues, FromArgs, LaxBool, ToArgs, ToMontyObject},
    bytecode::VM,
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, Heap, HeapData},
    intern::{Interns, StaticStrings},
    object::MontyTimeZone,
    resource::ResourceTracker,
    types::{file::FileMode, str::StringRepr},
    value::Value,
};

// =============================================================================
// OsFunctionCall — the central public dispatch value.
// =============================================================================

/// Tagged dispatch value for OS-level operations.
///
/// Each variant carries the strongly-typed args/kwargs the corresponding OS
/// call needs. The fs/ layer matches on this enum directly (no `MontyObject`
/// introspection); host bindings get a generic `(positional, keyword)` view
/// via [`OsFunctionCall::to_args`].
///
/// See the module docs for how to add a new variant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::IntoStaticStr)]
pub enum OsFunctionCall {
    // ---- FS read / check (single path) ------------------------------------
    /// Check if a path exists.
    #[strum(serialize = "Path.exists")]
    Exists(MontyPath),
    /// Check if path is a regular file.
    #[strum(serialize = "Path.is_file")]
    IsFile(MontyPath),
    /// Check if path is a directory.
    #[strum(serialize = "Path.is_dir")]
    IsDir(MontyPath),
    /// Check if path is a symbolic link.
    #[strum(serialize = "Path.is_symlink")]
    IsSymlink(MontyPath),
    /// Read file contents as text.
    #[strum(serialize = "Path.read_text")]
    ReadText(MontyPath),
    /// Read file contents as bytes.
    #[strum(serialize = "Path.read_bytes")]
    ReadBytes(MontyPath),
    /// `stat()` — return a stat result tuple.
    #[strum(serialize = "Path.stat")]
    Stat(MontyPath),
    /// List directory contents.
    #[strum(serialize = "Path.iterdir")]
    Iterdir(MontyPath),
    /// Resolve symlinks and return absolute path.
    #[strum(serialize = "Path.resolve")]
    Resolve(MontyPath),
    /// Absolute path without symlink resolution.
    #[strum(serialize = "Path.absolute")]
    Absolute(MontyPath),

    // ---- FS write (path + data) -------------------------------------------
    /// Write text to file (truncating).
    #[strum(serialize = "Path.write_text")]
    WriteText(PathStringDataArgs),
    /// Append text to file.
    #[strum(serialize = "Path.append_text")]
    AppendText(PathStringDataArgs),
    /// Write bytes to file (truncating).
    #[strum(serialize = "Path.write_bytes")]
    WriteBytes(PathBytesDataArgs),
    /// Append bytes to file.
    #[strum(serialize = "Path.append_bytes")]
    AppendBytes(PathBytesDataArgs),

    // ---- FS mutate (custom shapes) ----------------------------------------
    /// Open a file. The host performs the open-time effect (truncate for
    /// `w`/`w+`, create-if-missing for `a`/`a+`, existence check for `r`/`r+`)
    /// and returns a [`MontyObject::FileHandle`] — it never holds a live OS
    /// handle across calls.
    #[strum(serialize = "open")]
    Open(OpenCallArgs),
    /// Create directory (`parents`/`exist_ok` kwargs).
    #[strum(serialize = "Path.mkdir")]
    Mkdir(MkdirCallArgs),
    /// Remove file.
    #[strum(serialize = "Path.unlink")]
    Unlink(MontyPath),
    /// Remove directory.
    #[strum(serialize = "Path.rmdir")]
    Rmdir(MontyPath),
    /// Rename / move (src → dst).
    #[strum(serialize = "Path.rename")]
    Rename(RenameCallArgs),

    // ---- Non-FS -----------------------------------------------------------
    /// Get an environment variable value.
    #[strum(serialize = "os.getenv")]
    Getenv(GetenvArgs),
    /// Get the entire environment as a dictionary.
    #[strum(serialize = "os.environ")]
    GetEnviron,
    /// Get today's date from the host system (for `date.today()`).
    #[strum(serialize = "date.today")]
    DateToday,
    /// Get the current date/time from the host system (for `datetime.now(tz=...)`).
    /// Carries the timezone argument, `None` for a naive result.
    #[strum(serialize = "datetime.now")]
    DateTimeNow(Option<MontyTimeZone>),

    /// Placeholder left behind by [`crate::OsCall::take_function_call`] and
    /// [`crate::ReplOsCall::take_function_call`] after the real call has been
    /// moved out for host dispatch. Never produced by the VM and never
    /// dispatched — it just keeps the field droppable. Disabled for strum, so
    /// [`OsFunctionCall::name`] panics on it.
    #[strum(disabled)]
    Used,
}

impl OsFunctionCall {
    /// Stable string name for this OS function — surfaces in
    /// [`Self::on_no_handler`] errors, host `os` callbacks, and serialised
    /// snapshots. The strum `serialize` string on each variant. Panics on
    /// [`Self::Used`], which is never surfaced.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.into()
    }

    /// Projects this call's args into `(positional, keyword)` `MontyObject`
    /// vectors for delivery to a host callback.
    #[must_use]
    pub fn to_args(self) -> (Vec<MontyObject>, Vec<(MontyObject, MontyObject)>) {
        match self {
            // Single-path variants — just the path in positionals.
            Self::Exists(p)
            | Self::IsFile(p)
            | Self::IsDir(p)
            | Self::IsSymlink(p)
            | Self::ReadText(p)
            | Self::ReadBytes(p)
            | Self::Stat(p)
            | Self::Iterdir(p)
            | Self::Resolve(p)
            | Self::Absolute(p)
            | Self::Unlink(p)
            | Self::Rmdir(p) => (vec![p.into_monty_object()], vec![]),
            // Multi-field variants delegate to their derived `ToArgs`.
            Self::WriteText(a) | Self::AppendText(a) => a.to_args(),
            Self::WriteBytes(a) | Self::AppendBytes(a) => a.to_args(),
            Self::Open(a) => a.to_args(),
            Self::Mkdir(a) => a.to_args(),
            Self::Rename(a) => a.to_args(),
            Self::Getenv(a) => a.to_args(),
            // Unit & single-value non-FS variants.
            Self::GetEnviron | Self::DateToday => (vec![], vec![]),
            Self::DateTimeNow(tz) => (vec![tz.map_or(MontyObject::None, MontyObject::TimeZone)], vec![]),
            Self::Used => unreachable!("OsFunctionCall::Used dispatched after take_function_call"),
        }
    }

    /// Whether this call can be handled by a `MountTable` (in the `monty-fs` crate).
    /// Non-FS variants (`Getenv`, `GetEnviron`, `DateToday`, `DateTimeNow`)
    /// must fall through to the host callback.
    ///
    /// Deliberately an allowlist: `Used` (and any future non-FS variant) must
    /// return `false`, because `monty-fs` panics if a call without a
    /// [`Self::primary_path`] reaches its filesystem dispatch.
    #[must_use]
    pub fn is_filesystem(&self) -> bool {
        matches!(
            self,
            Self::Exists(_)
                | Self::IsFile(_)
                | Self::IsDir(_)
                | Self::IsSymlink(_)
                | Self::ReadText(_)
                | Self::ReadBytes(_)
                | Self::WriteText(_)
                | Self::WriteBytes(_)
                | Self::AppendText(_)
                | Self::AppendBytes(_)
                | Self::Stat(_)
                | Self::Iterdir(_)
                | Self::Resolve(_)
                | Self::Absolute(_)
                | Self::Open(_)
                | Self::Mkdir(_)
                | Self::Unlink(_)
                | Self::Rmdir(_)
                | Self::Rename(_)
        )
    }

    /// Whether this call mutates filesystem state — the read-only-mount gate.
    /// `Open`'s write-ness is mode-dependent (`w`/`w+`/`a`/`a+` write; `r`/`r+`
    /// don't).
    #[must_use]
    pub fn is_write(&self) -> bool {
        match self {
            Self::WriteText(_)
            | Self::WriteBytes(_)
            | Self::AppendText(_)
            | Self::AppendBytes(_)
            | Self::Mkdir(_)
            | Self::Unlink(_)
            | Self::Rmdir(_)
            | Self::Rename(_) => true,
            Self::Open(args) => args.mode.create(),
            _ => false,
        }
    }

    /// Whether this operation checks existence without reading content.
    /// Existence checks return `false` for nonexistent paths rather than
    /// raising `FileNotFoundError`, matching CPython's `pathlib.Path`.
    #[must_use]
    pub fn is_existence_check(&self) -> bool {
        matches!(
            self,
            Self::Exists(_) | Self::IsFile(_) | Self::IsDir(_) | Self::IsSymlink(_)
        )
    }

    /// The call's primary path (for routing and error reporting), or `None`
    /// for non-FS variants.
    #[must_use]
    pub fn primary_path(&self) -> Option<&str> {
        match self {
            Self::Exists(p)
            | Self::IsFile(p)
            | Self::IsDir(p)
            | Self::IsSymlink(p)
            | Self::ReadText(p)
            | Self::ReadBytes(p)
            | Self::Stat(p)
            | Self::Iterdir(p)
            | Self::Resolve(p)
            | Self::Absolute(p)
            | Self::Unlink(p)
            | Self::Rmdir(p) => Some(p.as_str()),
            Self::WriteText(a) | Self::AppendText(a) => Some(a.path.as_str()),
            Self::WriteBytes(a) | Self::AppendBytes(a) => Some(a.path.as_str()),
            Self::Open(a) => Some(a.path.as_str()),
            Self::Mkdir(a) => Some(a.path.as_str()),
            Self::Rename(a) => Some(a.src.as_str()),
            Self::Getenv(_) | Self::GetEnviron | Self::DateToday | Self::DateTimeNow(_) => None,
            Self::Used => unreachable!("OsFunctionCall::Used inspected after take_function_call"),
        }
    }

    /// The rename destination path, or `None` for every other variant — the
    /// second routing key a mount table needs (both rename endpoints must
    /// resolve to the same mount).
    #[must_use]
    pub fn rename_destination(&self) -> Option<&str> {
        match self {
            Self::Rename(a) => Some(a.dst.as_str()),
            _ => None,
        }
    }

    /// Exception to raise when no handler accepted this call: `PermissionError`
    /// for FS ops (with the path), `RuntimeError` for non-FS ops.
    #[must_use]
    pub fn on_no_handler(&self) -> MontyException {
        if self.is_filesystem() {
            let path = self.primary_path().unwrap_or("<unknown>");
            MontyException::new(
                ExcType::PermissionError,
                Some(format!("Permission denied: {}", StringRepr(path))),
            )
        } else {
            MontyException::new(
                ExcType::RuntimeError,
                Some(format!("'{}' is not supported in this environment", self.name())),
            )
        }
    }
}

impl fmt::Display for OsFunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl<C: ContainsHeap> DropWithContext<C> for OsFunctionCall {
    // Owned args (String/Vec<u8>/bool/MontyPath/MontyObject) hold no live
    // heap references, so a plain drop is correct.
    fn drop_with(self, _heap: &mut C) {
        drop(self);
    }
}

// =============================================================================
// Args structs — per-variant payloads carried by `OsFunctionCall`.
// =============================================================================
//
// Each variant carries a struct that derives `ToArgs` for projection to
// `(positional, keyword)` MontyObjects. Zero-arg variants use empty structs so
// `to_args()` has no special arms. Producers construct these directly via
// struct literals (see `types/path.rs`, `builtins/open.rs`, etc.).

/// `path + str data` shape used by `WriteText` and `AppendText`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct PathStringDataArgs {
    pub path: MontyPath,
    pub data: String,
}

/// `path + bytes data` shape used by `WriteBytes` and `AppendBytes`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct PathBytesDataArgs {
    pub path: MontyPath,
    pub data: Vec<u8>,
}

/// `open(path, mode)` shape. The mode is parsed into [`FileMode`] before
/// construction so the fs/ backend doesn't re-parse; `ToArgs` re-serialises
/// it back to a `MontyObject::String` for the host.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct OpenCallArgs {
    pub path: MontyPath,
    pub mode: FileMode,
}

/// `mkdir(path, parents=False, exist_ok=False)` shape. `parents`/`exist_ok`
/// are kw-only so `ToArgs` emits them as kwargs (matching CPython).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct MkdirCallArgs {
    pub path: MontyPath,
    #[from_args(kw_only)]
    pub parents: bool,
    #[from_args(kw_only)]
    pub exist_ok: bool,
}

/// `rename(src, dst)` shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct RenameCallArgs {
    pub src: MontyPath,
    pub dst: MontyPath,
}

/// `os.getenv(key, default=None)` shape. The host decides whether to
/// substitute `default` when the variable is unset.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct GetenvArgs {
    pub key: String,
    pub default: MontyObject,
}

// =============================================================================
// MontyPath — owned virtual-sandbox path used by every path-bearing variant.
// =============================================================================

/// Owned virtual (sandbox) path carried by OS-call args.
///
/// `String` newtype: derefs to `&str` for fs/ routing, and `ToMontyObject`
/// projects it back to [`MontyObject::Path`] at the host boundary. Constructed
/// at the producer site after the source `Value` has been validated as a
/// path/string — never from raw input.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MontyPath(String);

impl MontyPath {
    #[must_use]
    pub fn new(path: String) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for MontyPath {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for MontyPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MontyPath {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl ToMontyObject for MontyPath {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::Path(self.0)
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

// =============================================================================
// stat_result builders — separate utility API used by host backends.
// =============================================================================
//
// These functions create MontyObject::NamedTuple values that match Python's
// os.stat_result structure. The stat_result has 10 fields:
// st_mode, st_ino, st_dev, st_nlink, st_uid, st_gid, st_size, st_atime, st_mtime, st_ctime.

/// Creates a `stat_result` for a regular file.
///
/// The file type bits (`0o100_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - File permissions as octal. Common values:
///   - `0o644` - rw-r--r-- (owner read/write, others read)
///   - `0o600` - rw------- (owner read/write only)
///   - `0o755` - rwxr-xr-x (executable, owner full, others read/execute)
///   - `0o100644` - same as 0o644 with explicit file type bits
/// * `size` - File size in bytes
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn file_stat(mode: i64, size: i64, mtime: f64) -> MontyObject {
    let mode = if mode < 0o1000 { mode | 0o100_000 } else { mode };
    stat_result(mode, 0, 0, 1, 0, 0, size, mtime, mtime, mtime)
}

/// Creates a `stat_result` for a directory.
///
/// The directory type bits (`0o040_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - Directory permissions as octal. Common values:
///   - `0o755` - rwxr-xr-x (owner full, others read/execute)
///   - `0o700` - rwx------ (owner only)
///   - `0o040755` - same as 0o755 with explicit directory type bits
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn dir_stat(mode: i64, mtime: f64) -> MontyObject {
    let mode = if mode < 0o1000 { mode | 0o040_000 } else { mode };
    stat_result(mode, 0, 0, 2, 0, 0, 4096, mtime, mtime, mtime)
}

/// Creates a `stat_result` for a symbolic link.
///
/// The symlink type bits (`0o120_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - Symlink permissions as octal. Common values:
///   - `0o777` - rwxrwxrwx (symlinks typically have full permissions)
///   - `0o120777` - same as 0o777 with explicit symlink type bits
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn symlink_stat(mode: i64, mtime: f64) -> MontyObject {
    let mode = if mode < 0o1000 { mode | 0o120_000 } else { mode };
    stat_result(mode, 0, 0, 1, 0, 0, 0, mtime, mtime, mtime)
}

/// Creates a full `stat_result` with all 10 fields specified.
///
/// This is the low-level builder; prefer `file_stat()`, `dir_stat()`, or `symlink_stat()`
/// for common cases.
#[must_use]
#[expect(clippy::too_many_arguments)]
pub fn stat_result(
    st_mode: i64,
    st_ino: i64,
    st_dev: i64,
    st_nlink: i64,
    st_uid: i64,
    st_gid: i64,
    st_size: i64,
    st_atime: f64,
    st_mtime: f64,
    st_ctime: f64,
) -> MontyObject {
    MontyObject::NamedTuple {
        type_name: STAT_RESULT_TYPE_NAME.to_owned(),
        field_names: STAT_RESULT_FIELDS.iter().map(|s| (*s).to_owned()).collect(),
        values: vec![
            MontyObject::Int(st_mode),
            MontyObject::Int(st_ino),
            MontyObject::Int(st_dev),
            MontyObject::Int(st_nlink),
            MontyObject::Int(st_uid),
            MontyObject::Int(st_gid),
            MontyObject::Int(st_size),
            MontyObject::Float(st_atime),
            MontyObject::Float(st_mtime),
            MontyObject::Float(st_ctime),
        ],
    }
}

const STAT_RESULT_TYPE_NAME: &str = "StatResult";
const STAT_RESULT_FIELDS: &[&str] = &[
    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid", "st_size", "st_atime", "st_mtime", "st_ctime",
];
