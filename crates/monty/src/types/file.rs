//! Heap-backed Python file wrappers used by the `open()` builtin.
//!
//! Monty does not keep native file descriptors open inside the sandbox.  These
//! objects store only the virtual path, requested mode, and small Python-visible
//! state such as `closed`.  Each `read()` or `write()` call is a complete
//! one-shot [`OsFunction`](crate::os::OsFunction) operation, so host filesystem
//! access remains mediated by the same boundary used by `pathlib.Path`.
//!
//! # Unsupported / diverging behavior
//!
//! The current implementation is a deliberate subset of CPython's file API.
//! Code that relies on the following will not behave the same way as on
//! CPython:
//!
//! - `read(size)` is rejected; `read()` only accepts zero arguments and
//!   always returns the full file content. A `read()` that raises in the
//!   host still marks the file consumed, so a user-caught failure followed
//!   by a retry returns empty rather than re-reading; CPython would retry.
//! - `readline()`, `readlines()`, file iteration, `seek()`, and `tell()`
//!   are not implemented. `seekable()` reports `False` (instead of
//!   CPython's `True` for regular files) so the
//!   `if f.seekable(): f.seek(0)` idiom routes to the non-seekable arm.
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
//!
//! Any code path that needs one of these should be added explicitly
//! rather than relying on CPython parity.

use std::{borrow::Cow, fmt::Write, mem, str::FromStr};

use ahash::AHashSet;

use super::{PyTrait, Type, bytes::Bytes, str::allocate_string};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::str::StringRepr,
    value::{EitherStr, Value},
};

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
/// Monty keeps no live OS file descriptor: every `read()`/`write()` is a
/// complete one-shot OS call that the host opens, performs, and closes. All
/// state needed to make those calls reproducible across a snapshot/resume —
/// `path`, `mode`, `position`, `id` — lives here and is serialized.
///
/// `position` is the byte offset future seek-aware reads (`readline`,
/// `read(size)`, `seek`) will operate from; it is plumbed end-to-end but no
/// current operation mutates it.
///
/// TODO(perf): a host may assign an `id` (otherwise `None`). A future
/// optimization could let the host cache a real OS handle keyed by that `id`,
/// seeking it to `position`, instead of re-opening the file on every call. The
/// stateless (re-open every call) model must remain the default so snapshots
/// never depend on host state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OpenFile {
    path: String,
    mode: FileMode,
    /// Whether at least one write has been issued. For `w`/`wb` mode this
    /// switches subsequent writes from truncating to appending so write #2
    /// doesn't clobber write #1. Truncating modes start `true` because the
    /// host already emptied the file at `open()` time.
    first_write_done: bool,
    /// Whether `read()` has been dispatched at least once. Monty has no
    /// read-position state, so each `read()` is a full-file OS call; further
    /// reads short-circuit to empty `str`/`bytes` to match CPython's EOF
    /// behavior without a host round-trip.
    read_consumed: bool,
    /// Whether `close()` has been called. Operations on a closed file raise
    /// `ValueError`.
    closed: bool,
    /// Byte offset for seek-aware reads (currently never mutated).
    position: u64,
    /// Optional host-assigned id for this open file (Monty never sets it).
    id: Option<u64>,
}

impl OpenFile {
    /// Creates a path-backed file wrapper from a parsed `open()` mode and the
    /// `position`/`id` carried across the host boundary by a
    /// [`MontyObject::FileHandle`](crate::MontyObject::FileHandle).
    ///
    /// Truncating modes (`w`/`w+`) have already had the file emptied by the
    /// host at `open()` time, so the wrapper starts with `first_write_done`
    /// set: the first user `write()` should append rather than truncate again.
    /// `read_consumed` always starts `false` regardless of mode — it is only
    /// consulted on `read()` and only flips after a read is dispatched.
    #[must_use]
    pub fn with_state(path: String, mode: FileMode, position: u64, id: Option<u64>) -> Self {
        Self {
            path,
            mode,
            first_write_done: mode.truncate(),
            read_consumed: false,
            closed: false,
            position,
            id,
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

    /// Returns the optional host-assigned id for this open file.
    #[must_use]
    pub fn id(&self) -> Option<u64> {
        self.id
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

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // File wrappers store only owned Rust strings and booleans.
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
    /// Implements `file.read()` as a full-file OS read.
    ///
    /// The OS call's first argument is the file object itself
    /// (`Value::Ref(self_id)`); the host boundary converts it to a
    /// [`MontyObject::FileHandle`](crate::MontyObject::FileHandle), so the host
    /// receives the path, mode, position, and id needed to service the read.
    ///
    /// Because Monty has no read-position state, a successful `read()` sets
    /// `read_consumed` and any subsequent `read()` returns an empty
    /// `str`/`bytes` value without round-tripping to the host. This matches
    /// CPython's EOF behavior for a sequential read.
    ///
    /// `read_consumed` is set *before* dispatch, not after, so a host that
    /// raises during the read still leaves the file marked consumed — a
    /// user-caught failure followed by a retry returns empty instead of
    /// re-reading. That divergence from CPython is documented in the
    /// module-level comment; tracking the success/failure outcome would
    /// require per-call state plumbed through the snapshot/resume cycle.
    fn read(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        args.check_zero_args("read", vm.heap)?;
        let (binary, already_consumed) = {
            let file = self.get(vm.heap);
            file.ensure_open()?;
            if !file.mode.readable() {
                return Err(unsupported_operation("not readable"));
            }
            (file.mode.is_binary(), file.read_consumed)
        };

        if already_consumed {
            let empty = if binary {
                Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(Vec::new())))?)
            } else {
                allocate_string("", vm.heap)?
            };
            return Ok(CallResult::Value(empty));
        }

        self.get_mut(vm.heap).read_consumed = true;

        let function = if binary {
            OsFunction::ReadBytes
        } else {
            OsFunction::ReadText
        };
        vm.heap.inc_ref(self_id);
        Ok(CallResult::OsCall(function, ArgValues::One(Value::Ref(self_id))))
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
        let function = {
            let file = self.get_mut(vm.heap);
            if !file.mode.writable() {
                let message = if file.mode.is_binary() { "write" } else { "not writable" };
                data.drop_with_heap(vm);
                return Err(unsupported_operation(message));
            }
            let append = file.mode.is_append() || file.first_write_done;
            let function = if file.mode.is_binary() {
                if append {
                    OsFunction::AppendBytes
                } else {
                    OsFunction::WriteBytes
                }
            } else if append {
                OsFunction::AppendText
            } else {
                OsFunction::WriteText
            };
            file.first_write_done = true;
            function
        };

        vm.heap.inc_ref(self_id);
        Ok(CallResult::OsCall(function, ArgValues::Two(Value::Ref(self_id), data)))
    }

    /// Marks the file wrapper as closed.
    fn close(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("close", vm.heap)?;
        self.get_mut(vm.heap).closed = true;
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

    /// Returns `False` until `seek()`/`tell()` are implemented. Reporting
    /// `True` would advertise a capability that does not exist: the common
    /// `if f.seekable(): f.seek(0)` pattern would take the CPython-compatible
    /// branch and then crash with `AttributeError` on the `seek()` call.
    /// Returning `False` here routes that pattern to the non-seekable arm,
    /// which is at least consistent.
    fn seekable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("seekable", vm.heap)?;
        self.get(vm.heap).ensure_open()?;
        Ok(CallResult::Value(Value::Bool(false)))
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
