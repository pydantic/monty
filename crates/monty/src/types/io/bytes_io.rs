//! `io.BytesIO`: an in-memory binary stream over one `Vec<u8>`.
//!
//! Its methods keep CPython's own wording, which differs from `StringIO`'s in
//! small ways: lower-case `negative seek value`, a full stop on the closed-file
//! error, and `readlines(hint)` counting bytes up to the hint rather than past it.

use std::{fmt::Write, ops::Range};

use monty_macros::pyattrs;
use serde::{Deserialize, Serialize};

use super::{buffer_growth, non_negative, optional_size, position_value, seek_args, unsupported_no_args};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropGuard, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    py_attrs::PyAttrs,
    py_value::PyValue,
    types::{LazyHeapSet, List, PyTrait, Type, TypeRef, bytes::allocate_bytes},
    value::{EitherStr, VALUE_SIZE, Value},
};

/// The state of one `io.BytesIO`.
///
/// `pos` may lie past the end of the data; a later non-empty write pads the
/// gap with zero bytes, as CPython does.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct BytesIo {
    /// The stored bytes.
    #[serde(with = "serde_bytes")]
    buf: Vec<u8>,
    /// The stream position in bytes.
    pos: usize,
    /// Set by `close()`, which also drops the data.
    closed: bool,
}

/// `BytesIO(initial_bytes=b'')`.
#[derive(FromArgs)]
#[from_args(name = "BytesIO")]
struct BytesIoArgs {
    #[from_args(default = Value::None)]
    initial_bytes: Value,
}

impl BytesIo {
    /// The `io.BytesIO(initial_bytes=b'')` constructor; the position starts at 0.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let BytesIoArgs { initial_bytes } = BytesIoArgs::from_args(args, vm)?;
        defer_drop!(initial_bytes, vm);
        let buf = initial_payload(initial_bytes, vm)?;
        let stream = Self { buf, ..Self::default() };
        Ok(Value::Ref(vm.heap.allocate(HeapData::BytesIo(Box::new(stream)))))
    }

    /// What `copy.copy` builds: the data and position; a closed stream refuses.
    pub(crate) fn copy(&self, vm: &VM<'_>) -> RunResult<Self> {
        ensure_open(self)?;
        vm.heap.tracker.check_allocation(self.buf.len())?;
        Ok(Self {
            buf: self.buf.clone(),
            ..*self
        })
    }

    /// The bytes [`Self::write`] will allocate for `data`, for callers to preflight.
    ///
    /// CPython's `OverflowError` if the write would end past `ssize_t`, which
    /// also keeps [`Self::write`]'s arithmetic from wrapping on 32-bit targets.
    fn write_growth(&self, data: &[u8]) -> RunResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let end = self
            .pos
            .checked_add(data.len())
            .filter(|&end| isize::try_from(end).is_ok())
            .ok_or_else(ExcType::overflow_new_buffer_size_too_large)?;
        Ok(buffer_growth(end.max(self.buf.len()), self.buf.capacity()))
    }

    /// Writes `data` at the position, overwriting what is there and padding a
    /// gap past the end with zero bytes.
    fn write(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let end = self.pos + data.len();
        // One reservation for the padding and the data, as `write_growth` assumed.
        self.buf.reserve(end.saturating_sub(self.buf.len()));
        if self.pos > self.buf.len() {
            self.buf.resize(self.pos, 0);
        }
        let overlap = end.min(self.buf.len());
        self.buf[self.pos..overlap].copy_from_slice(&data[..overlap - self.pos]);
        self.buf.extend_from_slice(&data[overlap - self.pos..]);
        self.pos = end;
    }

    /// The range of up to `size` bytes from the position (everything for `None`),
    /// advancing past them.
    fn read(&mut self, size: Option<usize>) -> Range<usize> {
        let start = self.pos.min(self.buf.len());
        let end = size.map_or(self.buf.len(), |size| start.saturating_add(size).min(self.buf.len()));
        self.pos = self.pos.max(end);
        start..end
    }

    /// The range of the next `\n`-terminated line, cut to `limit` bytes when given.
    fn readline(&mut self, limit: Option<usize>) -> Range<usize> {
        let start = self.pos.min(self.buf.len());
        let tail = &self.buf[start..];
        // Only the first `limit` bytes are searched, so repeated short reads stay linear.
        let window = &tail[..limit.map_or(tail.len(), |limit| limit.min(tail.len()))];
        let end = start + window.iter().position(|&b| b == b'\n').map_or(window.len(), |i| i + 1);
        self.pos = self.pos.max(end);
        start..end
    }
}

impl HeapItem for BytesIo {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // The data is plain Rust bytes.
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, BytesIo> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::BytesIo.into()
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Streams hash by identity, as any object without `__eq__` does.
    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        self.py_default_repr_fmt(f, vm)
    }

    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterator(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// `iter(b) is b`; unlike `StringIO`, a closed `BytesIO` is still returned.
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let line = stream.readline(None);
        Ok((!line.is_empty())
            .then(|| self.copy_out(line, vm))
            .transpose()?
            .map(CallResult::Value))
    }

    fn py_is_context_manager(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        ensure_open(self.get(vm.heap))?;
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    /// Closes the stream; returning `None` lets any exception propagate.
    fn py_exit(&mut self, vm: &mut VM<'h>, _exc: Option<HeapId>) -> RunResult<CallResult> {
        close(self.get_mut(vm.heap));
        Ok(CallResult::Value(Value::None))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok((attr.static_string(vm.interns) == Some(StaticStrings::Closed))
            .then(|| CallResult::Value(Value::Bool(self.get(vm.heap).closed))))
    }
}

/// `BytesIO`'s methods; the `closed` attribute stays in `py_getattr`.
pub(crate) struct PyBytesIoImpl;

#[pyattrs]
impl<'heap> PyAttrs<'heap> for PyBytesIoImpl {
    type Receiver = HeapObjectRead<'heap, BytesIo>;

    /// Re-running the constructor reopens the stream and, as in CPython, empties it
    /// before checking the payload, so a rejected payload leaves it empty.
    #[pymethod(DunderInit)]
    fn __init__<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let BytesIoArgs { initial_bytes } = BytesIoArgs::from_args(args, vm)?;
        defer_drop!(initial_bytes, vm);
        *this.get_mut(vm.heap) = BytesIo::default();
        this.get_mut(vm.heap).buf = initial_payload(initial_bytes, vm)?;
        Ok(Value::None)
    }

    #[pymethod(Read)]
    fn read<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.read(args, "BytesIO.read", vm)
    }

    #[pymethod(Read1)]
    fn read1<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.read(args, "BytesIO.read1", vm)
    }

    #[pymethod(Readline)]
    fn readline<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = optional_size(args.get_zero_one_arg("BytesIO.readline", vm.heap)?, vm)?;
        let stream = this.get_mut(vm.heap);
        ensure_open(stream)?;
        let line = stream.readline(non_negative(size));
        this.copy_out(line, vm)
    }

    #[pymethod(Readlines)]
    fn readlines<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.readlines(args, vm)
    }

    #[pymethod(Write)]
    fn write<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let data = args.get_one_arg("BytesIO.write", vm.heap)?;
        defer_drop!(data, vm);
        this.write_value(data, vm)
    }

    #[pymethod(Writelines)]
    fn writelines<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.writelines(args, vm)
    }

    #[pymethod(Getvalue)]
    fn getvalue<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        args.check_zero_args("BytesIO.getvalue", vm.heap)?;
        let stream = this.get(vm.heap);
        ensure_open(stream)?;
        this.copy_out(0..stream.buf.len(), vm)
    }

    #[pymethod(Tell)]
    fn tell<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        args.check_zero_args("BytesIO.tell", vm.heap)?;
        let stream = this.get(vm.heap);
        ensure_open(stream)?;
        Ok(position_value(stream.pos))
    }

    #[pymethod(Seek)]
    fn seek<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.seek(args, vm)
    }

    #[pymethod(Truncate)]
    fn truncate<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        this.truncate(args, vm)
    }

    #[pymethod(Close)]
    fn close<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        args.check_zero_args("BytesIO.close", vm.heap)?;
        close(this.get_mut(vm.heap));
        Ok(Value::None)
    }

    #[pymethod(Flush)]
    fn flush<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        check_open(this, "BytesIO.flush", args, vm).map(|()| Value::None)
    }

    #[pymethod(Readable)]
    fn readable<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        check_open(this, "BytesIO.readable", args, vm).map(|()| Value::Bool(true))
    }

    #[pymethod(Writable)]
    fn writable<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        check_open(this, "BytesIO.writable", args, vm).map(|()| Value::Bool(true))
    }

    #[pymethod(Seekable)]
    fn seekable<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        check_open(this, "BytesIO.seekable", args, vm).map(|()| Value::Bool(true))
    }

    #[pymethod(Isatty)]
    fn isatty<'h>(this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        check_open(this, "BytesIO.isatty", args, vm).map(|()| Value::Bool(false))
    }

    #[pymethod(Fileno)]
    fn fileno<'h>(_this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        unsupported_no_args("fileno", args, vm)
    }

    #[pymethod(Detach)]
    fn detach<'h>(_this: &mut HeapObjectRead<'h, BytesIo>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        unsupported_no_args("detach", args, vm)
    }
}

/// Rejects arguments to a zero-argument method, then checks the stream is open.
fn check_open<'h>(this: &HeapObjectRead<'h, BytesIo>, name: &str, args: ArgValues, vm: &mut VM<'h>) -> RunResult<()> {
    args.check_zero_args(name, vm.heap)?;
    ensure_open(this.get(vm.heap))
}

impl<'h> HeapObjectRead<'h, BytesIo> {
    /// A new `bytes` of the buffer's `range`, preflighted since the stream can be most of the budget.
    fn copy_out(&self, range: Range<usize>, vm: &VM<'h>) -> RunResult<Value> {
        vm.heap.tracker.check_allocation(range.len())?;
        Ok(allocate_bytes(self.get(vm.heap).buf[range].to_vec(), vm.heap))
    }

    /// `read(size=-1, /)` and `read1(size=-1, /)`: a negative or `None` size reads to the end.
    fn read(&self, args: ArgValues, name: &str, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = optional_size(args.get_zero_one_arg(name, vm.heap)?, vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let data = stream.read(non_negative(size));
        self.copy_out(data, vm)
    }

    /// `readlines(hint=None, /)`: lines until their total reaches a positive
    /// `hint`. CPython checks for closure before converting the hint, and
    /// accepts only real `int`s, not `__index__` objects.
    fn readlines(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let hint = args.get_zero_one_arg("BytesIO.readlines", vm.heap)?;
        defer_drop!(hint, vm);
        ensure_open(self.get(vm.heap))?;
        let hint = match hint {
            None | Some(Value::None) => None,
            Some(value @ (Value::Int(_) | Value::Bool(_))) => non_negative(Some(value.as_int(vm)?)),
            Some(value) if value.as_long_int(vm).is_some() => return Err(ExcType::overflow_c_ssize_t()),
            Some(value) => return Err(ExcType::type_error_integer_argument_expected(&value.py_type_name(vm))),
        };
        // Guarded so a limit error mid-loop releases the lines built so far.
        let mut lines_guard = DropGuard::new(Vec::new(), vm);
        let (lines, vm) = lines_guard.as_parts_mut();
        let mut total = 0usize;
        loop {
            vm.heap.tracker.check_memory_time_every(lines.len())?;
            let line = self.get_mut(vm.heap).readline(None);
            if line.is_empty() {
                break;
            }
            total += line.len();
            // Short lines cost no heap entry, so the poll alone misses the list's own doubling.
            vm.heap
                .tracker
                .check_growth(lines.len(), lines.capacity(), VALUE_SIZE)?;
            lines.push(self.copy_out(line, vm)?);
            if hint.is_some_and(|hint| hint > 0 && total >= hint) {
                break;
            }
        }
        let (lines, vm) = lines_guard.into_parts();
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(lines)))))
    }

    /// Writes one `bytes` value, returning its length; the type is checked first.
    fn write_value(&self, data: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let PyValue::Bytes(bytes) = data.py_value(vm) else {
            return Err(ExcType::type_error_bytes_like_required(&data.py_type_name(vm)));
        };
        ensure_open(self.get(vm.heap))?;
        let data = copy_payload(bytes.as_slice(vm), vm)?;
        let stream = self.get(vm.heap);
        vm.heap.tracker.check_pending_allocation(stream.write_growth(&data)?)?;
        self.get_mut(vm.heap).write(&data);
        Ok(position_value(data.len()))
    }

    /// `writelines(lines, /)`: `write()` for each item, refused once closed.
    fn writelines(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let lines = args.get_one_arg("BytesIO.writelines", vm.heap)?;
        if let Err(err) = ensure_open(self.get(vm.heap)) {
            lines.drop_with(vm);
            return Err(err);
        }
        let iter = lines.into_py_iter(vm)?.require_synchronous(vm)?;
        defer_drop!(iter, vm);
        let mut iter = iter.read(vm);
        while let Some(line) = iter.py_next(vm)? {
            defer_drop!(line, vm);
            self.write_value(line, vm)?;
        }
        Ok(Value::None)
    }

    /// `seek(pos, whence=0, /)`: any offset from any anchor, clamped at 0.
    fn seek(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let (pos, whence) = seek_args(args, "BytesIO.seek", vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        if pos < 0 && whence == 0 {
            return Err(ExcType::value_error(format!("negative seek value {pos}")));
        }
        let anchor = match whence {
            0 => 0,
            1 => stream.pos,
            2 => stream.buf.len(),
            _ => return Err(ExcType::value_error_invalid_whence(whence, false)),
        };
        // `pos` fits `isize` (see `seek_args`), so the sum overflows where CPython's `Py_ssize_t` does.
        let target = isize::try_from(anchor)
            .ok()
            .and_then(|anchor| anchor.checked_add(isize::try_from(pos).ok()?))
            .ok_or_else(ExcType::overflow_new_position_too_large)?
            .max(0)
            .unsigned_abs();
        stream.pos = target;
        Ok(position_value(target))
    }

    /// `truncate(size=None, /)`: `None` truncates at the position, which is unchanged.
    fn truncate(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = args.get_zero_one_arg("BytesIO.truncate", vm.heap)?;
        defer_drop!(size, vm);
        ensure_open(self.get(vm.heap))?;
        let size = match size {
            None | Some(Value::None) => self.get(vm.heap).pos,
            Some(size) => match size.as_int_with_overflow(vm, ExcType::overflow_c_long)? {
                size if size < 0 => return Err(ExcType::value_error(format!("negative size value {size}"))),
                // A C `long` is `ssize_t`-sized on the targets that bound it below `i64`.
                size => isize::try_from(size)
                    .ok()
                    .and_then(|size| usize::try_from(size).ok())
                    .ok_or_else(ExcType::overflow_c_long)?,
            },
        };
        let stream = self.get_mut(vm.heap);
        if size < stream.buf.len() {
            stream.buf.truncate(size);
        }
        Ok(position_value(size))
    }
}

/// The constructor's `initial_bytes`: empty for `None`, else a copy of the `bytes`.
fn initial_payload(initial_bytes: &Value, vm: &VM<'_>) -> RunResult<Vec<u8>> {
    match initial_bytes {
        Value::None => Ok(Vec::new()),
        value => bytes_payload(value, vm),
    }
}

/// A copy of a `bytes` value's data, or CPython's bytes-like `TypeError`.
fn bytes_payload(value: &Value, vm: &VM<'_>) -> RunResult<Vec<u8>> {
    match value.py_value(vm) {
        PyValue::Bytes(bytes) => copy_payload(bytes.as_slice(vm), vm),
        _ => Err(ExcType::type_error_bytes_like_required(&value.py_type_name(vm))),
    }
}

/// A copy of `data`, preflighted since a payload can be most of the budget.
fn copy_payload(data: &[u8], vm: &VM<'_>) -> RunResult<Vec<u8>> {
    vm.heap.tracker.check_allocation(data.len())?;
    Ok(data.to_vec())
}

/// The `ValueError` every `BytesIO` method raises once it is closed.
fn ensure_open(stream: &BytesIo) -> RunResult<()> {
    if stream.closed {
        Err(ExcType::value_error_closed_file())
    } else {
        Ok(())
    }
}

/// Marks the stream closed and releases its data, as CPython frees the buffer.
fn close(stream: &mut BytesIo) {
    stream.closed = true;
    stream.buf = Vec::new();
}
