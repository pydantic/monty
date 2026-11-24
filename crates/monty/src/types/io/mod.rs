//! The in-memory streams of the `io` module, and the newline handling they
//! share with `open()` text files.
//!
//! [`StringIo`] and [`BytesIo`] are leaf heap objects owning their buffers as
//! plain Rust data, so they hold no heap references and dump with the session.
//! Their methods follow CPython's C implementations (`Modules/_io/stringio.c`
//! and `bytesio.c`), including where the two word the same error differently.

pub(crate) mod bytes_io;
pub(crate) mod newline;
pub(crate) mod string_io;

pub(crate) use bytes_io::BytesIo;
pub(crate) use newline::{Newline, translate_newlines};
pub(crate) use string_io::StringIo;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{Heap, HeapData},
    types::PyTrait,
    value::Value,
};

/// Whether `value` is an `open()` file or an in-memory stream.
///
/// Monty cannot hand out these objects' methods as values, so code that
/// CPython writes as `fp.write(...)` through a looked-up attribute (`json.dump`,
/// `csv.writer`, `print(file=...)`) must call them by name with `call_attr`.
pub(crate) fn is_native_stream(value: &Value, heap: &Heap) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::OpenFile(_) | HeapData::StringIo(_) | HeapData::BytesIo(_)))
}

/// Whether `value` is a `StringIO` or `BytesIO`, whose methods never suspend.
pub(crate) fn is_memory_stream(value: &Value, heap: &Heap) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::StringIo(_) | HeapData::BytesIo(_)))
}

/// An optional `size`/`hint` argument, as CPython's
/// `_Py_convert_optional_to_ssize_t` reads it: `None` or absent, else an
/// `__index__` value that must fit `ssize_t`.
pub(crate) fn optional_size(arg: Option<Value>, vm: &mut VM<'_>) -> RunResult<Option<i64>> {
    let Some(arg) = arg else {
        return Ok(None);
    };
    defer_drop!(arg, vm);
    match arg {
        Value::None => Ok(None),
        Value::Int(n) => Ok(Some(*n)),
        _ => match arg.py_index_impl(vm)? {
            Some(index) => {
                defer_drop!(index, vm);
                index.as_int_with_overflow(vm, overflow_index_sized).map(Some)
            }
            None => Err(ExcType::type_error_integer_or_none(&arg.py_type_name(vm))),
        },
    }
}

/// The `OverflowError` CPython's `PyNumber_AsSsize_t(obj, OverflowError)` raises.
fn overflow_index_sized() -> RunError {
    ExcType::overflow_index_sized_int().into()
}

/// `seek(pos, whence=0, /)`'s arguments: `pos` as a C `ssize_t`, `whence` as a C `int`.
///
/// Keywords are refused with `{kw_name}() takes no keyword arguments`.
pub(crate) fn seek_args(args: ArgValues, kw_name: &str, vm: &mut VM<'_>) -> RunResult<(i64, i64)> {
    let (pos, whence) = args
        .reject_kwargs(kw_name, vm.heap)?
        .get_one_two_args("seek", vm.heap)?;
    defer_drop!(pos, vm);
    defer_drop!(whence, vm);
    let pos = pos.as_int(vm)?;
    let whence = match whence {
        Some(whence) => {
            let whence = whence.as_int_with_overflow(vm, ExcType::overflow_c_int)?;
            i32::try_from(whence).map_err(|_| ExcType::overflow_c_int())?;
            whence
        }
        None => 0,
    };
    Ok((pos, whence))
}

/// A non-negative position or size as a stream offset: CPython's `OverflowError`
/// past `ssize_t`, which on 32-bit targets is below `i64::MAX`.
pub(crate) fn stream_position(pos: i64) -> RunResult<usize> {
    isize::try_from(pos)
        .ok()
        .and_then(|pos| usize::try_from(pos).ok())
        .ok_or_else(ExcType::overflow_c_ssize_t)
}

/// A size argument as a limit: negative means unlimited, as `-1` does in CPython.
pub(crate) fn non_negative(size: Option<i64>) -> Option<usize> {
    size.and_then(|size| usize::try_from(size).ok())
}

/// The bytes a stream buffer of `capacity` allocates to hold `needed` bytes,
/// or zero if it already fits.
///
/// Mirrors `Vec::reserve`'s amortized doubling. The writes reserve the whole
/// result in one step, so preflighting this covers the seek padding and the
/// data together rather than missing a second doubling between them.
pub(crate) fn buffer_growth(needed: usize, capacity: usize) -> usize {
    if needed <= capacity {
        0
    } else {
        needed.max(capacity.saturating_mul(2)) - capacity
    }
}

/// A position or length as a Python `int`.
pub(crate) fn position_value(pos: usize) -> Value {
    Value::Int(i64::try_from(pos).unwrap_or(i64::MAX))
}
