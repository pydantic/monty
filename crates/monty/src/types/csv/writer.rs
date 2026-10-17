//! `_csv.writer`: formats records and hands each line to the target's `write()`.
//!
//! Formatting is a port of CPython's `join_append_data`, so quoting and escaping decisions
//! match. A file target yields the host write (one `OsCall` per `writerow`, and one combined
//! write per `writerows`); any other target's `write`, bound at construction, runs as a
//! Python call.

use std::{fmt::Write, mem, sync::Arc};

use monty_types::ResourceTracker;
use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    fstring::FormatFloat,
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapRead},
    intern::StaticStrings,
    os_dispatch::{PendingEffect, PostConversionEffect},
    string_builder::StringBuilder,
    types::{
        CsvObject, PyTrait, Type,
        csv::dialect::{Dialect, Quoting},
        iter::collect_owned_iterable,
        str::allocate_string,
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// A `_csv.writer`: the target, its `write` callable and the dialect.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Writer {
    /// The `csvfile` argument. Owned.
    target: Value,
    /// `csvfile.write`, bound at construction as CPython does, so rebinding the attribute
    /// later does not redirect writes. `None` for a file target, whose write is a host call
    /// through `target`. Owned.
    write: Value,
    /// Shared with each record written, so a row costs no copy of the dialect's strings.
    pub(crate) dialect: Arc<Dialect>,
}

impl Writer {
    pub(crate) fn new(target: Value, write: Value, dialect: Dialect) -> Self {
        Self {
            target,
            write,
            dialect: Arc::new(dialect),
        }
    }

    /// Whether the target is an `open()` file, whose `write()` yields a host call.
    pub(crate) fn targets_file(&self, vm: &VM<'_>) -> bool {
        is_file(&self.target, vm)
    }

    /// An owned clone of the bound `write` (`None` for a file target).
    pub(crate) fn write_method(&self, heap: &impl ContainsHeap) -> Value {
        self.write.clone_with_heap(heap)
    }

    /// Invokes `on_child` for each heap id this writer owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [&self.target, &self.write] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Releases the owned references (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.target.py_dec_ref_ids(stack);
        self.write.py_dec_ref_ids(stack);
    }
}

/// `target.write`, looked up once at construction and required to be callable as CPython
/// does: `Some(Value::None)` for a file target (its write is a host call), `None` when
/// `target` cannot back a writer. The caller owns the returned callable.
pub(crate) fn write_method(target: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    if is_file(target, vm) {
        return Ok(Some(Value::None));
    }
    let name = EitherStr::Interned(vm.interns.intern_static(StaticStrings::Write));
    match target.py_getattr(&name, vm) {
        Ok(CallResult::Value(method)) if method.is_callable(vm) => Ok(Some(method)),
        Ok(result) => {
            result.drop_with(vm);
            Ok(None)
        }
        Err(err) if err.is_catchable(ExcType::AttributeError) => Ok(None),
        Err(err) => Err(err),
    }
}

/// `writer.writerow(row)`: formats `row` and returns whatever the target's `write()` returns.
pub(super) fn writerow<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let row = args.get_one_arg("writer.writerow", vm.heap)?;
    defer_drop!(row, vm);
    let items = row_items(row, vm)?;
    defer_drop!(items, vm);
    write_fields(obj, items, vm)
}

/// Formats `items` as one record and writes it, returning whatever `write()` returns: the
/// tail of `writerow`, and of the `DictWriter` methods once they have flattened their dict.
pub(super) fn write_fields<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    items: &[Value],
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let (target, write, dialect) = parts(obj, vm);
    defer_drop!(target, vm);
    defer_drop!(write, vm);
    let terminator_chars = terminator_chars(&dialect.lineterminator);
    let line = format_items(&dialect, &terminator_chars, items, &mut String::new(), vm)?;
    write_line(target, write, line, vm)
}

/// `writer.writerows(rows)`: writes every row and returns `None`.
///
/// A file target receives all the lines in one host write, since a Rust loop cannot suspend
/// once per row; any other target's `write()` is called synchronously as each row arrives.
pub(super) fn writerows<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let rows = args.get_one_arg("writer.writerows", vm.heap)?;
    defer_drop!(rows, vm);
    let (target, write, dialect) = parts(obj, vm);
    defer_drop!(target, vm);
    defer_drop!(write, vm);
    let terminator_chars = terminator_chars(&dialect.lineterminator);
    if is_file(target, vm) {
        let rows: Vec<Value> = collect_owned_iterable(rows.clone_with_heap(vm.heap), vm)?;
        defer_drop!(rows, vm);
        let mut lines = String::new();
        for row in rows {
            let items = row_items(row, vm)?;
            defer_drop!(items, vm);
            format_row(&dialect, &terminator_chars, items, &mut lines, vm)?;
        }
        if rows.is_empty() {
            Ok(CallResult::Value(Value::None))
        } else {
            let lines = StringBuilder::from_existing(lines, &vm.heap.tracker).finish(vm.heap)?;
            write_line(target, write, lines, vm).map(discard_write_result)
        }
    } else {
        let iterator = rows.py_iter(vm)?;
        defer_drop!(iterator, vm);
        // One line buffer serves every row, so each costs a single exact-size allocation.
        let mut line = String::new();
        // One view across the loop: its step count is what polls the time limit.
        let mut rows = iterator.read(vm);
        while let Some(row) = rows.py_next(vm)? {
            defer_drop!(row, vm);
            let items = row_items(row, vm)?;
            defer_drop!(items, vm);
            writerow_now(write, &dialect, &terminator_chars, items, &mut line, vm)?;
        }
        Ok(CallResult::Value(Value::None))
    }
}

/// Formats `items` through `line` and runs the bound `write` to completion; the per-row step
/// of `writerows` and `DictWriter.writerows` on a target that is not a file.
pub(super) fn writerow_now(
    write: &Value,
    dialect: &Dialect,
    terminator_chars: &[char],
    items: &[Value],
    line: &mut String,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let line = format_items(dialect, terminator_chars, items, line, vm)?;
    vm.evaluate_function("writer.writerows", write, ArgValues::One(line))?
        .drop_with(vm);
    Ok(())
}

/// The distinct characters of a line terminator, sorted for `binary_search`: membership
/// per field character must not scan the terminator itself, which can be any length.
/// Built once per call, since the sort is linear-logarithmic in the terminator.
pub(super) fn terminator_chars(lineterminator: &str) -> Vec<char> {
    let mut chars: Vec<char> = lineterminator.chars().collect();
    chars.sort_unstable();
    chars.dedup();
    chars
}

/// One record as a `str`, formatted through `line`, which is cleared first and keeps its
/// capacity for the caller's next record.
fn format_items(
    dialect: &Dialect,
    terminator_chars: &[char],
    items: &[Value],
    line: &mut String,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    line.clear();
    format_row(dialect, terminator_chars, items, line, vm)?;
    // From the slice: one exact-size allocation, bounded by the tracker-grown buffer.
    Ok(allocate_string(line.as_str(), vm.heap))
}

/// Appends one record to `out`: `join_append` per field plus the line terminator.
///
/// `out` grows through a tracker-checked builder at each step, since a record copies its
/// fields' `str()` forms (twice for a quote-doubled field) before the next checkpoint. The
/// builder cannot outlive a field's `str()` call, which re-enters the VM, so it wraps `out`
/// per step rather than owning it.
pub(crate) fn format_row(
    dialect: &Dialect,
    terminator_chars: &[char],
    items: &[Value],
    out: &mut String,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let mut record_len = 0usize;
    let mut field_buffer = String::new();
    let mut number = String::new();
    for (index, item) in items.iter().enumerate() {
        // A `None` field skips the polled character loop, so poll per field as well.
        vm.heap.tracker.check_time_every(index)?;
        let mut quoted = match dialect.quoting {
            Quoting::Nonnumeric => !is_number(item, vm),
            Quoting::All => true,
            Quoting::Strings => item.is_str(vm.heap),
            Quoting::Notnull => !matches!(item, Value::None),
            Quoting::Minimal | Quoting::None => false,
        };
        field_buffer.clear();
        let tracker = &vm.heap.tracker;
        match item {
            Value::None => {}
            // `str()` of an int or float is written to a stack or scratch buffer: the
            // common numeric fields cost no heap string.
            Value::Int(v) => {
                let mut digits = itoa::Buffer::new();
                let text = digits.format(*v);
                append_field_data(dialect, terminator_chars, text, &mut quoted, &mut field_buffer, tracker)?;
            }
            Value::Float(v) => {
                number.clear();
                write!(number, "{}", FormatFloat(*v))?;
                append_field_data(
                    dialect,
                    terminator_chars,
                    &number,
                    &mut quoted,
                    &mut field_buffer,
                    tracker,
                )?;
            }
            _ => {
                let text = item.py_str(vm)?;
                defer_drop!(text, vm);
                append_field_data(
                    dialect,
                    terminator_chars,
                    text.to_str(vm)?,
                    &mut quoted,
                    &mut field_buffer,
                    &vm.heap.tracker,
                )?;
            }
        }
        record_len += append_field(dialect, &field_buffer, quoted, index > 0, out, &vm.heap.tracker)?;
    }
    // A lone empty field would write as an empty line, which reads back as no record at all.
    if items.len() == 1 && record_len == 0 {
        if matches!(dialect.quoting, Quoting::None | Quoting::Strings | Quoting::Notnull) {
            return Err(ExcType::csv_error("single empty field record must be quoted"));
        }
        append_field(dialect, "", true, false, out, &vm.heap.tracker)?;
    }
    let mut builder = StringBuilder::from_existing(mem::take(out), &vm.heap.tracker);
    builder.push_str(&dialect.lineterminator)?;
    *out = builder.finish_raw()?;
    Ok(())
}

/// Longest stretch of a field scanned between clock polls: a field with no special
/// character is otherwise one run, however long, and never reaches the poll.
const FIELD_SCAN_CHUNK: usize = 64 * 1024;

/// Escapes or quote-doubles the characters of one field into `buffer`, as
/// `join_append_data` does, upgrading `quoted` when a special character requires it.
/// `buffer` is at most twice `text`, which is already tracked; the loop polls the clock
/// at least once per [`FIELD_SCAN_CHUNK`] of input.
fn append_field_data(
    dialect: &Dialect,
    terminator_chars: &[char],
    text: &str,
    quoted: &mut bool,
    buffer: &mut String,
    tracker: &ResourceTracker,
) -> RunResult<()> {
    let is_special = |c: char| {
        c == dialect.delimiter
            || Some(c) == dialect.escapechar
            || Some(c) == dialect.quotechar
            || c == '\n'
            || c == '\r'
            || terminator_chars.binary_search(&c).is_ok()
    };
    // One reservation for the common no-escape case, rather than doubling per chunk.
    buffer.reserve(text.len());
    let mut rest = text;
    let mut polls = 0usize;
    while !rest.is_empty() {
        tracker.check_time_every(polls)?;
        polls += 1;
        // Everything up to the next special character is copied as one run, scanning at
        // most a chunk at a time so a long ordinary field still reaches the poll above.
        let window = &rest[..rest.floor_char_boundary(FIELD_SCAN_CHUNK)];
        let Some(run) = window.find(is_special) else {
            buffer.push_str(window);
            rest = &rest[window.len()..];
            continue;
        };
        buffer.push_str(&rest[..run]);
        rest = &rest[run..];
        let Some(c) = rest.chars().next() else {
            break;
        };
        rest = &rest[c.len_utf8()..];
        let mut want_escape = dialect.quoting == Quoting::None;
        if !want_escape {
            if Some(c) == dialect.quotechar {
                if dialect.doublequote {
                    buffer.push(c);
                } else {
                    want_escape = true;
                }
            } else if Some(c) == dialect.escapechar {
                want_escape = true;
            }
            if !want_escape {
                *quoted = true;
            }
        }
        if want_escape {
            let Some(escapechar) = dialect.escapechar else {
                return Err(ExcType::csv_error("need to escape, but no escapechar set"));
            };
            buffer.push(escapechar);
        }
        buffer.push(c);
    }
    Ok(())
}

/// Appends a prepared field to `out`, after a delimiter when asked and quoted when asked,
/// returning the bytes written (the delimiter included): the caller only tests it for zero.
fn append_field(
    dialect: &Dialect,
    text: &str,
    quoted: bool,
    delimited: bool,
    out: &mut String,
    tracker: &ResourceTracker,
) -> RunResult<usize> {
    let mut builder = StringBuilder::from_existing(mem::take(out), tracker);
    let mut written = text.len();
    if delimited {
        builder.push(dialect.delimiter)?;
        written += 1;
    }
    if quoted {
        let quotechar = dialect.quotechar.expect("quoting requires a quotechar");
        builder.push(quotechar)?;
        builder.push_str(text)?;
        builder.push(quotechar)?;
        written += 2;
    } else {
        builder.push_str(text)?;
    }
    *out = builder.finish_raw()?;
    Ok(written)
}

/// `PyNumber_Check`: the numeric types a `QUOTE_NONNUMERIC` writer leaves unquoted.
fn is_number(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value.py_type(vm), Type::Int | Type::Float | Type::Bool | Type::Complex)
}

/// Whether `value` is an `open()` file, whose `write()` yields a host call.
fn is_file(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::OpenFile(_)))
}

/// The fields of one row, as `writerow`'s `iter(row)` sees them: a non-iterable row is
/// `_csv.Error`, unlike `writerows`' plain `TypeError`. A `list` or `tuple`, the usual row,
/// is copied without an iterator.
fn row_items(row: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let direct = match row {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => Some(list.as_slice()),
            HeapData::Tuple(tuple) => Some(tuple.as_slice()),
            _ => None,
        },
        _ => None,
    };
    if let Some(items) = direct {
        vm.heap.tracker.check_allocation(items.len() * VALUE_SIZE)?;
        Ok(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect())
    } else {
        let iterator = match row.py_iter(vm) {
            Ok(iterator) => iterator,
            Err(err) if err.is_catchable(ExcType::TypeError) => {
                return Err(ExcType::csv_error(format!(
                    "iterable expected, not {}",
                    row.py_type_name(vm)
                )));
            }
            Err(err) => return Err(err),
        };
        collect_owned_iterable(iterator, vm)
    }
}

/// `write(line)`: the host call of a file target, otherwise the bound `write`.
fn write_line(target: &Value, write: &Value, line: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if is_file(target, vm) {
        let name = vm.interns.intern_static(StaticStrings::Write);
        vm.call_attr(target.clone_with_heap(vm.heap), name, ArgValues::One(line))
    } else {
        vm.call_function(write, ArgValues::One(line))
    }
}

/// Marks a file write's pending result as `None`, which `writerows` returns.
fn discard_write_result(mut result: CallResult) -> CallResult {
    if let CallResult::OsCallWithEffect {
        effect: PendingEffect::Post(PostConversionEffect::WritePosition { result_none, .. }),
        ..
    } = &mut result
    {
        *result_none = true;
    }
    result
}

/// The owned target, its `write` and the dialect of a writer object, read under a short borrow.
fn parts<'h>(obj: &HeapRead<'h, CsvObject>, vm: &VM<'h>) -> (Value, Value, Arc<Dialect>) {
    let CsvObject::Writer(writer) = obj.get(vm.heap) else {
        unreachable!("dispatched on CsvObject::Writer")
    };
    (
        writer.target.clone_with_heap(vm.heap),
        writer.write.clone_with_heap(vm.heap),
        Arc::clone(&writer.dialect),
    )
}
