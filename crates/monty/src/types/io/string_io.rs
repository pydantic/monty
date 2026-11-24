//! `io.StringIO`: an in-memory text stream.
//!
//! The text lives in one Rust `String`; positions are char offsets, as in
//! CPython, so [`StringIo::byte_pos`] caches the byte offset of the current
//! position and an ASCII buffer converts between the two for free. Sequential
//! reads, `readline()` loops and appends therefore never rescan the buffer;
//! only a `seek()`/`truncate()` into non-ASCII text walks it.
//!
//! Newline translation happens as text is written, CPython's design:
//! `newline=None` stores `\r\n` and `\r` as `\n`, `'\r'` / `'\r\n'` store
//! `\n` as that terminator.

use std::{fmt::Write, iter, ops::Range};

use serde::{Deserialize, Serialize};

use super::{
    buffer_growth,
    newline::{Newline, SeenNewlines, translate_newlines},
    non_negative, optional_size, position_value, seek_args, stream_position,
};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    types::{
        LazyHeapSet, List, PyTrait, Type, TypeRef,
        str::{PyStr, allocate_string},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// The state of one `io.StringIO`.
///
/// `pos` may lie past the end of the text (`seek(100)`); a later write pads
/// the gap with `'\0'`, as CPython does. `byte_pos` is always the byte offset
/// of `min(pos, char_len)`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct StringIo {
    /// The stored text, after newline translation.
    buf: String,
    /// Chars in `buf`, so ASCII text can index by byte.
    char_len: usize,
    /// The stream position in chars.
    pos: usize,
    /// Byte offset in `buf` of the position, clamped to the end of the text.
    byte_pos: usize,
    /// The `newline` argument.
    newline: Newline,
    /// Line endings written so far, for `newlines` in the universal modes.
    seen: SeenNewlines,
    /// Set by `close()`, which also drops the text.
    closed: bool,
}

/// `StringIO(initial_value='', newline='\n')`.
#[derive(FromArgs)]
#[from_args(name = "StringIO", at_most_total)]
struct StringIoArgs {
    #[from_args(default = Value::None)]
    initial_value: Value,
    #[from_args(default)]
    newline: Option<Value>,
}

impl StringIo {
    /// The `io.StringIO(initial_value='', newline='\n')` constructor.
    ///
    /// CPython validates `newline` (type, then value) before `initial_value`,
    /// then writes the initial text through the same translation as `write()`
    /// and rewinds to the start.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let StringIoArgs { initial_value, newline } = StringIoArgs::from_args(args, vm)?;
        defer_drop!(initial_value, vm);
        let newline = parse_newline(newline, vm)?;
        let mut stream = Self {
            newline,
            ..Self::default()
        };
        match initial_value {
            Value::None => {}
            value => {
                let Some(text) = PyStr::from_value(value, vm) else {
                    return Err(ExcType::type_error_str_or_none(
                        "initial_value",
                        &value.py_type_name(vm),
                    ));
                };
                let text = text.as_str(vm);
                vm.heap.tracker.check_allocation(stream.translation_cost(text))?;
                let text = stream.translate(text.to_owned());
                vm.heap.tracker.check_pending_allocation(stream.write_growth(&text)?)?;
                stream.write(&text);
                stream.seek_to(0);
            }
        }
        Ok(Value::Ref(vm.heap.allocate(HeapData::StringIo(Box::new(stream)))))
    }

    /// What `copy.copy` builds: the text, position and newline mode.
    ///
    /// CPython's `__setstate__` re-runs `__init__` on the stored text, so the
    /// copy's `newlines` reports the endings of that text, not the writes seen.
    pub(crate) fn copy(&self, vm: &VM<'_>) -> RunResult<Self> {
        ensure_open(self)?;
        vm.heap.tracker.check_allocation(self.buf.len())?;
        let mut seen = SeenNewlines::default();
        if self.newline.is_universal() {
            seen.record(&self.buf);
        }
        Ok(Self {
            buf: self.buf.clone(),
            seen,
            ..*self
        })
    }

    /// Whether every char is one byte, so char offsets are byte offsets.
    fn is_ascii(&self) -> bool {
        self.buf.len() == self.char_len
    }

    /// Byte offset of char `char_pos`, which must not exceed `char_len`.
    fn byte_offset(&self, char_pos: usize) -> usize {
        if self.is_ascii() {
            char_pos
        } else {
            self.buf.char_indices().nth(char_pos).map_or(self.buf.len(), |(i, _)| i)
        }
    }

    /// Byte offset `chars` chars after `from`, stopping at the end of the text.
    fn advance(&self, from: usize, chars: usize) -> usize {
        if self.is_ascii() {
            (from + chars).min(self.buf.len())
        } else {
            self.buf[from..]
                .char_indices()
                .nth(chars)
                .map_or(self.buf.len(), |(i, _)| from + i)
        }
    }

    /// Moves to char `pos`, which may lie past the end of the text.
    fn seek_to(&mut self, pos: usize) {
        self.pos = pos;
        self.byte_pos = self.byte_offset(pos.min(self.char_len));
    }

    /// The `'\0'` chars a write must insert to reach a position past the end.
    fn padding(&self) -> usize {
        self.pos.saturating_sub(self.char_len)
    }

    /// Applies the newline mode to `text` as `write()` stores it, recording
    /// its line endings for `newlines` in the universal modes.
    fn translate(&mut self, text: String) -> String {
        match self.newline {
            Newline::Universal => {
                self.seen.record(&text);
                translate_newlines(&text)
            }
            Newline::Passthrough => {
                self.seen.record(&text);
                text
            }
            newline => newline.translate_write(text).0,
        }
    }

    /// The bytes the owned copy of `text` and its newline translation take before
    /// [`Self::write`]: `'\r\n'` can double the text.
    fn translation_cost(&self, text: &str) -> usize {
        let translated = match self.newline {
            Newline::Universal | Newline::Cr => text.len(),
            Newline::CrLf => text.len().saturating_mul(2),
            Newline::Lf | Newline::Passthrough => 0,
        };
        text.len().saturating_add(translated)
    }

    /// The bytes [`Self::write`] will allocate for `text`, for callers to
    /// preflight; an overwrite is counted as if it appended.
    ///
    /// CPython's `OverflowError` if the write would end past `ssize_t`, which
    /// also keeps [`Self::write`]'s arithmetic from wrapping on 32-bit targets.
    fn write_growth(&self, text: &str) -> RunResult<usize> {
        if text.is_empty() {
            return Ok(0);
        }
        let needed = self
            .pos
            .checked_add(text.len())
            .and_then(|_| self.buf.len().checked_add(self.padding()))
            .and_then(|len| len.checked_add(text.len()))
            .filter(|&needed| isize::try_from(needed).is_ok())
            .ok_or_else(ExcType::overflow_new_position_too_large)?;
        Ok(buffer_growth(needed, self.buf.capacity()))
    }

    /// Writes translated `text` at the position, overwriting what is there
    /// and padding a gap past the end with `'\0'`.
    fn write(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.pos > self.char_len {
            let pad = self.padding();
            // One reservation for the padding and the text, as `write_growth` assumed.
            self.buf.reserve(pad + text.len());
            self.buf.extend(iter::repeat_n('\0', pad));
            self.char_len = self.pos;
            self.byte_pos = self.buf.len();
        }
        let chars = text.chars().count();
        if self.pos == self.char_len {
            self.buf.push_str(text);
        } else {
            let end = self.advance(self.byte_pos, chars);
            self.buf.replace_range(self.byte_pos..end, text);
        }
        self.char_len = self.char_len.max(self.pos + chars);
        self.pos += chars;
        self.byte_pos += text.len();
    }

    /// The byte range of up to `size` chars from the position (everything for
    /// `None`), advancing past them.
    fn read(&mut self, size: Option<usize>) -> Range<usize> {
        let start = self.byte_pos;
        let remaining = self.char_len.saturating_sub(self.pos);
        let chars = size.map_or(remaining, |size| size.min(remaining));
        let end = self.advance(start, chars);
        self.pos += chars;
        self.byte_pos = end;
        start..end
    }

    /// The byte range of the next line, cut to `limit` chars when given; empty at the end.
    fn readline(&mut self, limit: Option<usize>) -> Range<usize> {
        if self.pos >= self.char_len {
            return 0..0;
        }
        let start = self.byte_pos;
        let tail = &self.buf[start..];
        let mut end = start + self.newline.line_len(tail);
        let mut chars = if self.is_ascii() {
            end - start
        } else {
            self.buf[start..end].chars().count()
        };
        if let Some(limit) = limit
            && limit < chars
        {
            end = self.advance(start, limit);
            chars = limit;
        }
        self.pos += chars;
        self.byte_pos = end;
        start..end
    }

    /// Cuts the text to `size` chars, leaving the position where it was.
    fn truncate(&mut self, size: usize) {
        if size < self.char_len {
            let end = self.byte_offset(size);
            self.buf.truncate(end);
            self.char_len = size;
            self.byte_pos = self.byte_pos.min(end);
        }
    }
}

impl HeapItem for StringIo {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // The text is plain Rust data.
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, StringIo> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::StringIo.into()
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

    /// `iter(s) is s`; `IOBase.__iter__` refuses a closed stream.
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        if self.get(vm.heap).closed {
            Err(ExcType::value_error_closed_file())
        } else {
            Ok(CallResult::Value(self.clone_value(vm.heap)))
        }
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let stream = self.get_mut(vm.heap);
        if stream.closed {
            return Err(ExcType::value_error_closed_string_io());
        }
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
        self.py_iter(vm)
    }

    /// Closes the stream; returning `None` lets any exception propagate.
    fn py_exit(&mut self, vm: &mut VM<'h>, _exc: Option<HeapId>) -> RunResult<CallResult> {
        close(self.get_mut(vm.heap));
        Ok(CallResult::Value(Value::None))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let stream = self.get(vm.heap);
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::Closed) => Value::Bool(stream.closed),
            Some(StaticStrings::Encoding | StaticStrings::Errors) => Value::None,
            Some(StaticStrings::LineBuffering) => {
                ensure_open(stream)?;
                Value::Bool(false)
            }
            Some(StaticStrings::Newlines) => {
                ensure_open(stream)?;
                if stream.newline.is_universal() {
                    stream.seen.to_value(vm.heap)
                } else {
                    Value::None
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::Read) => self.read(args, vm)?,
            Some(StaticStrings::Readline) => self.readline(args, vm)?,
            Some(StaticStrings::Readlines) => self.readlines(args, vm)?,
            Some(StaticStrings::Write) => self.write(args, vm)?,
            Some(StaticStrings::Writelines) => self.writelines(args, vm)?,
            Some(StaticStrings::Getvalue) => {
                args.check_zero_args("StringIO.getvalue", vm.heap)?;
                let stream = self.get(vm.heap);
                ensure_open(stream)?;
                self.copy_out(0..stream.buf.len(), vm)?
            }
            Some(StaticStrings::Tell) => {
                args.check_zero_args("StringIO.tell", vm.heap)?;
                let stream = self.get(vm.heap);
                ensure_open(stream)?;
                position_value(stream.pos)
            }
            Some(StaticStrings::Seek) => self.seek(args, vm)?,
            Some(StaticStrings::Truncate) => self.truncate(args, vm)?,
            Some(StaticStrings::Close) => {
                args.check_zero_args("StringIO.close", vm.heap)?;
                close(self.get_mut(vm.heap));
                Value::None
            }
            Some(StaticStrings::Flush) => {
                // `IOBase.flush` on a `StringIO` does not check for closure.
                args.check_zero_args("_IOBase.flush", vm.heap)?;
                Value::None
            }
            Some(method @ (StaticStrings::Readable | StaticStrings::Writable | StaticStrings::Seekable)) => {
                let name = match method {
                    StaticStrings::Readable => "StringIO.readable",
                    StaticStrings::Writable => "StringIO.writable",
                    _ => "StringIO.seekable",
                };
                args.check_zero_args(name, vm.heap)?;
                ensure_open(self.get(vm.heap))?;
                Value::Bool(true)
            }
            Some(StaticStrings::Isatty) => {
                args.check_zero_args("_IOBase.isatty", vm.heap)?;
                if self.get(vm.heap).closed {
                    return Err(ExcType::value_error_closed_file());
                }
                Value::Bool(false)
            }
            Some(method @ (StaticStrings::Fileno | StaticStrings::Detach)) => {
                let name = if method == StaticStrings::Fileno {
                    "fileno"
                } else {
                    "detach"
                };
                if matches!(args, ArgValues::Empty) {
                    return Err(ExcType::unsupported_operation(name));
                }
                args.drop_with(vm);
                return Err(ExcType::type_error_takes_no_arguments(name));
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::StringIo, attr.as_str(vm.interns)));
            }
        };
        Ok(CallResult::Value(value))
    }
}

impl<'h> HeapObjectRead<'h, StringIo> {
    /// A new `str` of the buffer's byte `range`, preflighted since the stream can be most of the budget.
    fn copy_out(&self, range: Range<usize>, vm: &VM<'h>) -> RunResult<Value> {
        vm.heap.tracker.check_allocation(range.len())?;
        Ok(allocate_string(self.get(vm.heap).buf[range].to_owned(), vm.heap))
    }

    /// `read(size=-1, /)`: a negative or `None` size reads to the end.
    fn read(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = args.reject_kwargs("StringIO.read", vm.heap)?;
        let size = optional_size(size.get_zero_one_arg("read", vm.heap)?, vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let text = stream.read(non_negative(size));
        self.copy_out(text, vm)
    }

    /// `readline(size=-1, /)`.
    fn readline(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = args.reject_kwargs("StringIO.readline", vm.heap)?;
        let size = optional_size(size.get_zero_one_arg("readline", vm.heap)?, vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let line = stream.readline(non_negative(size));
        self.copy_out(line, vm)
    }

    /// `readlines(hint=-1, /)`, `IOBase`'s: with a positive hint it stops
    /// after the line that takes the total past `hint`.
    fn readlines(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let hint = args.reject_kwargs("_IOBase.readlines", vm.heap)?;
        let hint = optional_size(hint.get_zero_one_arg("readlines", vm.heap)?, vm)?.unwrap_or(-1);
        if self.get(vm.heap).closed {
            return Err(ExcType::value_error_closed_file());
        }
        // Guarded so a limit error mid-loop releases the lines built so far.
        let mut lines_guard = DropGuard::new(Vec::new(), vm);
        let (lines, vm) = lines_guard.as_parts_mut();
        let mut total: i64 = 0;
        loop {
            vm.heap.tracker.check_memory_time_every(lines.len())?;
            let line = self.get_mut(vm.heap).readline(None);
            if line.is_empty() {
                break;
            }
            let length = i64::try_from(self.get(vm.heap).buf[line.clone()].chars().count()).unwrap_or(i64::MAX);
            // Short lines cost no heap entry, so the poll alone misses the list's own doubling.
            vm.heap
                .tracker
                .check_growth(lines.len(), lines.capacity(), VALUE_SIZE)?;
            lines.push(self.copy_out(line, vm)?);
            if hint > 0 {
                if length > hint - total {
                    break;
                }
                total += length;
            }
        }
        let (lines, vm) = lines_guard.into_parts();
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(lines)))))
    }

    /// `write(s, /)`: returns the length of `s`, before any newline translation.
    fn write(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let text = args
            .reject_kwargs("StringIO.write", vm.heap)?
            .get_one_arg("StringIO.write", vm.heap)?;
        defer_drop!(text, vm);
        self.write_value(text, vm)
    }

    /// Writes one `str`, checking its type before whether the stream is open.
    fn write_value(&self, text: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let Some(text) = PyStr::from_value(text, vm) else {
            return Err(ExcType::type_error_string_argument_expected(&text.py_type_name(vm)));
        };
        let text = text.as_str(vm);
        let stream = self.get(vm.heap);
        ensure_open(stream)?;
        vm.heap.tracker.check_allocation(stream.translation_cost(text))?;
        let text = text.to_owned();
        let written = position_value(text.chars().count());
        let text = self.get_mut(vm.heap).translate(text);
        let growth = self.get(vm.heap).write_growth(&text)?;
        vm.heap.tracker.check_pending_allocation(growth)?;
        self.get_mut(vm.heap).write(&text);
        Ok(written)
    }

    /// `writelines(lines, /)`, `IOBase`'s: `write()` for each item.
    fn writelines(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let lines = args
            .reject_kwargs("_IOBase.writelines", vm.heap)?
            .get_one_arg("_IOBase.writelines", vm.heap)?;
        if self.get(vm.heap).closed {
            lines.drop_with(vm);
            return Err(ExcType::value_error_closed_file());
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

    /// `seek(pos, whence=0, /)`: text streams only seek to an absolute
    /// position, the current one or the end.
    fn seek(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let (pos, whence) = seek_args(args, "StringIO.seek", vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let target = match whence {
            0 if pos < 0 => return Err(ExcType::value_error(format!("Negative seek position {pos}"))),
            0 => stream_position(pos)?,
            1 | 2 if pos != 0 => return Err(ExcType::os_error_nonzero_relative_seek()),
            1 => stream.pos,
            2 => stream.char_len,
            _ => return Err(ExcType::value_error_invalid_whence(whence, true)),
        };
        stream.seek_to(target);
        Ok(position_value(target))
    }

    /// `truncate(size=None, /)`: `None` truncates at the position.
    fn truncate(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let size = args.reject_kwargs("StringIO.truncate", vm.heap)?;
        let size = optional_size(size.get_zero_one_arg("truncate", vm.heap)?, vm)?;
        let stream = self.get_mut(vm.heap);
        ensure_open(stream)?;
        let size = match size {
            None => stream.pos,
            Some(size) if size < 0 => return Err(ExcType::value_error(format!("Negative size value {size}"))),
            Some(size) => stream_position(size)?,
        };
        stream.truncate(size);
        Ok(position_value(size))
    }
}

/// `StringIO()`'s `newline`: `'\n'` when omitted, `None` for universal newlines.
fn parse_newline(newline: Option<Value>, vm: &mut VM<'_>) -> RunResult<Newline> {
    let Some(newline) = newline else {
        return Ok(Newline::Lf);
    };
    defer_drop!(newline, vm);
    if matches!(newline, Value::None) {
        return Ok(Newline::Universal);
    }
    let Some(text) = PyStr::from_value(newline, vm) else {
        return Err(ExcType::type_error_str_or_none("newline", &newline.py_type_name(vm)));
    };
    let text = text.as_str(vm);
    Newline::parse(text).ok_or_else(|| ExcType::value_error_illegal_newline(text))
}

/// The `ValueError` `StringIO`'s own methods raise once it is closed.
fn ensure_open(stream: &StringIo) -> RunResult<()> {
    if stream.closed {
        Err(ExcType::value_error_closed_string_io())
    } else {
        Ok(())
    }
}

/// Marks the stream closed and releases its text, as CPython frees the buffer.
fn close(stream: &mut StringIo) {
    stream.closed = true;
    stream.buf = String::new();
    stream.char_len = 0;
    stream.byte_pos = 0;
}
