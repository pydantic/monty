//! Heap-backed Python file wrappers used by the `open()` builtin.
//!
//! Monty does not keep native file descriptors open inside the sandbox.  These
//! objects store only the virtual path, requested mode, and small Python-visible
//! state such as `closed`.  Each `read()` or `write()` call is a complete
//! one-shot [`OsFunction`](crate::os::OsFunction) operation, so host filesystem
//! access remains mediated by the same boundary used by `pathlib.Path`.

use std::{borrow::Cow, fmt::Write, mem, str::FromStr};

use ahash::AHashSet;

use super::{PyTrait, Type, str::allocate_string};
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
/// Carried publicly by [`MontyObject::FileHandle`] so a host servicing file
/// operations can inspect the mode without re-parsing the raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FileMode {
    /// `r` / `rb`: read-only; the file must already exist.
    Read(bool),
    /// `r+` / `rb+`: read and write an existing file; nothing happens at open time.
    ReadUpdate(bool),
    /// `w` / `wb`: write-only; truncate the file (creating it if missing) on open.
    Write(bool),
    /// `w+` / `wb+`: read and write; truncate the file (creating it if missing) on open.
    WriteUpdate(bool),
    /// `a` / `ab`: write-only appending; create the file if missing, preserving content.
    Append(bool),
    /// `a+` / `ab+`: read and append; create the file if missing, preserving content.
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
            return Err("Invalid mode: empty".into());
        }

        let mut action = None;
        let mut binary = false;
        let mut text = false;
        let mut updating = false;

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
                '+' => {
                    if updating {
                        return Err("invalid mode: update mode specified twice".into());
                    }
                    updating = true;
                }
                _ => return Err(format!("invalid mode: unknown mode character {ch:?}").into()),
            }
        }

        if binary && text {
            return Err("can't have text and binary mode at once".into());
        }

        Ok(match (action.unwrap_or('r'), updating) {
            ('w', false) => Self::Write(binary),
            ('w', true) => Self::WriteUpdate(binary),
            ('a', false) => Self::Append(binary),
            ('a', true) => Self::AppendUpdate(binary),
            (_, false) => Self::Read(binary),
            (_, true) => Self::ReadUpdate(binary),
        })
    }
}

/// Whether a write-mode file has already issued its initial truncating write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum WriteState {
    /// No write has been issued yet.
    Fresh,
    /// At least one write has been issued.
    Written,
}

/// Whether a file wrapper should accept further operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum FileState {
    /// The wrapper is open.
    Open,
    /// The wrapper has been closed.
    Closed,
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
    write_state: WriteState,
    state: FileState,
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
    /// host at `open()` time, so the wrapper starts in [`WriteState::Written`]:
    /// the first user `write()` should append rather than truncate again.
    #[must_use]
    pub fn with_state(path: String, mode: FileMode, position: u64, id: Option<u64>) -> Self {
        let write_state = if mode.truncate() {
            WriteState::Written
        } else {
            WriteState::Fresh
        };
        Self {
            path,
            mode,
            write_state,
            state: FileState::Open,
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

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let Some(method) = attr.static_string() else {
            return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)));
        };

        let file = self.get(vm.heap);
        let value = match method {
            StaticStrings::Name => allocate_string(file.path.clone(), vm.heap)?,
            StaticStrings::Mode => allocate_string(file.mode.as_str().to_owned(), vm.heap)?,
            StaticStrings::Closed => Value::Bool(matches!(file.state, FileState::Closed)),
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
    fn read(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        args.check_zero_args("read", vm.heap)?;
        let binary = {
            let file = self.get(vm.heap);
            file.ensure_open()?;
            if !file.mode.readable() {
                return Err(unsupported_operation("not readable"));
            }
            file.mode.is_binary()
        };

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
            let function = if file.mode.is_binary() {
                if file.mode.is_append() || matches!(file.write_state, WriteState::Written) {
                    OsFunction::AppendBytes
                } else {
                    OsFunction::WriteBytes
                }
            } else if file.mode.is_append() || matches!(file.write_state, WriteState::Written) {
                OsFunction::AppendText
            } else {
                OsFunction::WriteText
            };
            file.write_state = WriteState::Written;
            function
        };

        vm.heap.inc_ref(self_id);
        Ok(CallResult::OsCall(function, ArgValues::Two(Value::Ref(self_id), data)))
    }

    /// Marks the file wrapper as closed.
    fn close(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("close", vm.heap)?;
        self.get_mut(vm.heap).state = FileState::Closed;
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

    /// Returns `False`; Monty's file wrappers currently expose no seek state.
    fn seekable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("seekable", vm.heap)?;
        self.get(vm.heap).ensure_open()?;
        Ok(CallResult::Value(Value::Bool(false)))
    }
}

impl OpenFile {
    /// Raises the CPython-style error used for operations after `close()`.
    fn ensure_open(&self) -> RunResult<()> {
        if matches!(self.state, FileState::Closed) {
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

/// Builds the OSError used for unsupported file operations.
fn unsupported_operation(message: &'static str) -> RunError {
    SimpleException::new_msg(ExcType::OSError, message).into()
}
