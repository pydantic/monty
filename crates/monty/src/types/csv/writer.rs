//! `_csv.writer`: formats records and hands each line to the target's `write()`.
//!
//! Formatting is a port of CPython's `join_append_data`, so quoting and escaping decisions
//! match. A file target yields the host write (one `OsCall` per `writerow`, and one combined
//! write per `writerows`); any other target's `write`, bound at construction, runs as a
//! Python call.

use std::mem;

use monty_types::ResourceTracker;
use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapRead},
    intern::StaticStrings,
    os_dispatch::{PendingEffect, PostConversionEffect},
    string_builder::StringBuilder,
    types::{
        CsvObject, PyTrait, Type,
        csv::dialect::{Dialect, Quoting},
        iter::collect_owned_iterable,
    },
    value::{EitherStr, Value},
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
    pub(crate) dialect: Dialect,
}

impl Writer {
    pub(crate) fn new(target: Value, write: Value, dialect: Dialect) -> Self {
        Self { target, write, dialect }
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
        Ok(CallResult::Value(method)) if method.is_callable(vm.heap) => Ok(Some(method)),
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
    let (target, write, dialect) = parts(obj, vm);
    defer_drop!(target, vm);
    defer_drop!(write, vm);
    let line = format_line(&dialect, row, vm)?;
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
    if is_file(target, vm) {
        let rows: Vec<Value> = collect_owned_iterable(rows.clone_with_heap(vm.heap), vm)?;
        defer_drop!(rows, vm);
        let mut lines = String::new();
        for row in rows {
            let items = row_items(row, vm)?;
            defer_drop!(items, vm);
            format_row(&dialect, items, &mut lines, vm)?;
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
        while let Some(row) = iterator.read(vm).py_next(vm)? {
            defer_drop!(row, vm);
            writerow_now(write, &dialect, row, vm)?;
        }
        Ok(CallResult::Value(Value::None))
    }
}

/// Formats `row` and runs the bound `write` to completion; the per-row step of `writerows`
/// and `DictWriter.writerows` on a target that is not a file.
pub(super) fn writerow_now(write: &Value, dialect: &Dialect, row: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let line = format_line(dialect, row, vm)?;
    vm.evaluate_function("writer.writerows", write, ArgValues::One(line))?
        .drop_with(vm);
    Ok(())
}

/// One record as a `str`: the fields of `row` through `format_row`.
fn format_line(dialect: &Dialect, row: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let items = row_items(row, vm)?;
    defer_drop!(items, vm);
    let mut line = String::new();
    format_row(dialect, items, &mut line, vm)?;
    StringBuilder::from_existing(line, &vm.heap.tracker).finish(vm.heap)
}

/// Appends one record to `out`: `join_append` per field plus the line terminator.
///
/// `out` grows through a tracker-checked builder at each step, since a record copies its
/// fields' `str()` forms (twice for a quote-doubled field) before the next checkpoint. The
/// builder cannot outlive a field's `str()` call, which re-enters the VM, so it wraps `out`
/// per step rather than owning it.
pub(crate) fn format_row(dialect: &Dialect, items: &[Value], out: &mut String, vm: &mut VM<'_>) -> RunResult<()> {
    let mut record_len = 0usize;
    let mut field_buffer = String::new();
    let terminator_chars = terminator_chars(&dialect.lineterminator);
    for (index, item) in items.iter().enumerate() {
        let mut quoted = match dialect.quoting {
            Quoting::Nonnumeric => !is_number(item, vm),
            Quoting::All => true,
            Quoting::Strings => item.is_str(vm.heap),
            Quoting::Notnull => !matches!(item, Value::None),
            Quoting::Minimal | Quoting::None => false,
        };
        field_buffer.clear();
        if !matches!(item, Value::None) {
            let text = item.py_str(vm)?;
            defer_drop!(text, vm);
            append_field_data(
                dialect,
                &terminator_chars,
                text.to_str(vm)?,
                &mut quoted,
                &mut field_buffer,
                &vm.heap.tracker,
            )?;
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

/// The distinct characters of a line terminator, sorted for `binary_search`: membership
/// per field character must not scan the terminator itself, which can be any length.
fn terminator_chars(lineterminator: &str) -> Vec<char> {
    let mut chars: Vec<char> = lineterminator.chars().collect();
    chars.sort_unstable();
    chars.dedup();
    chars
}

/// Escapes or quote-doubles the characters of one field into `buffer`, as
/// `join_append_data` does, upgrading `quoted` when a special character requires it.
/// `buffer` is at most twice `text`, which is already tracked; the loop polls the clock.
fn append_field_data(
    dialect: &Dialect,
    terminator_chars: &[char],
    text: &str,
    quoted: &mut bool,
    buffer: &mut String,
    tracker: &ResourceTracker,
) -> RunResult<()> {
    for (index, c) in text.chars().enumerate() {
        tracker.check_time_every(index)?;
        let special = c == dialect.delimiter
            || Some(c) == dialect.escapechar
            || Some(c) == dialect.quotechar
            || c == '\n'
            || c == '\r'
            || terminator_chars.binary_search(&c).is_ok();
        if special {
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
        }
        buffer.push(c);
    }
    Ok(())
}

/// Appends a prepared field to `out`, after a delimiter when asked and quoted when asked,
/// returning the characters written (the delimiter included).
fn append_field(
    dialect: &Dialect,
    text: &str,
    quoted: bool,
    delimited: bool,
    out: &mut String,
    tracker: &ResourceTracker,
) -> RunResult<usize> {
    let mut builder = StringBuilder::from_existing(mem::take(out), tracker);
    let mut written = text.chars().count();
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
/// `_csv.Error`, unlike `writerows`' plain `TypeError`.
fn row_items(row: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
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
fn parts<'h>(obj: &HeapRead<'h, CsvObject>, vm: &VM<'h>) -> (Value, Value, Dialect) {
    let CsvObject::Writer(writer) = obj.get(vm.heap) else {
        unreachable!("dispatched on CsvObject::Writer")
    };
    (
        writer.target.clone_with_heap(vm.heap),
        writer.write.clone_with_heap(vm.heap),
        writer.dialect.clone(),
    )
}
