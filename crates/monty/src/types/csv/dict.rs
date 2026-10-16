//! `csv.DictReader` and `csv.DictWriter`: the pure-Python wrappers of CPython's `csv.py`,
//! mapping rows to and from dicts keyed by `fieldnames`.

use std::{mem, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapObjectRead, HeapRead, HeapReadOutput},
    string_builder::StringBuilder,
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
    /// Keys in the last row's dict: the capacity the next one starts at. Below the header
    /// width when a name repeats, since the repeat overwrites its slot; zero before any row.
    #[serde(skip)]
    dict_hint: usize,
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
            dict_hint: 0,
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
    if unset && let Some(header) = next_row(reader, vm)? {
        let header = Value::Ref(vm.heap.allocate(HeapData::List(List::new(header))));
        if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
            dict_reader.fieldnames = Some(header);
        }
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
    let (reader, fieldnames, restkey, restval, dict_hint) = {
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
            dict_reader.dict_hint,
        )
    };
    defer_drop!(reader, vm);
    defer_drop!(fieldnames, vm);
    defer_drop!(restkey, vm);
    defer_drop!(restval, vm);

    let Some(items) = next_row(reader, vm)? else {
        return Ok(None);
    };
    let mut items_guard = DropGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();
    let line_num = reader_line_num(reader, vm);
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.line_num = line_num;
    }
    // Blank lines yield empty rows, which CPython skips here. A native loop over the
    // source, so it polls the clock: an endless run of blank lines must still time out.
    let mut skipped = 0usize;
    while items.is_empty() {
        vm.heap.tracker.check_time_every(skipped)?;
        skipped += 1;
        let Some(next) = next_row(reader, vm)? else {
            return Ok(None);
        };
        mem::replace(items, next).drop_with(vm);
    }
    // CPython reads `self.fieldnames` to build the dict, and the property refreshes `line_num`.
    let line_num = reader_line_num(reader, vm);
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.line_num = line_num;
    }

    let names = fieldnames_items(fieldnames, vm)?;
    let mut names_guard = DropGuard::new(names, vm);
    let (names, vm) = names_guard.as_parts_mut();

    let (name_count, item_count) = (names.len(), items.len());
    // The last row's key count predicts this one's; the first row assumes distinct names.
    let capacity = if dict_hint == 0 { name_count } else { dict_hint };
    // The table is sized up front from a width the caller controls, so charge it as a
    // dict copy does before it exists.
    vm.heap
        .tracker
        .check_allocation(capacity.saturating_mul(2 * VALUE_SIZE))?;
    let mut dict_guard = DropGuard::new(Dict::with_capacity(capacity), vm);
    let (dict, vm) = dict_guard.as_parts_mut();
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
    let key_count = dict.len();
    if let CsvObject::DictReader(dict_reader) = obj.get_mut(vm.heap) {
        dict_reader.dict_hint = key_count;
    }
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
    let spec: Option<RowSpec> = None;
    defer_drop_mut!(spec, vm);
    let items = dict_to_list(obj, spec, rowdict, vm)?;
    defer_drop!(items, vm);
    write_items(obj, items, vm)
}

/// `DictWriter.writeheader()`: `self.writerow(dict(zip(self.fieldnames, self.fieldnames)))`,
/// returning what `write()` did. The dict is what rejects an unhashable name, and a repeated
/// name collapses in it, so the row goes through it as in CPython.
pub(super) fn writer_writeheader<'h>(
    obj: &mut HeapRead<'h, CsvObject>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    args.check_zero_args("DictWriter.writeheader", vm.heap)?;
    let fieldnames = dict_writer_part(obj, vm, |writer| writer.fieldnames.clone_with_heap(vm.heap));
    defer_drop!(fieldnames, vm);
    let names = fieldnames_items(fieldnames, vm)?;
    defer_drop!(names, vm);
    let mut header_guard = DropGuard::new(Dict::with_capacity(names.len()), vm);
    let (header, vm) = header_guard.as_parts_mut();
    for name in names {
        if let Some(old) = header.set(name.clone_with_heap(vm.heap), name.clone_with_heap(vm.heap), vm)? {
            old.drop_with(vm);
        }
    }
    let (header, vm) = header_guard.into_parts();
    let header = Value::Ref(vm.heap.allocate(HeapData::Dict(header)));
    defer_drop!(header, vm);
    let spec: Option<RowSpec> = None;
    defer_drop_mut!(spec, vm);
    let items = dict_to_list(obj, spec, header, vm)?;
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
    // Filled by the first row and refreshed whenever an attribute changes between rows.
    let spec: Option<RowSpec> = None;
    defer_drop_mut!(spec, vm);
    if targets_file {
        let rows = collect_iterable(rowdicts, vm)?;
        defer_drop!(rows, vm);
        let mut lists_guard = DropGuard::new(Vec::with_capacity(rows.len()), vm);
        let (flattened, vm) = lists_guard.as_parts_mut();
        for rowdict in rows {
            let items = dict_to_list(obj, spec, rowdict, vm)?;
            flattened.push(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))));
        }
        let (lists, vm) = lists_guard.into_parts();
        let rows = Value::Ref(vm.heap.allocate(HeapData::List(List::new(lists))));
        defer_drop!(rows, vm);
        with_writer(writer, vm, |writer_obj, vm| {
            let rows = rows.clone_with_heap(vm.heap);
            super::writer::writerows(writer_obj, ArgValues::One(rows), vm)
        })
    } else {
        let iterator = rowdicts.py_iter(vm)?;
        defer_drop!(iterator, vm);
        let terminator_chars = terminator_chars(&dialect.lineterminator);
        let mut line = String::new();
        // One view across the loop: its step count is what polls the time limit.
        let mut rows = iterator.read(vm);
        while let Some(rowdict) = rows.py_next(vm)? {
            defer_drop!(rowdict, vm);
            let items = dict_to_list(obj, spec, rowdict, vm)?;
            defer_drop!(items, vm);
            writerow_now(write, &dialect, &terminator_chars, items, &mut line, vm)?;
        }
        Ok(CallResult::Value(Value::None))
    }
}

/// What flattening a row needs: `fieldnames` as a list, the set CPython subtracts them
/// through under `extrasaction='raise'` (`None` under `'ignore'`), and `restval`.
///
/// CPython reads the attributes for every row, so a `writerows` call keeps one of these
/// and rebuilds it when [`RowSpec::is_current`] sees an attribute change between rows.
struct RowSpec {
    /// The attribute objects the spec was read from, kept to detect reassignment.
    fieldnames: Value,
    restval: Value,
    extrasaction: Value,
    names: Vec<Value>,
    known: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for RowSpec {
    fn drop_with(self, ctx: &mut C) {
        self.fieldnames.drop_with(ctx);
        self.restval.drop_with(ctx);
        self.extrasaction.drop_with(ctx);
        self.names.drop_with(ctx);
        self.known.drop_with(ctx);
    }
}

impl RowSpec {
    /// Reads the spec off the writer for `rowdict`. Under `'raise'` CPython reaches
    /// `rowdict.keys()` before it hashes the names, so a row that is not a dict fails here
    /// first; under `'ignore'` it iterates `fieldnames` before `rowdict.get()`.
    fn read<'h>(obj: &HeapRead<'h, CsvObject>, rowdict: &Value, vm: &mut VM<'h>) -> RunResult<Self> {
        let (fieldnames, restval, extrasaction) = dict_writer_part(obj, vm, |writer| {
            (
                writer.fieldnames.clone_with_heap(vm.heap),
                writer.restval.clone_with_heap(vm.heap),
                writer.extrasaction.clone_with_heap(vm.heap),
            )
        });
        defer_drop!(fieldnames, vm);
        defer_drop!(restval, vm);
        defer_drop!(extrasaction, vm);
        let raise_on_extra = extrasaction.is_str(vm.heap) && extrasaction.to_str(vm)? == "raise";
        if raise_on_extra && row_dict(rowdict, vm).is_none() {
            return Err(dict_method_missing(rowdict, raise_on_extra, vm));
        }
        let mut names_guard = DropGuard::new(fieldnames_items(fieldnames, vm)?, vm);
        let (names, vm) = names_guard.as_parts_mut();
        let known = if raise_on_extra {
            Some(known_names(names, vm)?)
        } else {
            None
        };
        let (names, vm) = names_guard.into_parts();
        Ok(Self {
            fieldnames: fieldnames.clone_with_heap(vm.heap),
            restval: restval.clone_with_heap(vm.heap),
            extrasaction: extrasaction.clone_with_heap(vm.heap),
            names,
            known,
        })
    }

    /// Whether the attributes still describe this spec: the same three objects, and for a
    /// `fieldnames` list the same items, since the list can be mutated in place between
    /// rows. A mutable iterable of any other kind (a set, a dict view) is reread per row.
    fn is_current(&self, fieldnames: &Value, restval: &Value, extrasaction: &Value, vm: &VM<'_>) -> bool {
        self.fieldnames.is(fieldnames)
            && self.restval.is(restval)
            && self.extrasaction.is(extrasaction)
            && match fieldnames {
                Value::Ref(id) => match vm.heap.get(*id) {
                    HeapData::List(list) => {
                        let items = list.as_slice();
                        items.len() == self.names.len() && items.iter().zip(&self.names).all(|(a, b)| a.is(b))
                    }
                    HeapData::Tuple(_)
                    | HeapData::Str(_)
                    | HeapData::Bytes(_)
                    | HeapData::Range(_)
                    | HeapData::FrozenSet(_) => true,
                    _ => false,
                },
                _ => true,
            }
    }
}

/// `set(fieldnames)`, which the `extrasaction='raise'` check subtracts the row's keys
/// through as CPython does, so the names must be hashable.
fn known_names(names: &[Value], vm: &mut VM<'_>) -> RunResult<Value> {
    let mut known_guard = DropGuard::new(Set::new(), vm);
    let (known, vm) = known_guard.as_parts_mut();
    for name in names {
        known.add(name.clone_with_heap(vm.heap), vm)?;
    }
    let (known, vm) = known_guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::Set(known))))
}

/// `DictWriter._dict_to_list`: `rowdict.get(key, restval)` for each field name, after the
/// `extrasaction='raise'` check for keys outside `fieldnames`.
///
/// `spec` is the caller's cache across rows, filled or refreshed here when the
/// attributes it was read from changed; the attributes are untouched until a row arrives.
fn dict_to_list<'h>(
    obj: &HeapRead<'h, CsvObject>,
    spec: &mut Option<RowSpec>,
    rowdict: &Value,
    vm: &mut VM<'h>,
) -> RunResult<Vec<Value>> {
    let current = dict_writer_part(obj, vm, |writer| {
        spec.as_ref()
            .is_some_and(|spec| spec.is_current(&writer.fieldnames, &writer.restval, &writer.extrasaction, vm))
    });
    if !current {
        if let Some(stale) = spec.take() {
            stale.drop_with(vm);
        }
        *spec = Some(RowSpec::read(obj, rowdict, vm)?);
    }
    let Some(spec) = spec else { unreachable!("filled above") };
    let raise_on_extra = spec.known.is_some();
    let Some(dict) = row_dict(rowdict, vm) else {
        return Err(dict_method_missing(rowdict, raise_on_extra, vm));
    };
    if let Some(known) = &spec.known {
        check_extra_keys(&dict, known, vm)?;
    }
    // One list per row: `writerows` on a file keeps every row's list until the single
    // write, so each is preflighted against the usage so far.
    vm.heap.tracker.check_allocation(spec.names.len() * VALUE_SIZE)?;
    let mut items_guard = DropGuard::new(Vec::with_capacity(spec.names.len()), vm);
    let (items, vm) = items_guard.as_parts_mut();
    for name in &spec.names {
        let value = match dict.dict_get(name, vm)? {
            Some(value) => value,
            None => spec.restval.clone_with_heap(vm.heap),
        };
        items.push(value);
    }
    Ok(items_guard.into_inner())
}

/// The row as a dict, or `None` when it is anything else (see [`dict_method_missing`]).
fn row_dict<'h>(rowdict: &Value, vm: &VM<'h>) -> Option<HeapObjectRead<'h, Dict>> {
    match rowdict {
        Value::Ref(id) => match vm.heap.read(*id) {
            HeapReadOutput::Dict(dict) => Some(dict),
            _ => None,
        },
        _ => None,
    }
}

/// The `AttributeError` CPython raises for a row that is not a mapping: `.keys()` is
/// what the `raise` check touches first, `.get()` otherwise.
fn dict_method_missing(rowdict: &Value, raise_on_extra: bool, vm: &VM<'_>) -> RunError {
    let method = if raise_on_extra { "keys" } else { "get" };
    ExcType::attribute_error(rowdict.py_type_name(vm), method)
}

/// `rowdict.keys() - self.fieldnames`: the `ValueError` for keys outside `known`, the set
/// [`known_names`] built.
fn check_extra_keys<'h>(dict: &HeapRead<'h, Dict>, known: &Value, vm: &mut VM<'h>) -> RunResult<()> {
    let Value::Ref(known_id) = known else {
        unreachable!("known_names allocates a set")
    };
    let HeapReadOutput::Set(known) = vm.heap.read(*known_id) else {
        unreachable!("known_names allocates a set")
    };
    let keys = dict_keys(dict, vm)?;
    defer_drop!(keys, vm);
    // The reprs of the offending keys outlive the keys' own tracked reprs, so the message
    // grows through the tracker; a wide row allocates per key, so the loop also polls.
    let mut wrong = String::new();
    // A key's repr may be empty, so the message's emptiness cannot stand in for this.
    let mut any_wrong = false;
    for (i, key) in keys.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        if !known.contains(key, vm)? {
            let repr = key.py_repr(vm)?;
            defer_drop!(repr, vm);
            let mut builder = StringBuilder::from_existing(mem::take(&mut wrong), &vm.heap.tracker);
            if any_wrong {
                builder.push_str(", ")?;
            }
            builder.push_str(repr.to_str(vm)?)?;
            wrong = builder.finish_raw()?;
            any_wrong = true;
        }
    }
    if any_wrong {
        Err(ExcType::value_error(format!(
            "dict contains fields not in fieldnames: {wrong}"
        )))
    } else {
        Ok(())
    }
}

/// Owned clones of a dict's keys, in insertion order: a bulk copy, preflighted as one.
fn dict_keys<'h>(dict: &HeapRead<'h, Dict>, vm: &VM<'h>) -> RunResult<Vec<Value>> {
    let dict = dict.get(vm.heap);
    vm.heap
        .tracker
        .check_allocation(dict.len().saturating_mul(VALUE_SIZE))?;
    Ok(dict.iter().map(|(key, _)| key.clone_with_heap(vm.heap)).collect())
}

/// Formats `items` and writes them through the wrapped `_csv.writer`.
fn write_items<'h>(obj: &mut HeapRead<'h, CsvObject>, items: &[Value], vm: &mut VM<'h>) -> RunResult<CallResult> {
    let writer = dict_writer_part(obj, vm, |writer| writer.writer.clone_with_heap(vm.heap));
    defer_drop!(writer, vm);
    with_writer(writer, vm, |writer_obj, vm| {
        super::writer::write_fields(writer_obj, items, vm)
    })
}

/// Runs `call` on the wrapped `_csv.writer`, an internal error if `writer` is not one (the
/// constructor always stores one).
fn with_writer<'h, T>(
    writer: &Value,
    vm: &mut VM<'h>,
    call: impl FnOnce(&mut HeapRead<'h, CsvObject>, &mut VM<'h>) -> RunResult<T>,
) -> RunResult<T> {
    let Value::Ref(writer_id) = writer else {
        return Err(RunError::internal("DictWriter: writer is not a _csv.writer"));
    };
    let HeapReadOutput::Csv(mut writer_obj) = vm.heap.read(*writer_id) else {
        return Err(RunError::internal("DictWriter: writer is not a _csv.writer"));
    };
    call(&mut writer_obj, vm)
}

/// Whether the wrapped `_csv.writer` targets a file, with its bound `write` and dialect;
/// `None` if `writer` is not one.
fn writer_parts(writer: &Value, vm: &VM<'_>) -> Option<(bool, Value, Arc<Dialect>)> {
    match writer {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Csv(csv) => match &**csv {
                CsvObject::Writer(writer) => Some((
                    writer.targets_file(vm),
                    writer.write_method(vm.heap),
                    Arc::clone(&writer.dialect),
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

/// The next record's fields from the wrapped `_csv.reader`, taken as a vector rather than
/// through `next()` so no list is built and discarded per row.
fn next_row(reader: &Value, vm: &mut VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let Value::Ref(reader_id) = reader else {
        return Err(RunError::internal("DictReader: reader is not a _csv.reader"));
    };
    let HeapReadOutput::Csv(mut reader_obj) = vm.heap.read(*reader_id) else {
        return Err(RunError::internal("DictReader: reader is not a _csv.reader"));
    };
    super::reader::next_fields(&mut reader_obj, vm)
}

/// `fieldnames` as owned values: a `list` (what a header read produces) is copied directly,
/// preflighted as a bulk copy; any other iterable is iterated as `zip` or `list()` would.
fn fieldnames_items(fieldnames: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    match fieldnames {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => {
                let items = list.as_slice();
                vm.heap
                    .tracker
                    .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
                Ok(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect())
            }
            _ => collect_iterable(fieldnames, vm),
        },
        _ => collect_iterable(fieldnames, vm),
    }
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
