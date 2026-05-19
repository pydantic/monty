//! Heap-backed Python file wrappers used by the `open()` builtin.
//!
//! Monty does not keep native file descriptors open inside the sandbox.  These
//! objects store only the virtual path, requested mode, and small Python-visible
//! state such as `closed`.  Each `read()` or `write()` call is a complete
//! one-shot [`OsFunction`](crate::os::OsFunction) operation, so host filesystem
//! access remains mediated by the same boundary used by `pathlib.Path`.

use std::{fmt::Write, mem};

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

/// The concrete `_io` wrapper type exposed by `type(open(...))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FileKind {
    /// Text-mode files, including read, write, and append modes.
    Text,
    /// Binary read-only files opened with modes such as `"rb"`.
    BufferedReader,
    /// Binary write or append files opened with modes such as `"wb"` or `"ab"`.
    BufferedWriter,
    /// Binary read/write files opened with modes such as `"r+b"`.
    BufferedRandom,
}

/// Read/write capability granted by an `open()` mode string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FileAccess {
    /// Read-only mode.
    Read,
    /// Write-only mode.
    Write,
    /// Read/write update mode.
    ReadWrite,
}

impl FileAccess {
    /// Returns whether `read()` is allowed by this access mode.
    #[must_use]
    pub const fn readable(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    /// Returns whether `write()` is allowed by this access mode.
    #[must_use]
    pub const fn writable(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
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

/// Parsed `open()` mode and the behavior Monty should expose for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenMode {
    /// The mode string preserved for the file object's `mode` attribute.
    pub mode: String,
    /// The concrete heap type to allocate.
    pub kind: FileKind,
    /// Whether the mode allows reading, writing, or both.
    pub access: FileAccess,
    /// Whether writes should always append.
    pub append: bool,
    /// Whether the file expects bytes instead of text.
    pub binary: bool,
}

impl OpenMode {
    /// Parses the Python `open()` mode string for Monty's supported file modes.
    ///
    /// Monty currently supports the common read, write, append, and update
    /// combinations in text or binary form. Exclusive creation (`x`) is rejected
    /// for now because it needs a dedicated mount-table operation to be race-free.
    pub fn parse(mode: &str) -> RunResult<Self> {
        if mode.is_empty() {
            return Err(invalid_mode(mode));
        }

        let mut action = None;
        let mut binary = false;
        let mut text = false;
        let mut updating = false;

        for ch in mode.chars() {
            match ch {
                'r' | 'w' | 'a' => {
                    if action.replace(ch).is_some() {
                        return Err(one_action_mode_error());
                    }
                }
                'x' => {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "exclusive creation mode is not supported",
                    )
                    .into());
                }
                'b' => {
                    if binary {
                        return Err(invalid_mode(mode));
                    }
                    binary = true;
                }
                't' => {
                    if text {
                        return Err(invalid_mode(mode));
                    }
                    text = true;
                }
                '+' => {
                    if updating {
                        return Err(invalid_mode(mode));
                    }
                    updating = true;
                }
                _ => return Err(invalid_mode(mode)),
            }
        }

        if binary && text {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "can't have text and binary mode at once").into(),
            );
        }

        let action = action.unwrap_or('r');
        let access = if updating {
            FileAccess::ReadWrite
        } else if action == 'r' {
            FileAccess::Read
        } else {
            FileAccess::Write
        };
        let append = action == 'a';
        let kind = if binary {
            if updating {
                FileKind::BufferedRandom
            } else if access.readable() {
                FileKind::BufferedReader
            } else {
                FileKind::BufferedWriter
            }
        } else {
            FileKind::Text
        };

        Ok(Self {
            mode: mode.to_owned(),
            kind,
            access,
            append,
            binary,
        })
    }
}

/// A Python file object that stores path and mode state, but no native handle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OpenFile {
    path: String,
    mode: String,
    kind: FileKind,
    access: FileAccess,
    append: bool,
    binary: bool,
    write_state: WriteState,
    state: FileState,
}

impl OpenFile {
    /// Creates a new path-backed file wrapper from a parsed `open()` mode.
    #[must_use]
    pub fn new(path: String, mode: OpenMode) -> Self {
        Self {
            path,
            mode: mode.mode,
            kind: mode.kind,
            access: mode.access,
            append: mode.append,
            binary: mode.binary,
            write_state: WriteState::Fresh,
            state: FileState::Open,
        }
    }

    /// Returns the virtual path used for OS calls.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the mode string shown to Python code.
    #[must_use]
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// Returns the type represented by this file wrapper.
    #[must_use]
    pub fn file_type(&self) -> Type {
        match self.kind {
            FileKind::Text => Type::TextIOWrapper,
            FileKind::BufferedReader => Type::BufferedReader,
            FileKind::BufferedWriter => Type::BufferedWriter,
            FileKind::BufferedRandom => Type::BufferedRandom,
        }
    }
}

impl HeapItem for OpenFile {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.path.len() + self.mode.len()
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
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)));
        };

        match method {
            StaticStrings::Read => self.read(vm, args),
            StaticStrings::Write => self.write(vm, args),
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
            StaticStrings::Mode => allocate_string(file.mode.clone(), vm.heap)?,
            StaticStrings::Closed => Value::Bool(matches!(file.state, FileState::Closed)),
            StaticStrings::Encoding if !file.binary => allocate_string("utf-8", vm.heap)?,
            _ => return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(value)))
    }
}

impl<'h> HeapRead<'h, OpenFile> {
    /// Implements `file.read()` as a full-file OS read.
    fn read(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("read", vm.heap)?;
        let (path, binary) = {
            let file = self.get(vm.heap);
            file.ensure_open()?;
            if !file.access.readable() {
                return Err(unsupported_operation("not readable"));
            }
            (file.path.clone(), file.binary)
        };

        let path_value = allocate_string(path, vm.heap)?;
        let function = if binary {
            OsFunction::ReadBytes
        } else {
            OsFunction::ReadText
        };
        Ok(CallResult::OsCall(function, ArgValues::One(path_value)))
    }

    /// Implements `file.write(data)` as a one-shot OS write or append.
    fn write(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        let data = args.get_one_arg("write", vm.heap)?;
        let binary = self.get(vm.heap).binary;
        if let Err(err) = validate_write_data(&data, binary, vm) {
            data.drop_with_heap(vm);
            return Err(err);
        }
        let (path, function) = {
            let file = self.get_mut(vm.heap);
            file.ensure_open()?;
            if !file.access.writable() {
                let message = if file.binary { "write" } else { "not writable" };
                data.drop_with_heap(vm);
                return Err(unsupported_operation(message));
            }
            let function = if file.binary {
                if file.append || matches!(file.write_state, WriteState::Written) {
                    OsFunction::AppendBytes
                } else {
                    OsFunction::WriteBytes
                }
            } else if file.append || matches!(file.write_state, WriteState::Written) {
                OsFunction::AppendText
            } else {
                OsFunction::WriteText
            };
            file.write_state = WriteState::Written;
            (file.path.clone(), function)
        };

        let path_value = allocate_string(path, vm.heap)?;
        Ok(CallResult::OsCall(function, ArgValues::Two(path_value, data)))
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
        Ok(CallResult::Value(Value::Bool(file.access.readable())))
    }

    /// Returns whether this file object supports `write()`.
    fn writable(&mut self, vm: &mut VM<'h, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("writable", vm.heap)?;
        let file = self.get(vm.heap);
        file.ensure_open()?;
        Ok(CallResult::Value(Value::Bool(file.access.writable())))
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

/// Builds the `ValueError` used for malformed `open()` modes.
fn invalid_mode(mode: &str) -> RunError {
    SimpleException::new_msg(ExcType::ValueError, format!("invalid mode: {mode:?}")).into()
}

/// Builds the CPython error for modes with multiple open actions.
fn one_action_mode_error() -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        "must have exactly one of create/read/write/append mode",
    )
    .into()
}

/// Builds the OSError used for unsupported file operations.
fn unsupported_operation(message: &'static str) -> RunError {
    SimpleException::new_msg(ExcType::OSError, message).into()
}
