//! `csv.DictReader` and `csv.DictWriter`: the pure-Python wrappers of CPython's `csv.py`,
//! mapping rows to and from dicts keyed by `fieldnames`.

use std::mem;

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, HeapData, HeapId, HeapRead, HeapReadOutput},
    types::{
        CsvObject, Dict, List, PyTrait, Set,
        csv::{
            Dialect,
            writer::{terminator_chars, writerow_now},
        },
        iter::collect_iterable,
    },
    value::{VALUE_SIZE, Value},
};

/// A `csv.DictReader`: a `_csv.reader` plus the header handling around it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DictReader {
    /// The underlying `_csv.reader`. Owned.
    pub(crate) reader: Value,
    /// `None` until the header row is read (or `fieldnames` is assigned). Owned.
    pub(crate) fieldnames: Option<Value>,
    /// Key for the surplus fields of a long row. Owned.
    pub(crate) restkey: Value,
    /// Value for the missing fields of a short row. Owned.
    pub(crate) restval: Value,
    /// The `dialect` argument as passed, which is what `.dialect` reports. Owned.
    pub(crate) dialect: Value,
    pub(crate) line_num: u64,
}

impl DictReader {
    pub(crate) fn new(
        reader: Value,
        fieldnames: Option<Value>,
        restkey: Value,
        restval: Value,
        dialect: Value,
    ) -> Self {
        Self {
            reader,
            fieldnames,
            restkey,
            restval,
            dialect,
            line_num: 0,
        }
    }

    /// Invokes `on_child` for each heap id this reader owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [
            Some(&self.reader),
            self.fieldnames.as_ref(),
            Some(&self.restkey),
            Some(&self.restval),
            Some(&self.dialect),
        ]
        .into_iter()
        .flatten()
        {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Releases every owned reference (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.reader.py_dec_ref_ids(stack);
        if let Some(fieldnames) = &mut self.fieldnames {
            fieldnames.py_dec_ref_ids(stack);
        }
        self.restkey.py_dec_ref_ids(stack);
        self.restval.py_dec_ref_ids(stack);
        self.dialect.py_dec_ref_ids(stack);
    }
}

/// A `csv.DictWriter`: a `_csv.writer` plus the field order a row dict is flattened in.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DictWriter {
    /// The underlying `_csv.writer`. Owned.
    pub(crate) writer: Value,
    /// Any iterable of keys, as passed. Owned.
    pub(crate) fieldnames: Value,
    /// Value written for a key the row dict lacks. Owned.
    pub(crate) restval: Value,
    /// The `extrasaction` argument, lower-cased at construction; a key outside `fieldnames`
    /// raises when it is `'raise'` at the time of the write. Owned.
    pub(crate) extrasaction: Value,
}

impl DictWriter {
    pub(crate) fn new(writer: Value, fieldnames: Value, restval: Value, extrasaction: Value) -> Self {
        Self {
            writer,
            fieldnames,
            restval,
            extrasaction,
        }
    }

    /// Invokes `on_child` for each heap id this writer owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [&self.writer, &self.fieldnames, &self.restval, &self.extrasaction] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Releases every owned reference (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.writer.py_dec_ref_ids(stack);
        self.fieldnames.py_dec_ref_ids(stack);
        self.restval.py_dec_ref_ids(stack);
        self.extrasaction.py_dec_ref_ids(stack);
    }
}

/// `DictReader.fieldnames`: reads the header row on first access, then reports the names
/// (`None` for an empty source). The caller owns the returned reference.
pub(super) fn reader_fieldnames<'h>(obj: &mut HeapRead<'h, CsvObject>, vm: &mut VM<'h>) -> RunResult<Value> {
    let (reader, unset) = {
        let CsvObject::DictReader(dict_reader) = obj.get(vm.heap) else {
            unreachable!("dispatched on CsvObject::DictReader")
        };
        (
            dict_reader.reader.clone_with_heap(vm.heap),
            dict_reader.fieldnames.is_none(),
        )
    };
    defer_drop!(reader, vm);
    if unset
        && let Some(header) = next_row(reader, vm)?
        && let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap)
    {
        dict_reader.fieldnames = Some(header);
    }
    let line_num = reader_line_num(reader, vm);
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.line_num = line_num;
    }
    let CsvObject::DictReader(dict_reader) = obj.get(vm.heap) else {
        unreachable!("dispatched on CsvObject::DictReader")
    };
    Ok(dict_reader
        .fieldnames
        .as_ref()
        .map_or(Value::None, |names| names.clone_with_heap(vm.heap)))
}

/// `DictReader.__next__`: the next non-empty row as a dict, with `restkey` / `restval`
/// covering a length mismatch against `fieldnames`.
pub(super) fn reader_next<'h>(obj: &mut HeapRead<'h, CsvObject>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let first = matches!(obj.get(vm.heap), CsvObject::DictReader(dict_reader) if dict_reader.line_num == 0);
    if first {
        // Used only for its side effect of reading the header, as in CPython.
        reader_fieldnames(obj, vm)?.drop_with(vm);
    }
    let (reader, fieldnames, restkey, restval) = {
        let CsvObject::DictReader(dict_reader) = obj.get(vm.heap) else {
            unreachable!("dispatched on CsvObject::DictReader")
        };
        (
            dict_reader.reader.clone_with_heap(vm.heap),
            dict_reader
                .fieldnames
                .as_ref()
                .map_or(Value::None, |names| names.clone_with_heap(vm.heap)),
            dict_reader.restkey.clone_with_heap(vm.heap),
            dict_reader.restval.clone_with_heap(vm.heap),
        )
    };
    defer_drop!(reader, vm);
    defer_drop!(fieldnames, vm);
    defer_drop!(restkey, vm);
    defer_drop!(restval, vm);

    let Some(row) = next_row(reader, vm)? else {
        return Ok(None);
    };
    let mut row_guard = DropGuard::new(row, vm);
    let (row, vm) = row_guard.as_parts_mut();
    let line_num = reader_line_num(reader, vm);
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.line_num = line_num;
    }
    // Blank lines yield empty rows, which CPython skips here. A native loop over the
    // source, so it polls the clock: an endless run of blank lines must still time out.
    let mut skipped = 0usize;
    while row_len(row, vm) == 0 {
        vm.heap.tracker.check_time_every(skipped)?;
        skipped += 1;
        let Some(next) = next_row(reader, vm)? else {
            return Ok(None);
        };
        mem::replace(row, next).drop_with(vm);
    }
    // CPython reads `self.fieldnames` to build the dict, and the property refreshes `line_num`.
    let line_num = reader_line_num(reader, vm);
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.line_num = line_num;
    }

    let names = collect_iterable(fieldnames, vm)?;
    let mut names_guard = DropGuard::new(names, vm);
    let (names, vm) = names_guard.as_parts_mut();
    let items = row_values(row, vm);
    let mut items_guard = DropGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    let mut dict_guard = DropGuard::new(Dict::new(), vm);
    let (dict, vm) = dict_guard.as_parts_mut();
    let (name_count, item_count) = (names.len(), items.len());
    // `dict(zip(fieldnames, row))`: the surplus of the longer side stays behind.
    let paired = name_count.min(item_count);
    for (name, item) in names.drain(..paired).zip(items.drain(..paired)) {
        if let Some(old) = dict.set(name, item, vm)? {
            old.drop_with(vm);
        }
    }
    if name_count < item_count {
        let extra = List::new(mem::take(items));
        let extra = Value::Ref(vm.heap.allocate(HeapData::List(extra)));
        if let Some(old) = dict.set(restkey.clone_with_heap(vm.heap), extra, vm)? {
            old.drop_with(vm);
        }
    } else {
        for name in names.drain(..) {
            if let Some(old) = dict.set(name, restval.clone_with_heap(vm.heap), vm)? {
                old.drop_with(vm);
            }
        }
    }
    let (dict, vm) = dict_guard.into_parts();
    Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Dict(dict)))))
}

/// `DictWriter.writerow(rowdict)`: flattens the dict in `fieldnames` order and writes it.
pub(super) fn writer_writerow<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let rowdict = args.get_one_arg("DictWriter.writerow", vm.heap)?;
    defer_drop!(rowdict, vm);
    let items = dict_to_list(obj, rowdict, vm)?;
    defer_drop!(items, vm);
    write_items(obj, items, vm)
}

/// `DictWriter.writeheader()`: writes `fieldnames` as a row, returning what `write()` did.
pub(super) fn writer_writeheader<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    args.check_zero_args("DictWriter.writeheader", vm.heap)?;
    let fieldnames = dict_writer_part(obj, vm, |writer| writer.fieldnames.clone_with_heap(vm.heap));
    defer_drop!(fieldnames, vm);
    let items = collect_iterable(fieldnames, vm)?;
    defer_drop!(items, vm);
    write_items(obj, items, vm)
}

/// `DictWriter.writerows(rowdicts)`: flattens and writes each dict.
///
/// A file target gets every row in one host write, so the dicts are all flattened first;
/// any other target is written synchronously as each dict arrives.
pub(super) fn writer_writerows<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let rowdicts = args.get_one_arg("DictWriter.writerows", vm.heap)?;
    defer_drop!(rowdicts, vm);
    let writer = dict_writer_part(obj, vm, |writer| writer.writer.clone_with_heap(vm.heap));
    defer_drop!(writer, vm);
    let Some((targets_file, write, dialect)) = writer_parts(writer, vm) else {
        return Err(RunError::internal("DictWriter: writer is not a _csv.writer"));
    };
    defer_drop!(write, vm);
    if targets_file {
        let rows = collect_iterable(rowdicts, vm)?;
        defer_drop!(rows, vm);
        let mut lists_guard = DropGuard::new(Vec::with_capacity(rows.len()), vm);
        let (flattened, vm) = lists_guard.as_parts_mut();
        for rowdict in rows {
            let items = dict_to_list(obj, rowdict, vm)?;
            flattened.push(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))));
        }
        let (lists, vm) = lists_guard.into_parts();
        let rows = Value::Ref(vm.heap.allocate(HeapData::List(List::new(lists))));
        with_writer(writer, rows, vm, |writer_obj, rows, vm| {
            super::writer::writerows(writer_obj, ArgValues::One(rows), vm)
        })
    } else {
        let iterator = rowdicts.py_iter(vm)?;
        defer_drop!(iterator, vm);
        let terminator_chars = terminator_chars(&dialect.lineterminator);
        // One view across the loop: its step count is what polls the time limit.
        let mut rows = iterator.read(vm);
        while let Some(rowdict) = rows.py_next(vm)? {
            defer_drop!(rowdict, vm);
            let items = dict_to_list(obj, rowdict, vm)?;
            let row = Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))));
            defer_drop!(row, vm);
            writerow_now(write, &dialect, &terminator_chars, row, vm)?;
        }
        Ok(CallResult::Value(Value::None))
    }
}

/// `DictWriter._dict_to_list`: `rowdict.get(key, restval)` for each field name, after the
/// `extrasaction='raise'` check for keys outside `fieldnames`.
fn dict_to_list<'h>(obj: &mut HeapRead<'h, CsvObject>, rowdict: &Value, vm: &mut VM<'h>) -> RunResult<Vec<Value>> {
    let (fieldnames, restval, extrasaction) = {
        let CsvObject::DictWriter(writer) = obj.get(vm.heap) else {
            unreachable!("dispatched on CsvObject::DictWriter")
        };
        (
            writer.fieldnames.clone_with_heap(vm.heap),
            writer.restval.clone_with_heap(vm.heap),
            writer.extrasaction.clone_with_heap(vm.heap),
        )
    };
    defer_drop!(fieldnames, vm);
    defer_drop!(restval, vm);
    defer_drop!(extrasaction, vm);
    // `self.extrasaction == "raise"`, read at each write since the attribute is assignable.
    let raise_on_extra = extrasaction.is_str(vm.heap) && extrasaction.to_str(vm)? == "raise";
    let names = collect_iterable(fieldnames, vm)?;
    defer_drop!(names, vm);

    let Value::Ref(dict_id) = rowdict else {
        return Err(dict_method_missing(rowdict, raise_on_extra, vm));
    };
    let HeapReadOutput::Dict(dict) = vm.heap.read(*dict_id) else {
        return Err(dict_method_missing(rowdict, raise_on_extra, vm));
    };
    if raise_on_extra {
        check_extra_keys(&dict, names, vm)?;
    }
    // One list per row: `writerows` on a file keeps every row's list until the single
    // write, so each is preflighted against the usage so far.
    vm.heap.tracker.check_allocation(names.len() * VALUE_SIZE)?;
    let mut items_guard = DropGuard::new(Vec::with_capacity(names.len()), vm);
    let (items, vm) = items_guard.as_parts_mut();
    for name in names {
        let value = match dict.dict_get(name, vm)? {
            Some(value) => value,
            None => restval.clone_with_heap(vm.heap),
        };
        items.push(value);
    }
    Ok(items_guard.into_inner())
}

/// The `AttributeError` CPython raises for a row that is not a mapping: `.keys()` is
/// what the `raise` check touches first, `.get()` otherwise.
fn dict_method_missing(rowdict: &Value, raise_on_extra: bool, vm: &VM<'_>) -> RunError {
    let method = if raise_on_extra { "keys" } else { "get" };
    ExcType::attribute_error(rowdict.py_type_name(vm), method)
}

/// `rowdict.keys() - self.fieldnames`: the `ValueError` for keys outside `fieldnames`.
/// The names go through a set as in CPython, so they must be hashable.
fn check_extra_keys<'h>(dict: &HeapRead<'h, Dict>, names: &[Value], vm: &mut VM<'h>) -> RunResult<()> {
    let mut known_guard = DropGuard::new(Set::new(), vm);
    let (known, vm) = known_guard.as_parts_mut();
    for name in names {
        known.add(name.clone_with_heap(vm.heap), vm)?;
    }
    let (known, vm) = known_guard.into_parts();
    let known = Value::Ref(vm.heap.allocate(HeapData::Set(known)));
    defer_drop!(known, vm);
    let Value::Ref(known_id) = known else {
        unreachable!("allocated above")
    };
    let HeapReadOutput::Set(known) = vm.heap.read(*known_id) else {
        unreachable!("allocated above")
    };
    let keys = dict_keys(dict, vm);
    defer_drop!(keys, vm);
    let mut wrong = Vec::new();
    for key in keys {
        if !known.contains(key, vm)? {
            let repr = key.py_repr(vm)?;
            defer_drop!(repr, vm);
            wrong.push(repr.to_str(vm)?.to_owned());
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(ExcType::value_error(format!(
            "dict contains fields not in fieldnames: {}",
            wrong.join(", ")
        )))
    }
}

/// Owned clones of a dict's keys, in insertion order.
fn dict_keys<'h>(dict: &HeapRead<'h, Dict>, vm: &VM<'h>) -> Vec<Value> {
    let dict = dict.get(vm.heap);
    (0..dict.len())
        .filter_map(|index| dict.key_at(index))
        .map(|key| key.clone_with_heap(vm.heap))
        .collect()
}

/// Formats `items` and writes them through the wrapped `_csv.writer`.
fn write_items<'h>(obj: &mut HeapRead<'h, CsvObject>, items: &[Value], vm: &mut VM<'h>) -> RunResult<CallResult> {
    let writer = dict_writer_part(obj, vm, |writer| writer.writer.clone_with_heap(vm.heap));
    defer_drop!(writer, vm);
    let row = Value::Ref(vm.heap.allocate(HeapData::List(List::new(
        items.iter().map(|item| item.clone_with_heap(vm.heap)).collect(),
    ))));
    with_writer(writer, row, vm, |writer_obj, row, vm| {
        super::writer::writerow(writer_obj, ArgValues::One(row), vm)
    })
}

/// Runs `call` on the wrapped `_csv.writer` with `row`, which is released if `writer` is
/// not one (an internal error: the constructor always stores one).
fn with_writer<'h, T>(
    writer: &Value,
    row: Value,
    vm: &mut VM<'h>,
    call: impl FnOnce(&mut HeapRead<'h, CsvObject>, Value, &mut VM<'h>) -> RunResult<T>,
) -> RunResult<T> {
    let mut row_guard = DropGuard::new(row, vm);
    let (_, vm) = row_guard.as_parts_mut();
    let Value::Ref(writer_id) = writer else {
        return Err(RunError::internal("DictWriter: writer is not a _csv.writer"));
    };
    let HeapReadOutput::Csv(mut writer_obj) = vm.heap.read(*writer_id) else {
        return Err(RunError::internal("DictWriter: writer is not a _csv.writer"));
    };
    let (row, vm) = row_guard.into_parts();
    call(&mut writer_obj, row, vm)
}

/// Whether the wrapped `_csv.writer` targets a file, with its bound `write` and dialect;
/// `None` if `writer` is not one.
fn writer_parts(writer: &Value, vm: &VM<'_>) -> Option<(bool, Value, Dialect)> {
    match writer {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Csv(csv) => match &**csv {
                CsvObject::Writer(writer) => Some((
                    writer.targets_file(vm),
                    writer.write_method(vm.heap),
                    writer.dialect.clone(),
                )),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Reads one field of the `DictWriter` under a short borrow.
fn dict_writer_part<'h, T>(obj: &HeapRead<'h, CsvObject>, vm: &VM<'h>, part: impl FnOnce(&DictWriter) -> T) -> T {
    let CsvObject::DictWriter(writer) = obj.get(vm.heap) else {
        unreachable!("dispatched on CsvObject::DictWriter")
    };
    part(writer)
}

/// `next(reader)` on the wrapped `_csv.reader`, charging a recursion level for the re-entry.
fn next_row(reader: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    reader.read(vm).py_next(vm)
}

/// The wrapped reader's `line_num`.
fn reader_line_num(reader: &Value, vm: &VM<'_>) -> u64 {
    match reader {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Csv(csv) => match &**csv {
                CsvObject::Reader(reader) => reader.line_num,
                _ => 0,
            },
            _ => 0,
        },
        _ => 0,
    }
}

/// The length of a row list.
fn row_len(row: &Value, vm: &VM<'_>) -> usize {
    row.py_len(vm).unwrap_or(0)
}

/// Owned clones of a row list's items.
fn row_values(row: &Value, vm: &VM<'_>) -> Vec<Value> {
    match row {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => list
                .as_slice()
                .iter()
                .map(|item| item.clone_with_heap(vm.heap))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}
