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
    object::FileMode,
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::str::StringRepr,
    value::{EitherStr, Value},
};

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

    /// Returns the mode string shown to Python code.
    #[must_use]
    pub fn mode(&self) -> &str {
        &self.mode.mode
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
        mem::size_of::<Self>() + self.path.len() + self.mode.mode.len()
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
            StaticStrings::Mode => allocate_string(file.mode.mode.clone(), vm.heap)?,
            StaticStrings::Closed => Value::Bool(matches!(file.state, FileState::Closed)),
            StaticStrings::Encoding if !file.mode.binary => allocate_string("utf-8", vm.heap)?,
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
            file.mode.binary
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
        let binary = self.get(vm.heap).mode.binary;
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
                let message = if file.mode.binary { "write" } else { "not writable" };
                data.drop_with_heap(vm);
                return Err(unsupported_operation(message));
            }
            let function = if file.mode.binary {
                if file.mode.append() || matches!(file.write_state, WriteState::Written) {
                    OsFunction::AppendBytes
                } else {
                    OsFunction::WriteBytes
                }
            } else if file.mode.append() || matches!(file.write_state, WriteState::Written) {
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
