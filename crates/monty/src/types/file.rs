//! Heap-backed Python file wrappers used by the `open()` builtin.
//!
//! Monty does not keep native file descriptors open inside the sandbox. These
//! objects store only the virtual path, requested mode, a small Python-visible
//! state such as `closed`, and (lazily) a heap-resident full-file buffer
//! populated on the first sized/line read or `seek()`. Each OS round-trip is a
//! complete one-shot [`OsFunction`](crate::os::OsFunction) operation, so host
//! filesystem access remains mediated by the same boundary used by
//! `pathlib.Path`.
//!
//! # Read / seek buffering
//!
//! The **first** read of any shape (bare `read()`, sized `read(N)`,
//! `readline`/`readlines`) or `seek()` triggers an `OsFunction::ReadText` /
//! `OsFunction::ReadBytes` call whose result is *stored* into the file's
//! [`OpenFile::buffer`] field instead of being pushed directly to the operand
//! stack; the per-call slice (everything-from-position for bare `read()`, the
//! requested N chars/bytes for sized reads, the next line for `readline`,
//! etc.) is computed from the now-loaded buffer and pushed in its place.
//! Subsequent reads and seeks slice that heap buffer in pure Monty with no
//! further OS calls. The buffer lives in the heap and counts against the
//! configured `max_memory`.
//!
//! A buffered read that *fails* in the host leaves the file in a retry-safe
//! state — `pending_read` is cleared, `buffer` stays empty, and `eof` is not
//! flipped — so user code that catches the exception and retries gets a
//! fresh attempt.
//!
//! `position` is the **char offset** in text mode (so `f.read(5)` advances
//! `position` by 5 chars regardless of byte width) and the **byte offset** in
//! binary mode. `tell()` returns this number — text-mode is therefore a
//! divergence from CPython, which returns an opaque byte cookie. See
//! `limitations/open.md` for the full divergence list.
//!
//! # Unsupported / diverging behavior
//!
//! The current implementation is a deliberate subset of CPython's file API.
//! Code that relies on the following will not behave the same way as on
//! CPython:
//!
//! - The context-manager protocol (`with open(...) as f:`) is supported but
//!   `__exit__` always returns `None` — it cannot suppress an in-flight
//!   exception. The file is closed on exit on both the success and exception
//!   paths.
//! - `+` update modes (`r+`, `w+`, `a+`, and their `b` variants) are
//!   rejected at parse time because Monty has no read-position tracking;
//!   without it a write after a read would silently truncate the file via
//!   the one-shot OS write.
//! - The `encoding`, `errors`, and `newline` arguments to `open()` are
//!   accepted only at their CPython defaults (with `encoding="utf-8"` as
//!   a documented no-op). Text I/O is whole-file UTF-8 with no error
//!   handlers or newline translation.
//! - Bytes paths are decoded as UTF-8 instead of using CPython's
//!   `os.fsdecode` / filesystem-encoding behavior.
//! - `tell()` in text mode returns a char index, not CPython's opaque byte
//!   cookie. Round-trips through `seek()` correctly.
//! - `seek(N)` in text mode accepts any char-index offset, where CPython
//!   restricts it to `seek(0)`, `seek(0, 2)`, or a cookie from `tell()`.
//! - `for line in f:` iteration is not implemented. Use `readlines()` and
//!   iterate the resulting list instead.
//!
//! Any code path that needs one of these should be added explicitly
//! rather than relying on CPython parity.

use std::{borrow::Cow, fmt::Write, mem, str::FromStr};

use ahash::AHashSet;

use super::{
    List, PyTrait, Type,
    bytes::Bytes,
    str::{allocate_string, allocate_string_no_interning},
};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    os::{MontyPath, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs},
    resource::{ResourceError, ResourceTracker},
    types::str::StringRepr,
    value::{EitherStr, Value},
};

/// Shape of a buffered file read/seek request, recorded on [`OpenFile`] between
/// emitting the OS-call-with-store-hook and the matching resume.
///
/// The host returns the **full file content** for any of these variants —
/// Monty does not ask the host to slice. On resume the VM stores the host
/// content into [`OpenFile::buffer`] and uses this enum to compute the slice
/// (in pure Monty) that becomes the call's return value, advancing
/// [`OpenFile::position`] accordingly.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum ReadSpec {
    /// `f.read()` / `f.read(-1)` — return `buffer[position..]` and advance
    /// `position` to end-of-buffer.
    All,
    /// `f.read(N)` — return up to N chars (text) or bytes (binary) starting at
    /// `position`.
    Size(usize),
    /// `f.readline()` — return one line including the trailing `\n`, or the
    /// remainder of the buffer if the final line has no `\n`.
    Line,
    /// `f.readlines()` — return a Python list of every remaining line.
    Lines,
    /// `f.seek(offset, whence)` — buffer must be loaded so we can validate
    /// bounds and resolve `SEEK_END`. After the load, `compute_slice` updates
    /// `position`/`eof` and returns the new position as an int.
    Seek { offset: i64, whence: i64 },
}

/// File-specific work to perform when a paused OS call resumes.
///
/// This generalizes the original buffered-read hook: both buffered reads and
/// writes need to update [`OpenFile`] state only after the host reports a
/// successful OS operation. Keeping them in one enum avoids adding another VM
/// hook while preserving retry-safe exception behavior.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum PendingFileEffect {
    /// Store a full-file read result into the file buffer, then compute the
    /// pending read/seek slice.
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
}

/// A parsed Python `open()` mode.
///
/// This single enum captures everything that matters about how a file was
/// opened: the access pattern (`r`/`w`/`a` and the `+` update flag) and
/// whether the file is binary. The variant name encodes the access pattern;
/// the `bool` payload is `true` for binary and `false` for text — i.e.
/// `Read(true)` is `'rb'` and `Read(false)` is `'r'`.
///
/// Construct one with the [`FromStr`] impl (`mode_str.parse::<FileMode>()`).
/// The original input string is
/// intentionally not preserved; [`FileMode::as_str`] rebuilds the canonical
/// CPython form (`'r'`, `'rb+'`, `'wb'`, …), matching how CPython itself
/// normalizes input like `'rt'` → `'r'` and `'r+b'` → `'rb+'`.
///
/// `+` update modes (`ReadUpdate`/`WriteUpdate`/`AppendUpdate`) are reserved
/// in the enum so the mode space is fully represented, but [`FromStr`]
/// currently rejects them — properly modelling them needs read-position
/// tracking that the file wrapper does not yet implement. Treat the `Update`
/// variants as unreachable at runtime; do not pattern-match against them as
/// if they were a valid result of parsing user input.
///
/// Carried publicly by [`MontyObject::FileHandle`] so a host servicing file
/// operations can inspect the mode without re-parsing the raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FileMode {
    /// `r` / `rb`: read-only; the file must already exist.
    Read(bool),
    /// `r+` / `rb+`: read and write an existing file. Reserved; not yet
    /// produced by [`FromStr`].
    ReadUpdate(bool),
    /// `w` / `wb`: write-only; truncate the file (creating it if missing) on open.
    Write(bool),
    /// `w+` / `wb+`: read and write; truncate the file (creating it if missing).
    /// Reserved; not yet produced by [`FromStr`].
    WriteUpdate(bool),
    /// `a` / `ab`: write-only appending; create the file if missing, preserving content.
    Append(bool),
    /// `a+` / `ab+`: read and append; create the file if missing, preserving content.
    /// Reserved; not yet produced by [`FromStr`].
    AppendUpdate(bool),
}

impl FileMode {
    /// Returns the canonical Python `open()` mode string for this mode,
    /// matching what CPython exposes via `file.mode`.
    ///
    /// The result is always one of the 12 well-formed mode strings (`r`, `rb`,
    /// `r+`, `rb+`, `w`, `wb`, `w+`, `wb+`, `a`, `ab`, `a+`, `ab+`). This is
    /// the canonical form CPython itself normalizes user input into — e.g.
    /// `'rt'` → `'r'`, `'r+b'` → `'rb+'`, `'br'` → `'rb'`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read(false) => "r",
            Self::Read(true) => "rb",
            Self::ReadUpdate(false) => "r+",
            Self::ReadUpdate(true) => "rb+",
            Self::Write(false) => "w",
            Self::Write(true) => "wb",
            Self::WriteUpdate(false) => "w+",
            Self::WriteUpdate(true) => "wb+",
            Self::Append(false) => "a",
            Self::Append(true) => "ab",
            Self::AppendUpdate(false) => "a+",
            Self::AppendUpdate(true) => "ab+",
        }
    }

    /// Whether the file is binary (`'rb'`, `'wb'`, …) rather than text.
    #[must_use]
    pub fn is_binary(&self) -> bool {
        let (Self::Read(b)
        | Self::ReadUpdate(b)
        | Self::Write(b)
        | Self::WriteUpdate(b)
        | Self::Append(b)
        | Self::AppendUpdate(b)) = self;
        *b
    }

    /// Whether `read()` is allowed by this mode.
    #[must_use]
    pub fn readable(&self) -> bool {
        matches!(
            self,
            Self::Read(_) | Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_)
        )
    }

    /// Whether `write()` is allowed by this mode.
    #[must_use]
    pub fn writable(&self) -> bool {
        matches!(
            self,
            Self::Write(_) | Self::WriteUpdate(_) | Self::Append(_) | Self::AppendUpdate(_) | Self::ReadUpdate(_)
        )
    }

    /// Whether writes should always append (`a`/`a+`).
    #[must_use]
    pub fn is_append(&self) -> bool {
        matches!(self, Self::Append(_) | Self::AppendUpdate(_))
    }

    /// Whether `open()` must truncate the file to empty immediately (`w`/`w+`).
    #[must_use]
    pub fn truncate(&self) -> bool {
        matches!(self, Self::Write(_) | Self::WriteUpdate(_))
    }

    /// Whether `open()` must create the file immediately if missing.
    ///
    /// True for the `w`/`w+` and `a`/`a+` families. For append modes this must
    /// not disturb existing content.
    #[must_use]
    pub fn create(&self) -> bool {
        matches!(
            self,
            Self::Write(_) | Self::WriteUpdate(_) | Self::Append(_) | Self::AppendUpdate(_)
        )
    }

    /// Returns the `_io` wrapper type a file opened with this mode presents as.
    #[must_use]
    pub fn file_type(&self) -> Type {
        match self {
            _ if !self.is_binary() => Type::TextIOWrapper,
            Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_) => Type::BufferedRandom,
            Self::Read(_) => Type::BufferedReader,
            Self::Write(_) | Self::Append(_) => Type::BufferedWriter,
        }
    }

    /// Returns the bare Python type name (`type(f).__name__`) for this mode.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            _ if !self.is_binary() => "TextIOWrapper",
            Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_) => "BufferedRandom",
            Self::Read(_) => "BufferedReader",
            Self::Write(_) | Self::Append(_) => "BufferedWriter",
        }
    }
}

/// Parses a Python `open()` mode string into a [`FileMode`].
///
/// Monty supports the common read, write, append, and update combinations in
/// text or binary form. Exclusive creation (`x`) is rejected for now because
/// it needs a dedicated mount-table operation to be race-free.
///
/// The `Err` payload is a CPython-matched message — empty input, an unknown
/// mode character, duplicated `b`/`t`/`+`, conflicting binary+text flags, or
/// more than one of the `r`/`w`/`a` actions.
impl FromStr for FileMode {
    type Err = Cow<'static, str>;

    fn from_str(mode: &str) -> Result<Self, Self::Err> {
        if mode.is_empty() {
            // CPython's empty-mode error message, mirrored verbatim. Note: the
            // duplicate-action message is different (lowercase, no `... and at most one
            // plus` suffix) — see the `'r' | 'w' | 'a'` arm.
            return Err("Must have exactly one of create/read/write/append mode and at most one plus".into());
        }

        let mut action = None;
        let mut binary = false;
        let mut text = false;

        for ch in mode.chars() {
            match ch {
                'r' | 'w' | 'a' => {
                    if action.replace(ch).is_some() {
                        return Err("must have exactly one of create/read/write/append mode".into());
                    }
                }
                'x' => return Err("exclusive creation mode is not supported".into()),
                'b' => {
                    if binary {
                        return Err("invalid mode: binary mode specified twice".into());
                    }
                    binary = true;
                }
                't' => {
                    if text {
                        return Err("invalid mode: text mode specified twice".into());
                    }
                    text = true;
                }
                // `+` modes (`r+`, `w+`, `a+`, and their `b` variants) need
                // read-position tracking that Monty does not yet implement.
                // Reject them outright rather than silently truncating on the
                // first write (which would happen because the OS-level read
                // and write ops are full-file one-shots).
                '+' => return Err("update modes ('+') are not yet supported".into()),
                _ => return Err(format!("invalid mode: {ch:?}").into()),
            }
        }

        if binary && text {
            return Err("can't have text and binary mode at once".into());
        }

        Ok(match action.unwrap_or('r') {
            'w' => Self::Write(binary),
            'a' => Self::Append(binary),
            _ => Self::Read(binary),
        })
    }
}

/// A Python file object that stores path and mode state, but no native handle.
///
/// Monty keeps no live OS file descriptor: every OS round-trip is a complete
/// one-shot call that the host opens, performs, and closes. All state needed
/// to make those calls reproducible across a snapshot/resume — `path`, `mode`,
/// `position`, `id`, `buffer`, `pending_read`, `eof` — lives here and is
/// serialized.
///
/// `position` semantics depend on mode (documented on the field). It is the
/// offset future sized/line/seek operations operate against the heap buffer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OpenFile {
    path: String,
    mode: FileMode,
    /// Whether at least one write has been issued. For `w`/`wb` mode this
    /// switches subsequent writes from truncating to appending so write #2
    /// doesn't clobber write #1. Truncating modes start `true` because the
    /// host already emptied the file at `open()` time.
    first_write_done: bool,
    /// Whether `close()` has been called. Operations on a closed file raise
    /// `ValueError`.
    closed: bool,
    /// Position for sized/line/seek operations. Char index in text mode (so
    /// `read(5)` advances by 5 chars regardless of byte width), byte index in
    /// binary mode. Starts at 0; advanced by bare `read()` (by the length of
    /// the returned content), sized reads, line reads, and `seek()`.
    position: u64,
    /// Heap reference to the cached full-file content, populated lazily on
    /// the first sized/line read or `seek()`. Always `HeapData::Str` for text
    /// mode and `HeapData::Bytes` for binary mode. `None` until populated.
    buffer: Option<HeapId>,
    /// Cached metadata about the loaded buffer, populated in
    /// [`apply_buffer_store`] when `buffer` is set. The cache turns each
    /// sized-read / line-read / seek into O(work-actually-done) instead of
    /// O(total-buffer): without it `compute_slice_text` would re-walk the
    /// buffer from char 0 on every call (so a 10k-`readline()` loop on a
    /// 1MB UTF-8 file is O(n²)). See [`BufferMeta`] for field semantics.
    /// `None` while `buffer` is `None`; otherwise always `Some`.
    ///
    /// **Not serialized.** This is a pure cache derived from `buffer` +
    /// `position`, and trusting it across a snapshot boundary would let a
    /// crafted snapshot (with `byte_position` past `buffer.len()` or in the
    /// middle of a UTF-8 code point, or a `buffer_total` larger than the
    /// real buffer) drive `compute_slice_text` / `compute_slice_binary`
    /// into a panicking `&buffer[..]` slice. By skipping serde, the field
    /// is always `None` after deserialization; the next `compute_slice`
    /// call sees `Some(buffer) + None(buffer_meta)` and rebuilds it via
    /// [`populate_buffer_meta`] from the trusted heap buffer.
    #[serde(skip)]
    buffer_meta: Option<BufferMeta>,
    /// Set between emitting the OS-call-with-store hook and the resume firing.
    /// Tells the resume path which slice to compute once the buffer is
    /// populated. Cleared by the post-resume hook (or by exception cleanup).
    pending_read: Option<ReadSpec>,
    /// Tracks whether we have reached EOF on this file. Separate from the
    /// buffer's existence so bare `read()` (which today never populates the
    /// buffer) can also flag EOF without forcing a load.
    eof: bool,
    /// Logical length tracked for write-only files that never load a read
    /// buffer. For read-capable files, the loaded buffer metadata remains the
    /// source of truth for `SEEK_END`.
    file_length: u64,
}

/// Cached metadata about an [`OpenFile`]'s loaded buffer.
///
/// Both fields are derived from `position` + the heap-resident buffer and
/// could be recomputed from them on demand; we cache them so the hot
/// read/readline loops do not re-scan the buffer from char 0 on every call.
/// Maintained incrementally by every operation that advances `position`.
///
/// Binary-mode files set `byte_position == min(position, buffer.len())` and
/// `buffer_total == buffer.len()` — the cache is mostly redundant there, but
/// kept identical to the text-mode layout so the surrounding code does not
/// need to branch on mode.
/// **Not (de)serialized** — see the security note on
/// [`OpenFile::buffer_meta`]. Without `serde` derives, the cache cannot be
/// driven by attacker-controlled snapshot bytes; it is rebuilt by
/// [`populate_buffer_meta`] from the trusted heap buffer after a restore.
#[derive(Debug, Clone, Copy)]
struct BufferMeta {
    /// Byte offset into the buffer that matches `position`, clamped to
    /// `buffer.len()`. For text mode this caches the
    /// [`nth_char_byte_offset`] walk; for binary mode it equals
    /// `min(position, buffer.len())`.
    byte_position: u64,
    /// Total length of the buffer in user-visible units — chars for text
    /// (cached `chars().count()`), bytes for binary (`== buffer.len()`).
    /// Constant once the buffer is loaded.
    buffer_total: u64,
}

impl OpenFile {
    /// Creates a path-backed file wrapper from a parsed `open()` mode and the
    /// `position` carried across the host boundary by a
    /// [`MontyObject::FileHandle`](crate::MontyObject::FileHandle).
    ///
    /// Truncating modes (`w`/`w+`) have already had the file emptied by the
    /// host at `open()` time, so the wrapper starts with `first_write_done`
    /// set: the first user `write()` should append rather than truncate again.
    /// `buffer`/`pending_read`/`eof` all start unset — the buffer is populated
    /// lazily on the first sized/line read or `seek()`.
    #[must_use]
    pub fn with_state(path: String, mode: FileMode, position: u64) -> Self {
        Self {
            path,
            mode,
            first_write_done: mode.truncate(),
            closed: false,
            position,
            buffer: None,
            buffer_meta: None,
            pending_read: None,
            eof: false,
            file_length: position,
        }
    }

    /// Returns the virtual path used for OS calls.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the canonical mode string shown to Python code.
    #[must_use]
    pub fn mode(&self) -> &'static str {
        self.mode.as_str()
    }

    /// Returns the parsed `open()` mode.
    #[must_use]
    pub fn file_mode(&self) -> &FileMode {
        &self.mode
    }

    /// Returns the byte offset for seek-aware reads.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Returns the type represented by this file wrapper.
    #[must_use]
    pub fn file_type(&self) -> Type {
        self.mode.file_type()
    }
}

impl HeapItem for OpenFile {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.path.len()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // The buffer holds a heap reference (Str/Bytes) that must be released
        // when the file is dropped. Everything else is plain Rust data.
        if let Some(buffer_id) = self.buffer.take() {
            stack.push(buffer_id);
        }
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, OpenFile> {
    fn py_type(&self, vm: &VM<'h, impl ResourceTracker>) -> Type {
        self.get(vm.heap).file_type()
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, _other: &Self, _vm: &mut VM<'h, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(false)
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let file = self.get(vm.heap);
        write!(
            f,
            "<{} name={} mode={}>",
            file.file_type(),
            StringRepr(file.path()),
            StringRepr(file.mode())
        )?;
        Ok(())
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)));
        };

        match method {
            StaticStrings::Read => self.read(self_id, vm, args),
            StaticStrings::Readline => self.readline(self_id, vm, args),
            StaticStrings::Readlines => self.readlines(self_id, vm, args),
            StaticStrings::Tell => self.tell(vm, args),
            StaticStrings::Seek => self.seek(self_id, vm, args),
            StaticStrings::Write => self.write(self_id, vm, args),
            StaticStrings::Close => self.close(vm, args),
            StaticStrings::Flush => self.flush(vm, args),
            StaticStrings::Readable => self.readable(vm, args),
            StaticStrings::Writable => self.writable(vm, args),
            StaticStrings::Seekable => self.seekable(vm, args),
            _ => {
                args.drop_with_heap(vm);
                Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)))
            }
        }
    }

    fn py_is_context_manager(&self) -> bool {
        true
    }

    fn py_enter(&mut self, self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CallResult> {
        // Match CPython: entering on a closed file raises before the body runs.
        // (Reusing a closed file as a context manager is rare but the error
        // message is part of the user contract.)
        self.get(vm.heap).ensure_open()?;
        // Return the file itself. Bumping the refcount here gives the new
        // Value::Ref its own count — constructing a fresh Value::Ref without
        // an inc_ref would let the Drop impl panic when an in-flight value
        // is later discarded without a matching drop_with_heap.
        vm.heap.inc_ref(self_id);
        Ok(CallResult::Value(Value::Ref(self_id)))
    }

    fn py_exit(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        _exc: Option<HeapId>,
    ) -> RunResult<CallResult> {
        // `with open(...) as f:` always closes the file on exit, success or
        // failure. We don't suppress exceptions: returning `None` is falsy, so
        // any in-flight exception propagates as it would in CPython.
        //
        // `close()` on an already-closed file is idempotent (a no-op), matching
        // CPython.
        self.get_mut(vm.heap).closed = true;
        Ok(CallResult::Value(Value::None))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let Some(method) = attr.static_string() else {
            return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)));
        };

        let file = self.get(vm.heap);
        let value = match method {
            StaticStrings::Name => allocate_string(file.path.clone(), vm.heap)?,
            StaticStrings::Mode => allocate_string(file.mode.as_str().to_owned(), vm.heap)?,
            StaticStrings::Closed => Value::Bool(file.closed),
            StaticStrings::Encoding if !file.mode.is_binary() => allocate_string("utf-8", vm.heap)?,
            _ => return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(value)))
    }
}

impl<'h> HeapRead<'h, OpenFile> {
    /// Implements `file.read()` and `file.read(size)`.
    ///
    /// All variants flow through the same buffer-store hook so the file's
    /// `position` and `eof` flags stay correct regardless of whether the
    /// caller mixes bare `read()`, sized `read(N)`, or line-oriented
    /// operations. The buffer holds the full file content; further reads
    /// slice it in pure Monty.
    fn read(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let spec = parse_read_size_arg(args.get_zero_one_arg("read", vm.heap)?, vm)?;
        if matches!(spec, ReadSpec::Size(0)) {
            // `read(0)`: empty result without any OS call, position unchanged.
            // Open/readable checks still happen to match CPython's error order.
            let binary = {
                let file = self.get(vm.heap);
                file.ensure_open()?;
                if !file.mode.readable() {
                    return Err(unsupported_operation("not readable"));
                }
                file.mode.is_binary()
            };
            Ok(CallResult::Value(empty_result(binary, vm.heap)?))
        } else {
            self.read_with_spec(self_id, vm, spec)
        }
    }

    /// Implements `file.readline()` — yields up to and including the next
    /// `\n`, or the rest of the buffer if the final line has no newline. At
    /// EOF returns `''`/`b''`.
    fn readline(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        args.check_zero_args("readline", vm.heap)?;
        self.read_with_spec(self_id, vm, ReadSpec::Line)
    }

    /// Implements `file.readlines()` — returns a `list[str]` (or `list[bytes]`
    /// for binary mode) of every remaining line.
    fn readlines(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        args.check_zero_args("readlines", vm.heap)?;
        self.read_with_spec(self_id, vm, ReadSpec::Lines)
    }

    /// Implements `file.tell()` — returns the current position as an int.
    ///
    /// In text mode the value is a char index into the buffer (a documented
    /// divergence from CPython, which returns an opaque byte cookie); in
    /// binary mode it is a byte offset, which matches CPython.
    fn tell(&self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("tell", vm.heap)?;
        let file = self.get(vm.heap);
        file.ensure_open()?;
        // Positions never exceed the buffer length, and the buffer is bounded
        // by `max_memory`. A pathological host could hand back a buffer with
        // `len > i64::MAX` which would only matter on a system with > 8 EiB
        // of RAM — far outside the resource model. We surface the overflow
        // as `OverflowError` rather than panic.
        let pos = i64::try_from(file.position).map_err(|_| ExcType::overflow_c_ssize_t())?;
        Ok(CallResult::Value(Value::Int(pos)))
    }

    /// Implements `file.seek(offset, whence=0)` — repositions within the
    /// buffer, loading it on demand if not yet present, then returns the new
    /// absolute position.
    fn seek(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let (offset, whence) = parse_seek_args(args, vm)?;
        if self.get(vm.heap).mode.readable() {
            self.read_with_spec(self_id, vm, ReadSpec::Seek { offset, whence })
        } else {
            let (target, file_length) = {
                let file = self.get(vm.heap);
                file.ensure_open()?;
                let position = i64::try_from(file.position).map_err(|_| ExcType::overflow_c_ssize_t())?;
                let file_length = i64::try_from(file.file_length).map_err(|_| ExcType::overflow_c_ssize_t())?;
                (
                    resolve_seek_target(offset, whence, position, file_length)?,
                    file.file_length,
                )
            };
            let target_u64 = u64::try_from(target).map_err(|_| ExcType::overflow_c_ssize_t())?;
            let file = self.get_mut(vm.heap);
            file.position = target_u64;
            file.eof = target_u64 >= file_length;
            Ok(CallResult::Value(Value::Int(target)))
        }
    }

    /// Shared dispatch for any operation that needs the buffer loaded
    /// (`readline`, `readlines`, `seek`).
    ///
    /// If the buffer is already loaded, computes the slice synchronously.
    /// Otherwise records the spec on the file and yields a
    /// [`CallResult::OsCallStoreBuffer`] so the host loads the full content
    /// and the resume hook completes the operation.
    fn read_with_spec(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        spec: ReadSpec,
    ) -> RunResult<CallResult> {
        let (binary, buffer_loaded) = {
            let file = self.get(vm.heap);
            file.ensure_open()?;
            if !file.mode.readable() {
                return Err(unsupported_operation("not readable"));
            }
            // `eof` on a readable file is only ever flipped by a path that
            // also populates `buffer`, so we never need to short-circuit on
            // EOF without a loaded buffer here. Write-only files would be
            // the only case where `eof=true && buffer=None` is possible, and
            // they're rejected by the readable check above.
            debug_assert!(!file.eof || file.buffer.is_some());
            (file.mode.is_binary(), file.buffer.is_some())
        };

        if buffer_loaded {
            return compute_slice(self_id, spec, vm).map(CallResult::Value);
        }

        // First buffered op: stash spec, yield to host for the full content.
        self.get_mut(vm.heap).pending_read = Some(spec);
        // Build the typed OS-call payload. The OS call always carries the
        // file's virtual path — the buffer-store hook (see VM dispatcher)
        // separately routes the result into the file's `buffer` slot, so we
        // only need the path here, not a file-handle reference.
        let path = MontyPath::new(self.get(vm.heap).path().to_owned());
        let call = if binary {
            OsFunctionCall::ReadBytes(path)
        } else {
            OsFunctionCall::ReadText(path)
        };
        inc_ref_for_pending_oscall(vm, self_id);
        Ok(CallResult::OsCallStoreBuffer { call, file_id: self_id })
    }

    /// Implements `file.write(data)` as a one-shot OS write or append.
    ///
    /// As with [`Self::read`], the first OS-call argument is the file object
    /// itself, delivered to the host as a `MontyObject::FileHandle`.
    fn write(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let data = args.get_one_arg("write", vm.heap)?;
        let binary = self.get(vm.heap).mode.is_binary();
        if let Err(err) = validate_write_data(&data, binary, vm) {
            data.drop_with_heap(vm);
            return Err(err);
        }
        if let Err(err) = self.get(vm.heap).ensure_open() {
            data.drop_with_heap(vm);
            return Err(err);
        }
        let (path, append, binary) = {
            let file = self.get_mut(vm.heap);
            if !file.mode.writable() {
                let message = if file.mode.is_binary() { "write" } else { "not writable" };
                data.drop_with_heap(vm);
                return Err(unsupported_operation(message));
            }
            let append = file.mode.is_append() || file.first_write_done;
            let binary = file.mode.is_binary();
            let path = file.path().to_owned();
            file.first_write_done = true;
            (path, append, binary)
        };

        // Extract the data payload into an owned `String` / `Vec<u8>` so the
        // OS-call args struct can own it across the snapshot/resume boundary.
        // `validate_write_data` already gated the variant — `binary` selects
        // bytes vs str.
        let path = MontyPath::new(path);
        let call = if binary {
            let bytes = extract_bytes_payload(&data, vm).expect("validate_write_data accepted a bytes-shaped value");
            data.drop_with_heap(vm);
            let args = PathBytesDataArgs { path, data: bytes };
            if append {
                OsFunctionCall::AppendBytes(args)
            } else {
                OsFunctionCall::WriteBytes(args)
            }
        } else {
            let text = extract_str_payload(&data, vm).expect("validate_write_data accepted a str-shaped value");
            data.drop_with_heap(vm);
            let args = PathStringDataArgs { path, data: text };
            if append {
                OsFunctionCall::AppendText(args)
            } else {
                OsFunctionCall::WriteText(args)
            }
        };

        inc_ref_for_pending_oscall(vm, self_id);
        vm.pending_file_effect = Some(PendingFileEffect::WritePosition {
            file_id: self_id,
            previous_position: self.get(vm.heap).position,
            previous_length: self.get(vm.heap).file_length,
        });
        Ok(CallResult::OsCall(call))
    }

    /// Marks the file wrapper as closed and releases the cached read buffer.
    ///
    /// Releasing the buffer matters for **resource accounting**: the
    /// full-file buffer is a separate heap entry whose `py_estimate_size`
    /// counts against `max_memory`. Without an explicit release here a
    /// closed file would keep its (potentially large) buffer alive until
    /// the file object's Python-level refcount drops to zero — long after
    /// the user has signalled they're done with it. By `dec_ref`ing the
    /// buffer here, `current_memory()` drops by the buffer size as soon as
    /// `close()` returns, matching CPython's behaviour and giving the user
    /// a deterministic way to free file-cache memory.
    ///
    /// Other holders (e.g. a `data = f.read()` reference) keep the entry
    /// alive via their own refcounts, so this release is safe — it only
    /// frees the buffer if nothing else points at it.
    fn close(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("close", vm.heap)?;
        let buffer_id = {
            let file = self.get_mut(vm.heap);
            file.closed = true;
            // Wipe the cached metadata alongside the buffer slot so a later
            // (erroneous) `compute_slice` doesn't see a `buffer_meta` that
            // points to a freed entry. Idempotent across repeat `close()` calls.
            file.buffer_meta = None;
            file.buffer.take()
        };
        if let Some(buffer_id) = buffer_id {
            vm.heap.dec_ref(buffer_id);
        }
        Ok(CallResult::Value(Value::None))
    }

    /// Implements `flush()` as a no-op because writes are committed immediately.
    fn flush(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("flush", vm.heap)?;
        self.get(vm.heap).ensure_open()?;
        Ok(CallResult::Value(Value::None))
    }

    /// Returns whether this file object supports `read()`.
    fn readable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("readable", vm.heap)?;
        let file = self.get(vm.heap);
        file.ensure_open()?;
        Ok(CallResult::Value(Value::Bool(file.mode.readable())))
    }

    /// Returns whether this file object supports `write()`.
    fn writable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("writable", vm.heap)?;
        let file = self.get(vm.heap);
        file.ensure_open()?;
        Ok(CallResult::Value(Value::Bool(file.mode.writable())))
    }

    /// Returns `True`: Monty file wrappers are modelled as regular files and
    /// support logical `seek()` / `tell()` state even though actual host I/O is
    /// still performed as one-shot calls.
    fn seekable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("seekable", vm.heap)?;
        self.get(vm.heap).ensure_open()?;
        Ok(CallResult::Value(Value::Bool(true)))
    }
}

impl OpenFile {
    /// Raises the CPython-style error used for operations after `close()`.
    fn ensure_open(&self) -> RunResult<()> {
        if self.closed {
            Err(SimpleException::new_msg(ExcType::ValueError, "I/O operation on closed file.").into())
        } else {
            Ok(())
        }
    }

    /// Drops any recorded `pending_read` slice spec without taking the buffer.
    ///
    /// Called by the VM resume path when the host raised during a buffered
    /// read OS call. Leaves the file in a retry-safe state: no stale slice
    /// spec hanging off the next operation, but also no spurious `eof` flag.
    pub(crate) fn clear_pending_read(&mut self) {
        self.pending_read = None;
    }

    /// Restores write-position state after a host-side write exception.
    pub(crate) fn rollback_write_position(&mut self, previous_position: u64, previous_length: u64) {
        self.position = previous_position;
        self.file_length = previous_length;
        self.eof = previous_position >= previous_length;
    }
}

/// Increments `file_id`'s refcount by 2 for an OS call that pins the file
/// across the host yield.
///
/// One ref is owned by the [`ArgValues`] passed to the host (released when
/// the host boundary converts the args to `MontyObject` and drops them); the
/// other ref is owned by the VM's `pending_file_effect` slot and is released
/// in [`apply_buffer_store`] / [`apply_write_position`] (or by
/// [`VM::resume_with_exception`](crate::bytecode::VM::resume_with_exception)
/// if the host raises). Keeping both inc_refs behind this one helper makes
/// the matched dec_refs easier to verify by eye.
fn inc_ref_for_pending_oscall(vm: &VM<'_, impl ResourceTracker>, file_id: HeapId) {
    vm.heap.inc_ref(file_id); // released at the host boundary (args drop)
    vm.heap.inc_ref(file_id); // released in apply_*_position / resume_with_exception (pin)
}

/// Materialises the host-returned `result` into a heap-resident `HeapId`
/// suitable for the file's `buffer` slot.
///
/// The `OsFunction::ReadText` / `ReadBytes` host boundary returns one of
/// `MontyObject::String` / `MontyObject::Bytes`, which `to_value` may turn
/// into an interned `Value::InternString` / `Value::InternBytes` (notably
/// for empty strings or single-char ASCII) instead of a `Value::Ref`. The
/// file's `buffer` slot is `Option<HeapId>` and slicing assumes a
/// heap-resident `Str` / `Bytes`, so interned variants are reallocated
/// onto the heap here.
///
/// `result` is fully consumed: every path runs `drop_with_heap` exactly
/// once. The two `Value::Ref`-producing arms `inc_ref` the entry they
/// hand back so the upcoming `drop_with_heap`'s dec_ref balances out and
/// the returned `HeapId` keeps the refcount it would have had without
/// the dance. This avoids `mem::forget`, which clippy flags on the
/// no-Drop release configuration.
fn os_read_result_to_heap_id(result: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<HeapId> {
    // Match by reference: `Value` has a Drop impl under `memory-model-checks`,
    // so we cannot destructure variants by move.
    let id = match &result {
        Value::Ref(id) => {
            vm.heap.inc_ref(*id);
            *id
        }
        Value::InternString(string_id) => {
            let s = vm.interns.get_str(*string_id).to_owned();
            // `allocate_string_no_interning` returns `Value::Ref` with
            // refcount 1; inc_ref+drop_with_heap below nets to zero and
            // lets us drop the temporary Value cleanly.
            let v = allocate_string_no_interning(s, vm.heap)?;
            let Value::Ref(new_id) = &v else {
                unreachable!("allocate_string_no_interning returns Value::Ref");
            };
            let new_id = *new_id;
            vm.heap.inc_ref(new_id);
            v.drop_with_heap(vm);
            new_id
        }
        Value::InternBytes(bytes_id) => {
            let b = vm.interns.get_bytes(*bytes_id).to_vec();
            vm.heap.allocate(HeapData::Bytes(Bytes::new(b)))?
        }
        _ => {
            result.drop_with_heap(vm);
            return Err(RunError::internal(
                "os_read_result_to_heap_id: OS result must be a string or bytes value",
            ));
        }
    };
    result.drop_with_heap(vm);
    Ok(id)
}

/// Stores the OS-returned full-file content into an [`OpenFile`]'s buffer and
/// computes the slice that the originating call (`read(N)` / `readline()` /
/// `readlines()` / `seek()`) should return.
///
/// Called by the VM resume path when the paused OS call was emitted via
/// [`CallResult::OsCallStoreBuffer`](crate::bytecode::CallResult::OsCallStoreBuffer).
///
/// Invariants on entry:
/// - `result` is `Value::Ref(_)` (or an interned `String`/`Bytes`) coming
///   from `OsFunction::ReadText` / `ReadBytes`. The host boundary guarantees
///   one of these for the OS functions we emit.
/// - `OpenFile::pending_read` is `Some(_)`. If it isn't (snapshot/restore
///   mismatch or VM bug) we raise an internal `RuntimeError` rather than
///   panic, so a host can recover.
///
/// The file owns one inc_ref on `result` for its `buffer` slot. The caller's
/// inc_ref on `file_id` (held by the in-flight OS call) is released here
/// via the RAII [`HeapGuard`] on `Value::Ref(file_id)`, so every error path
/// drops the pin without explicit `dec_ref` boilerplate.
///
/// **Error handling**: `pending_read` is taken up-front so every subsequent
/// error path leaves the file in a retry-safe state — a user-caught
/// exception followed by a retry sees no stale slice spec.
pub(crate) fn apply_buffer_store(
    file_id: HeapId,
    result: Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    // The pin's dec_ref happens automatically on every path via the guard's
    // Drop, so the early-return branches do not need explicit `dec_ref`s.
    let mut pin = HeapGuard::new(Value::Ref(file_id), vm);

    // Stage 1: drain `pending_read` from the file. `result_guard` keeps the
    // host-returned value alive across early-return branches; on the success
    // path we hand ownership back via `into_inner`.
    let (result, spec) = {
        let (_, vm) = pin.as_parts_mut();
        let mut result_guard = HeapGuard::new(result, vm);
        let (_, vm) = result_guard.as_parts_mut();

        let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
            return Err(RunError::internal(
                "apply_buffer_store: file_id does not point to an OpenFile",
            ));
        };
        let spec = file.get_mut(vm.heap).pending_read.take();
        drop(file);
        let Some(spec) = spec else {
            return Err(RunError::internal("apply_buffer_store: OpenFile has no pending_read"));
        };
        (result_guard.into_inner(), spec)
    };

    // Stage 2: materialise the host result onto the heap. `result_guard` is
    // no longer needed — the refcount now lives on `result_id`.
    let (_, vm) = pin.as_parts_mut();
    let result_id = os_read_result_to_heap_id(result, vm)?;

    // Stage 3: install the buffer. Defensive: if it was already populated
    // (e.g. a snapshot/restore race), drop the new content and slice from
    // the existing one instead of stomping it.
    let dec_result = {
        let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
            vm.heap.dec_ref(result_id);
            return Err(RunError::internal(
                "apply_buffer_store: file_id does not point to an OpenFile",
            ));
        };
        let f = file.get_mut(vm.heap);
        if f.buffer.is_some() {
            true
        } else {
            f.buffer = Some(result_id);
            false
        }
    };
    if dec_result {
        vm.heap.dec_ref(result_id);
    }

    // Populate the cached buffer metadata so the upcoming `compute_slice`
    // call (and every later read) starts from a known byte position and
    // buffer length without re-scanning the buffer from char 0.
    populate_buffer_meta(file_id, vm)?;

    // Compute the slice while the pin guard still keeps the file alive —
    // otherwise `open(p).read(5)` (where no caller holds a separate
    // reference) would risk a use-after-free if the pin's dec_ref ran first.
    compute_slice(file_id, spec, vm)
    // `pin` drops here, releasing the pending-file-effect refcount.
}

/// Applies a successful host write result to an [`OpenFile`]'s logical
/// position, then returns that same result to Python.
///
/// The host write result is expected to be the number of user-visible units
/// written: chars for text files, bytes for binary files. That matches the
/// values returned by Monty's filesystem backends and CPython's `write()`.
///
/// As with [`apply_buffer_store`], the pending-file-effect pin on `file_id`
/// is released via the RAII [`HeapGuard`] regardless of which path the
/// function takes.
pub(crate) fn apply_write_position(
    file_id: HeapId,
    result: Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let mut pin = HeapGuard::new(Value::Ref(file_id), vm);
    let (_, vm) = pin.as_parts_mut();
    let mut result_guard = HeapGuard::new(result, vm);
    let (result_ref, vm) = result_guard.as_parts_mut();

    let written = result_ref.as_int(vm)?;
    if written < 0 {
        return Err(RunError::internal(
            "apply_write_position: write count cannot be negative",
        ));
    }
    let written = u64::try_from(written).map_err(|_| ExcType::overflow_c_ssize_t())?;

    let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
        return Err(RunError::internal(
            "apply_write_position: file_id does not point to an OpenFile",
        ));
    };
    let f = file.get_mut(vm.heap);
    let new_position = f
        .position
        .checked_add(written)
        .ok_or_else(ExcType::overflow_c_ssize_t)?;
    f.position = new_position;
    f.file_length = f.file_length.max(new_position);
    f.eof = new_position >= f.file_length;
    drop(file);

    Ok(result_guard.into_inner())
    // `pin` drops here, releasing the pending-file-effect refcount.
}

/// Computes the [`Value`] returned by a buffered file operation, given the
/// already-loaded buffer and the operation's [`ReadSpec`].
///
/// Mutates `OpenFile::position` and `OpenFile::eof` to reflect the slice
/// that's being returned, and allocates a fresh `Str`/`Bytes`/`List` for the
/// result. All buffer reads go through the heap so the slice content is fully
/// snapshot-safe.
fn compute_slice(file_id: HeapId, spec: ReadSpec, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    // Defensive: if `buffer` is loaded but `buffer_meta` is missing (e.g. a
    // restored snapshot from an older schema, or a future code path that
    // sets `buffer` directly), reconstruct the cache before slicing instead
    // of raising an internal error.
    let needs_meta = {
        let HeapReadOutput::OpenFile(file) = vm.heap.read(file_id) else {
            return Err(RunError::internal("compute_slice: not an OpenFile"));
        };
        let f = file.get(vm.heap);
        f.buffer.is_some() && f.buffer_meta.is_none()
    };
    if needs_meta {
        populate_buffer_meta(file_id, vm)?;
    }

    let (binary, buffer_id, position, byte_position, buffer_total) = {
        let HeapReadOutput::OpenFile(file) = vm.heap.read(file_id) else {
            return Err(RunError::internal("compute_slice: not an OpenFile"));
        };
        let f = file.get(vm.heap);
        let buffer = f
            .buffer
            .ok_or_else(|| RunError::internal("compute_slice: buffer must be loaded"))?;
        let meta = f
            .buffer_meta
            .ok_or_else(|| RunError::internal("compute_slice: buffer_meta must be loaded"))?;
        let position = usize::try_from(f.position).map_err(|_| ExcType::overflow_c_ssize_t())?;
        let byte_position = usize::try_from(meta.byte_position).map_err(|_| ExcType::overflow_c_ssize_t())?;
        let buffer_total = usize::try_from(meta.buffer_total).map_err(|_| ExcType::overflow_c_ssize_t())?;
        (f.mode.is_binary(), buffer, position, byte_position, buffer_total)
    };

    if binary {
        compute_slice_binary(file_id, buffer_id, position, buffer_total, spec, vm)
    } else {
        compute_slice_text(file_id, buffer_id, position, byte_position, buffer_total, spec, vm)
    }
}

/// Text-mode slice computation.
///
/// `position` is the user-visible char index, `byte_position` is the cached
/// byte offset matching `position` clamped to `buffer.len()`, and
/// `buffer_total` is the cached `chars().count()` of the buffer. The cache
/// avoids an O(position) `nth_char_byte_offset` scan per call — the only
/// per-call walks are now over the bytes actually returned (so an N-line
/// `readline()` loop is O(N) total instead of O(N²)).
///
/// The buffer is read as a borrowed `&str` and the result is allocated while
/// that borrow is still live; `Heap::allocate` is `&self` and the paged
/// storage guarantees existing references stay valid across allocations, so
/// the previous full-buffer `to_owned()` is gone.
fn compute_slice_text(
    file_id: HeapId,
    buffer_id: HeapId,
    position: usize,
    byte_position: usize,
    buffer_total: usize,
    spec: ReadSpec,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    // Phase 1: read the buffer through an immutable heap borrow, compute the
    // slice, and allocate the result. `update_file_state` needs `&mut Heap`,
    // so we cannot call it until this scope ends and the borrow is released.
    let (value, new_position, new_byte_position, eof) = {
        let buffer = match vm.heap.get(buffer_id) {
            HeapData::Str(s) => s.as_str(),
            _ => return Err(RunError::internal("compute_slice_text: buffer is not a Str")),
        };
        debug_assert!(byte_position <= buffer.len());
        let tail = &buffer[byte_position..];

        match spec {
            ReadSpec::All => {
                let value = if byte_position == 0 {
                    vm.heap.inc_ref(buffer_id);
                    Value::Ref(buffer_id)
                } else {
                    allocate_string(tail.to_owned(), vm.heap)?
                };
                // Preserve `position` if it was already past `buffer_total`
                // (set there by `seek()`) — CPython's read-at-EOF leaves the
                // file position unchanged.
                (value, position.max(buffer_total), buffer.len(), true)
            }
            ReadSpec::Size(n) => {
                // Walk forward `n` chars (or however many remain) to find
                // their combined byte length — O(chars-taken), not O(position).
                let take = buffer_total.saturating_sub(position).min(n);
                let bytes_taken = tail.char_indices().nth(take).map_or(tail.len(), |(i, _)| i);
                let slice = &tail[..bytes_taken];
                let value = allocate_string(slice.to_owned(), vm.heap)?;
                let new_pos = position + take;
                let new_byte_pos = byte_position + bytes_taken;
                (value, new_pos, new_byte_pos, new_pos >= buffer_total)
            }
            ReadSpec::Line => {
                let (slice, chars_consumed) = match tail.find('\n') {
                    Some(rel) => {
                        let line = &tail[..=rel];
                        (line, line.chars().count())
                    }
                    None => (tail, tail.chars().count()),
                };
                let value = allocate_string(slice.to_owned(), vm.heap)?;
                let new_pos = position + chars_consumed;
                let new_byte_pos = byte_position + slice.len();
                (value, new_pos, new_byte_pos, new_pos >= buffer_total)
            }
            ReadSpec::Lines => {
                let mut items: Vec<Value> = Vec::new();
                let mut start = 0usize;
                while start < tail.len() {
                    let rest = &tail[start..];
                    let end = rest.find('\n').map_or(rest.len(), |i| i + 1);
                    let line = &rest[..end];
                    items.push(allocate_string(line.to_owned(), vm.heap)?);
                    start += end;
                }
                let list_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
                // Past-EOF preservation: matches `ReadSpec::All`.
                (Value::Ref(list_id), position.max(buffer_total), buffer.len(), true)
            }
            ReadSpec::Seek { offset, whence } => {
                let target = resolve_seek_target_usize(offset, whence, position, buffer_total)?;
                let target_usize = usize::try_from(target).map_err(|_| ExcType::overflow_c_ssize_t())?;
                // O(target) walk to refresh the byte cache. Seeks are rare
                // relative to reads, so paying for the walk here keeps the
                // per-read cost O(1) instead of O(position).
                let target_clamped = target_usize.min(buffer_total);
                let new_byte_pos = nth_char_byte_offset(buffer, target_clamped);
                (
                    Value::Int(target),
                    target_usize,
                    new_byte_pos,
                    target_usize >= buffer_total,
                )
            }
        }
    };

    update_file_state(file_id, new_position, new_byte_position, eof, vm)?;
    Ok(value)
}

/// Binary-mode slice computation.
///
/// `position` is the byte index, `buffer_total` is `buffer.len()` (cached
/// from [`BufferMeta`]). The buffer is read as a borrowed `&[u8]` and the
/// returned slice is the only bytes that get cloned (previously the *entire*
/// buffer was cloned on every call so the heap borrow could be released
/// before `update_position_eof`).
fn compute_slice_binary(
    file_id: HeapId,
    buffer_id: HeapId,
    position: usize,
    buffer_total: usize,
    spec: ReadSpec,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    // `seek()` allows positioning past EOF; clamp here so the slice index
    // operations below never panic when `position > len`. The cap is per-call
    // (we don't write it back to the file) so a subsequent `seek(0)` still
    // sees the original out-of-range position semantics on the seek path.
    let clamped_position = position.min(buffer_total);

    let (value, new_position, eof) = {
        let buffer = match vm.heap.get(buffer_id) {
            HeapData::Bytes(b) => b.as_slice(),
            _ => return Err(RunError::internal("compute_slice_binary: buffer is not Bytes")),
        };
        debug_assert_eq!(buffer.len(), buffer_total);
        let tail = &buffer[clamped_position..];

        match spec {
            ReadSpec::All => {
                let value = if clamped_position == 0 {
                    vm.heap.inc_ref(buffer_id);
                    Value::Ref(buffer_id)
                } else {
                    let id = vm.heap.allocate(HeapData::Bytes(Bytes::new(tail.to_vec())))?;
                    Value::Ref(id)
                };
                // Preserve `position` if it was already past `buffer_total`
                // (set there by `seek()`) — CPython's read-at-EOF leaves the
                // file position unchanged.
                (value, position.max(buffer_total), true)
            }
            ReadSpec::Size(n) => {
                let take = tail.len().min(n);
                let id = vm.heap.allocate(HeapData::Bytes(Bytes::new(tail[..take].to_vec())))?;
                // Advance from the un-clamped user position so a past-EOF
                // `read(N)` (which yields no bytes) leaves `position` alone
                // instead of snapping it back to `buffer_total`.
                let new_pos = position + take;
                (Value::Ref(id), new_pos, new_pos >= buffer_total)
            }
            ReadSpec::Line => {
                let end = tail.iter().position(|b| *b == b'\n').map_or(tail.len(), |i| i + 1);
                let id = vm.heap.allocate(HeapData::Bytes(Bytes::new(tail[..end].to_vec())))?;
                // See `Size` above — past-EOF `readline()` returns `b''`
                // without rewinding `position`.
                let new_pos = position + end;
                (Value::Ref(id), new_pos, new_pos >= buffer_total)
            }
            ReadSpec::Lines => {
                let mut items: Vec<Value> = Vec::new();
                let mut start = 0usize;
                while start < tail.len() {
                    let rest = &tail[start..];
                    let end = rest.iter().position(|b| *b == b'\n').map_or(rest.len(), |i| i + 1);
                    let id = vm.heap.allocate(HeapData::Bytes(Bytes::new(rest[..end].to_vec())))?;
                    items.push(Value::Ref(id));
                    start += end;
                }
                let list_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
                // Past-EOF preservation: matches `ReadSpec::All`.
                (Value::Ref(list_id), position.max(buffer_total), true)
            }
            ReadSpec::Seek { offset, whence } => {
                // Seek resolves against the un-clamped position so
                // `seek(0, 1)` from a past-EOF position uses the user-visible
                // target rather than the buffer-clamped one.
                let target = resolve_seek_target_usize(offset, whence, position, buffer_total)?;
                let target_usize = usize::try_from(target).map_err(|_| ExcType::overflow_c_ssize_t())?;
                (Value::Int(target), target_usize, target_usize >= buffer_total)
            }
        }
    };

    // Binary mode keeps `byte_position == min(position, buffer.len())` so
    // the cache stays consistent with the text-mode invariant.
    update_file_state(file_id, new_position, new_position.min(buffer_total), eof, vm)?;
    Ok(value)
}

/// Convenience wrapper around [`resolve_seek_target`] that widens
/// `usize` inputs to `i64` once, returning the resolved target as `i64`.
///
/// Returns an `OverflowError` if either input exceeds `i64::MAX`, which can
/// only happen with a > 8 EiB buffer — outside the resource-tracker
/// envelope. Centralising the conversion here keeps the call sites free of
/// `as i64` casts that clippy would flag.
fn resolve_seek_target_usize(offset: i64, whence: i64, position: usize, buffer_len: usize) -> RunResult<i64> {
    let position = i64::try_from(position).map_err(|_| ExcType::overflow_c_ssize_t())?;
    let buffer_len = i64::try_from(buffer_len).map_err(|_| ExcType::overflow_c_ssize_t())?;
    resolve_seek_target(offset, whence, position, buffer_len)
}

/// Resolves a `(offset, whence)` pair against the current `position` and
/// `buffer_len` (in chars for text mode, bytes for binary). Returns the
/// absolute non-negative target. Raises CPython-matched exceptions for
/// invalid `whence` or negative result.
fn resolve_seek_target(offset: i64, whence: i64, position: i64, buffer_len: i64) -> RunResult<i64> {
    let target = match whence {
        0 => offset,
        1 => position.checked_add(offset).ok_or_else(ExcType::overflow_c_ssize_t)?,
        2 => buffer_len.checked_add(offset).ok_or_else(ExcType::overflow_c_ssize_t)?,
        _ => {
            // Matches CPython's `BufferedReader.seek(0, 99)` message. (CPython's
            // `TextIOWrapper.seek` produces a different `"invalid whence
            // (99, should be 0, 1 or 2)"` string, but `BufferedReader` is the
            // form most generic code paths exercise.)
            return Err(
                SimpleException::new_msg(ExcType::ValueError, format!("whence value {whence} unsupported")).into(),
            );
        }
    };
    if target < 0 {
        // CPython's `BufferedReader.seek(-1)` raises `OSError(22, 'Invalid argument')`
        // which `__str__`s as "[Errno 22] Invalid argument". We match that
        // format to keep `except OSError as e: assert str(e) == ...` parity
        // for code paths shared by both interpreters.
        return Err(SimpleException::new_msg(ExcType::OSError, "[Errno 22] Invalid argument").into());
    }
    Ok(target)
}

/// Helper: returns the byte index of the `nth` character in `s`, or `s.len()`
/// when `nth >= s.chars().count()`.
fn nth_char_byte_offset(s: &str, nth: usize) -> usize {
    s.char_indices().nth(nth).map_or(s.len(), |(i, _)| i)
}

/// Writes `position`, the cached `byte_position`, and `eof` back to the file.
///
/// Done as a separate function so the immutable buffer borrow used by the
/// slice computation can be released before we acquire the mutable file
/// borrow. All three values are `usize` for caller convenience; the widening
/// to `u64` happens here at the single boundary so the slice code stays free
/// of `as u64` casts that clippy would flag.
fn update_file_state(
    file_id: HeapId,
    new_position: usize,
    new_byte_position: usize,
    eof: bool,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    // `usize > u64` is impossible on every platform we support, but
    // surfacing the conversion as an `expect` (rather than `as u64`) keeps
    // the assumption explicit and would survive a hypothetical wider-than-u64
    // target without silently truncating.
    let new_position = u64::try_from(new_position).expect("usize fits in u64");
    let new_byte_position = u64::try_from(new_byte_position).expect("usize fits in u64");
    let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
        return Err(RunError::internal(
            "update_file_state: file_id does not point to an OpenFile",
        ));
    };
    let f = file.get_mut(vm.heap);
    f.position = new_position;
    f.eof = eof;
    // `buffer_meta` is guaranteed to be `Some` whenever a compute_slice path
    // reaches this point (the function is only called after the buffer load
    // populated the cache). Update it in place; if it was somehow `None`,
    // initialise the cache from scratch using the values we already have.
    let buffer_total = f.buffer_meta.as_ref().map_or(new_byte_position, |m| m.buffer_total);
    f.buffer_meta = Some(BufferMeta {
        byte_position: new_byte_position,
        buffer_total,
    });
    f.file_length = buffer_total;
    Ok(())
}

/// Populates [`OpenFile::buffer_meta`] from the just-loaded buffer.
///
/// Called once by [`apply_buffer_store`] right after installing `buffer`.
/// Text mode walks the buffer to count chars and to find the byte offset
/// matching the current `position`; binary mode sets the cache to trivial
/// values derived from `buffer.len()` and `position`. The walk is O(buffer)
/// for text but happens exactly once per file (the cache is then maintained
/// incrementally by [`update_file_state`]).
fn populate_buffer_meta(file_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<()> {
    // Pull what we need out of the file under a short scoped borrow so we
    // can call `heap.get(buffer_id)` and `file.get_mut` separately without
    // overlapping borrows.
    let (buffer_id, position, binary, already_populated) = {
        let HeapReadOutput::OpenFile(file) = vm.heap.read(file_id) else {
            return Err(RunError::internal(
                "populate_buffer_meta: file_id does not point to an OpenFile",
            ));
        };
        let f = file.get(vm.heap);
        let Some(buffer_id) = f.buffer else {
            return Err(RunError::internal("populate_buffer_meta: buffer must be loaded"));
        };
        (buffer_id, f.position, f.mode.is_binary(), f.buffer_meta.is_some())
    };

    if already_populated {
        // The defensive branch in `apply_buffer_store` kept the prior buffer
        // alive — its cache is already correct.
        return Ok(());
    }

    let meta = match (vm.heap.get(buffer_id), binary) {
        (HeapData::Bytes(b), true) => {
            let len = b.as_slice().len() as u64;
            BufferMeta {
                byte_position: position.min(len),
                buffer_total: len,
            }
        }
        (HeapData::Str(s), false) => {
            let s = s.as_str();
            let char_count = s.chars().count();
            let pos_clamped = usize::try_from(position).unwrap_or(usize::MAX).min(char_count);
            BufferMeta {
                byte_position: nth_char_byte_offset(s, pos_clamped) as u64,
                buffer_total: char_count as u64,
            }
        }
        _ => {
            return Err(RunError::internal(
                "populate_buffer_meta: buffer type does not match file mode",
            ));
        }
    };

    let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
        return Err(RunError::internal(
            "populate_buffer_meta: file_id does not point to an OpenFile",
        ));
    };
    file.get_mut(vm.heap).buffer_meta = Some(meta);
    Ok(())
}

/// Parses `(offset: int, whence: int = 0)` for `file.seek()`. Mirrors
/// CPython's argument validation: missing `offset` raises `TypeError`,
/// `whence` outside `{0, 1, 2}` is deferred to `compute_slice` so the error
/// matches CPython's `invalid whence` message.
fn parse_seek_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<(i64, i64)> {
    let (offset, maybe_whence) = args.get_one_two_args("seek", vm.heap)?;
    let offset_int = offset.as_int(vm)?;
    let whence_int = match maybe_whence {
        Some(w) => w.as_int(vm)?,
        None => 0,
    };
    Ok((offset_int, whence_int))
}

/// Parses the optional `size` argument to `read()`.
///
/// CPython accepts `None` as "read all" and treats `bool` as an integer for
/// this argument. Heap-backed integer arguments are explicitly dropped after
/// conversion because `get_zero_one_arg` transfers ownership to the caller.
fn parse_read_size_arg(size_arg: Option<Value>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<ReadSpec> {
    let Some(size) = size_arg else {
        return Ok(ReadSpec::All);
    };
    let spec = match &size {
        Value::None => Ok(ReadSpec::All),
        Value::Bool(false) => Ok(ReadSpec::Size(0)),
        Value::Bool(true) => Ok(ReadSpec::Size(1)),
        _ => match size.as_int(vm) {
            Ok(n) if n < 0 => Ok(ReadSpec::All),
            Ok(n) => usize::try_from(n)
                .map(ReadSpec::Size)
                .map_err(|_| ExcType::overflow_c_ssize_t()),
            Err(err) => Err(err),
        },
    };
    size.drop_with_heap(vm);
    spec
}

/// Returns the empty `str` / `bytes` short-circuit result.
///
/// Uses the pre-interned [`StaticStrings::EmptyString`] for the text-mode
/// path so a hot `read(0)` does not allocate. Binary mode still allocates
/// a fresh empty `Bytes` because there is no equivalent interned bytes
/// singleton.
fn empty_result(binary: bool, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if binary {
        let id = heap.allocate(HeapData::Bytes(Bytes::new(Vec::new())))?;
        Ok(Value::Ref(id))
    } else {
        Ok(Value::InternString(StaticStrings::EmptyString.into()))
    }
}

/// Validates that `write()` receives text for text files and bytes for binary files.
fn validate_write_data(data: &Value, binary: bool, vm: &VM<'_, impl ResourceTracker>) -> RunResult<()> {
    if binary {
        if is_bytes(data, vm.heap) {
            Ok(())
        } else {
            Err(ExcType::type_error(format!(
                "a bytes-like object is required, not '{}'",
                data.py_type(vm)
            )))
        }
    } else if data.is_str(vm.heap) {
        Ok(())
    } else {
        Err(ExcType::type_error(format!(
            "write() argument must be str, not {}",
            data.py_type(vm)
        )))
    }
}

/// Owned `String` from a value pre-validated as a Python `str` (returns
/// `None` only if `validate_write_data` was bypassed — caller unwraps).
fn extract_str_payload(data: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<String> {
    match data {
        Value::InternString(id) => Some(vm.interns.get_str(*id).to_owned()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// Owned `Vec<u8>` from a value pre-validated as Python `bytes` — binary
/// companion to [`extract_str_payload`].
fn extract_bytes_payload(data: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<Vec<u8>> {
    match data {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id).to_owned()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => Some(b.as_slice().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// Returns whether a value is a Python `bytes` object.
fn is_bytes(data: &Value, heap: &Heap<impl ResourceTracker>) -> bool {
    match data {
        Value::InternBytes(_) => true,
        Value::Ref(id) => matches!(heap.get(*id), HeapData::Bytes(_)),
        _ => false,
    }
}

/// Builds the `io.UnsupportedOperation` used for file operations that the
/// open mode forbids (e.g. `read()` on `'w'`, `write()` on `'r'`). In CPython
/// this is a subclass of both `OSError` and `ValueError`; Monty matches both
/// in `try`/`except` matching via [`ExcType::is_subclass_of`].
fn unsupported_operation(message: &'static str) -> RunError {
    SimpleException::new_msg(ExcType::UnsupportedOperation, message).into()
}
