//! `_csv.writer`: formats records and hands each line to the target's `write()`.
//!
//! Formatting is a port of CPython's `join_append_data`, so quoting and escaping decisions
//! match. A file target yields the host write (one `OsCall` per `writerow`, and one combined
//! write per `writerows`); any other target's `write` runs as a Python call.

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapRead},
    intern::StaticStrings,
    os_dispatch::{PendingEffect, PostConversionEffect},
    types::{
        CsvObject, PyTrait, Type,
        csv::dialect::{Dialect, Quoting},
        iter::collect_owned_iterable,
        str::allocate_string,
    },
    value::{EitherStr, Value},
};

/// A `_csv.writer`: the object whose `write()` receives each formatted line, and the dialect.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Writer {
    /// The `csvfile` argument. Owned.
    target: Value,
    pub(crate) dialect: Dialect,
}

impl Writer {
    pub(crate) fn new(target: Value, dialect: Dialect) -> Self {
        Self { target, dialect }
    }

    /// Invokes `on_child` for each heap id this writer owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.target {
            on_child(*id);
        }
    }

    /// Releases the target reference (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.target.py_dec_ref_ids(stack);
    }
}

/// Whether `target` can back a writer: CPython looks `write` up once at construction.
pub(crate) fn has_write_method(target: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    if is_file(target, vm) {
        return Ok(true);
    }
    let name = EitherStr::Interned(vm.interns.intern_static(StaticStrings::Write));
    match target.py_getattr(&name, vm) {
        Ok(result) => {
            result.drop_with(vm);
            Ok(true)
        }
        Err(err) if err.exc_type() == Some(ExcType::AttributeError) => Ok(false),
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
    let (target, dialect) = parts(obj, vm);
    defer_drop!(target, vm);
    let items = row_items(row, vm)?;
    defer_drop!(items, vm);
    let line = format_row(&dialect, items, vm)?;
    write_line(target, line, vm)
}

/// `writer.writerows(rows)`: writes every row and returns `None`.
///
/// A file target receives all the lines in one host write, since a Rust loop cannot suspend
/// once per row; any other target's `write()` is called once per row, synchronously.
pub(super) fn writerows<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let rows = args.get_one_arg("writer.writerows", vm.heap)?;
    defer_drop!(rows, vm);
    let (target, dialect) = parts(obj, vm);
    defer_drop!(target, vm);
    let rows: Vec<Value> = collect_owned_iterable(rows.clone_with_heap(vm.heap), vm)?;
    defer_drop!(rows, vm);
    if is_file(target, vm) {
        // Bounded by the rows' own strings, which the tracker already charges.
        let mut lines = String::new();
        for row in rows {
            let items = row_items(row, vm)?;
            defer_drop!(items, vm);
            lines.push_str(&format_row(&dialect, items, vm)?);
        }
        if rows.is_empty() {
            Ok(CallResult::Value(Value::None))
        } else {
            write_line(target, lines, vm).map(discard_write_result)
        }
    } else {
        let write = EitherStr::Interned(vm.interns.intern_static(StaticStrings::Write));
        for row in rows {
            let items = row_items(row, vm)?;
            defer_drop!(items, vm);
            let line = allocate_string(format_row(&dialect, items, vm)?, vm.heap);
            let method = match target.py_getattr(&write, vm) {
                Ok(CallResult::Value(method)) => method,
                Ok(other) => {
                    line.drop_with(vm);
                    other.drop_with(vm);
                    return Err(RunError::internal("writer.writerows: write attribute is not a value"));
                }
                Err(err) => {
                    line.drop_with(vm);
                    return Err(err);
                }
            };
            defer_drop!(method, vm);
            vm.evaluate_function("writer.writerows", method, ArgValues::One(line))?
                .drop_with(vm);
        }
        Ok(CallResult::Value(Value::None))
    }
}

/// Formats one record: `join_append` per field plus the line terminator.
///
/// The result is bounded by twice the fields' `str()` forms plus a few characters per field,
/// all already charged to the tracker, so a plain `String` is fine.
pub(crate) fn format_row(dialect: &Dialect, items: &[Value], vm: &mut VM<'_>) -> RunResult<String> {
    let mut out = String::new();
    let mut record_len = 0usize;
    let mut field_buffer = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(dialect.delimiter);
            record_len += 1;
        }
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
            append_field_data(dialect, text.to_str(vm)?, &mut quoted, &mut field_buffer)?;
        }
        record_len += append_field(dialect, &field_buffer, quoted, &mut out);
    }
    // A lone empty field would write as an empty line, which reads back as no record at all.
    if items.len() == 1 && record_len == 0 {
        if matches!(dialect.quoting, Quoting::None | Quoting::Strings | Quoting::Notnull) {
            return Err(ExcType::csv_error("single empty field record must be quoted"));
        }
        append_field(dialect, "", true, &mut out);
    }
    out.push_str(&dialect.lineterminator);
    Ok(out)
}

/// Escapes or quote-doubles the characters of one field into `buffer`, as
/// `join_append_data` does, upgrading `quoted` when a special character requires it.
fn append_field_data(dialect: &Dialect, text: &str, quoted: &mut bool, buffer: &mut String) -> RunResult<()> {
    for c in text.chars() {
        let special = c == dialect.delimiter
            || Some(c) == dialect.escapechar
            || Some(c) == dialect.quotechar
            || c == '\n'
            || c == '\r'
            || dialect.lineterminator.contains(c);
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

/// Appends a prepared field, quoted when asked, returning the characters written.
fn append_field(dialect: &Dialect, text: &str, quoted: bool, out: &mut String) -> usize {
    let mut written = text.chars().count();
    if quoted {
        let quotechar = dialect.quotechar.expect("quoting requires a quotechar");
        out.push(quotechar);
        out.push_str(text);
        out.push(quotechar);
        written += 2;
    } else {
        out.push_str(text);
    }
    written
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
        Err(err) if err.exc_type() == Some(ExcType::TypeError) => {
            return Err(ExcType::csv_error(format!(
                "iterable expected, not {}",
                row.py_type_name(vm)
            )));
        }
        Err(err) => return Err(err),
    };
    collect_owned_iterable(iterator, vm)
}

/// Calls `target.write(line)`. A file target yields its host write.
fn write_line(target: &Value, line: String, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let line = allocate_string(line, vm.heap);
    let write = vm.interns.intern_static(StaticStrings::Write);
    vm.call_attr(target.clone_with_heap(vm.heap), write, ArgValues::One(line))
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

/// The owned target and dialect of a writer object, read under a short borrow.
fn parts<'h>(obj: &HeapRead<'h, CsvObject>, vm: &VM<'h>) -> (Value, Dialect) {
    let CsvObject::Writer(writer) = obj.get(vm.heap) else {
        unreachable!("dispatched on CsvObject::Writer")
    };
    (writer.target.clone_with_heap(vm.heap), writer.dialect.clone())
}
