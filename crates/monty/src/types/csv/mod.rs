//! Heap objects of the `csv` module: readers, writers, `DictReader`, `DictWriter` and dialects.
//!
//! One `HeapData::Csv(CsvObject)` variant covers the family, as `Itertools` does for its
//! adaptors; the Python-visible `type()` names live in [`Type`]. A reader pulls lines from its
//! source through `py_next` and so cannot load a file itself: `modules/csv.rs` yields that load
//! at construction, before any consumer runs.

pub mod dialect;
pub mod dict;
pub mod reader;
pub mod writer;

use std::{fmt::Write, mem};

pub(crate) use dialect::{DIALECT_NAMES, Dialect, FmtParams, Quoting};
pub(crate) use dict::{DictReader, DictWriter};
pub(crate) use reader::{FIELD_SIZE_LIMIT, Reader};
use serde::{Deserialize, Serialize};
pub(crate) use writer::{Writer, write_method};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    types::{LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// One `csv` object, whichever constructor produced it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum CsvObject {
    Reader(Reader),
    Writer(Writer),
    DictReader(DictReader),
    DictWriter(DictWriter),
    Dialect(Dialect),
}

/// Which object a [`CsvObject`] is, without borrowing it: dispatch reads this tag under a
/// short borrow and lets it end before re-entering the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reader,
    Writer,
    DictReader,
    DictWriter,
    Dialect,
}

impl CsvObject {
    fn kind(&self) -> Kind {
        match self {
            Self::Reader(_) => Kind::Reader,
            Self::Writer(_) => Kind::Writer,
            Self::DictReader(_) => Kind::DictReader,
            Self::DictWriter(_) => Kind::DictWriter,
            Self::Dialect(_) => Kind::Dialect,
        }
    }

    /// The Python type this object reports.
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Reader(_) => Type::CsvReader,
            Self::Writer(_) => Type::CsvWriter,
            Self::DictReader(_) => Type::CsvDictReader,
            Self::DictWriter(_) => Type::CsvDictWriter,
            Self::Dialect(_) => Type::CsvDialect,
        }
    }

    /// Invokes `on_child` for each heap id this object owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, on_child: impl FnMut(HeapId)) {
        match self {
            Self::Reader(reader) => reader.for_each_child_id(on_child),
            Self::Writer(writer) => writer.for_each_child_id(on_child),
            Self::DictReader(reader) => reader.for_each_child_id(on_child),
            Self::DictWriter(writer) => writer.for_each_child_id(on_child),
            Self::Dialect(_) => {}
        }
    }

    /// Allocates this object on the heap.
    pub(crate) fn allocate(self, vm: &VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::Csv(Box::new(self))))
    }
}

impl HeapItem for CsvObject {
    /// Mirrors [`CsvObject::for_each_child_id`] — the two must stay in sync.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Reader(reader) => reader.py_dec_ref_ids(stack),
            Self::Writer(writer) => writer.py_dec_ref_ids(stack),
            Self::DictReader(reader) => reader.py_dec_ref_ids(stack),
            Self::DictWriter(writer) => writer.py_dec_ref_ids(stack),
            Self::Dialect(_) => {}
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, CsvObject> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Identity hashing, as for any class without `__eq__`.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        self.py_default_repr_fmt(f, vm)
    }

    fn py_is_iterator(&self, vm: &VM<'h>) -> bool {
        matches!(self.get(vm.heap).kind(), Kind::Reader | Kind::DictReader)
    }

    fn py_is_iterable(&self, vm: &VM<'h>) -> bool {
        self.py_is_iterator(vm)
    }

    /// `iter(reader) is reader`; the other objects are not iterable.
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        if self.py_is_iterator(vm) {
            Ok(self.clone_value(vm.heap))
        } else {
            Err(ExcType::type_error_not_iterable(&self.py_type_name(vm)))
        }
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self.get(vm.heap).kind() {
            Kind::Reader => reader::next(self, vm),
            Kind::DictReader => dict::reader_next(self, vm),
            Kind::Writer | Kind::DictWriter | Kind::Dialect => {
                Err(ExcType::type_error_not_iterator(&self.py_type_name(vm)))
            }
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let Some(name) = attr.static_string(vm.interns) else {
            return Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns)));
        };
        let value = match (self.get(vm.heap), name) {
            (CsvObject::Reader(reader), StaticStrings::Dialect) => {
                CsvObject::Dialect(Dialect::clone(&reader.dialect)).allocate(vm)
            }
            (CsvObject::Reader(reader), StaticStrings::LineNum) => line_num_value(reader.line_num)?,
            (CsvObject::Writer(writer), StaticStrings::Dialect) => {
                CsvObject::Dialect(Dialect::clone(&writer.dialect)).allocate(vm)
            }
            (CsvObject::DictReader(_), StaticStrings::Fieldnames) => {
                // Reading the header mutates the object, which needs its own handle.
                let HeapReadOutput::Csv(mut this) = vm.heap.read(self.id()) else {
                    unreachable!("self is a csv object")
                };
                return dict::reader_fieldnames(&mut this, vm).map(|value| Some(CallResult::Value(value)));
            }
            (CsvObject::DictReader(reader), StaticStrings::LineNum) => line_num_value(reader.line_num)?,
            (CsvObject::DictReader(reader), StaticStrings::Reader) => reader.reader.clone_with_heap(vm.heap),
            (CsvObject::DictReader(reader), StaticStrings::Restkey) => reader.restkey.clone_with_heap(vm.heap),
            (CsvObject::DictReader(reader), StaticStrings::Restval) => reader.restval.clone_with_heap(vm.heap),
            (CsvObject::DictReader(reader), StaticStrings::Dialect) => reader.dialect.clone_with_heap(vm.heap),
            (CsvObject::DictWriter(writer), StaticStrings::Fieldnames) => writer.fieldnames.clone_with_heap(vm.heap),
            (CsvObject::DictWriter(writer), StaticStrings::Restval) => writer.restval.clone_with_heap(vm.heap),
            (CsvObject::DictWriter(writer), StaticStrings::Extrasaction) => {
                writer.extrasaction.clone_with_heap(vm.heap)
            }
            (CsvObject::DictWriter(writer), StaticStrings::Writer) => writer.writer.clone_with_heap(vm.heap),
            (CsvObject::Dialect(dialect), name) => match dialect.attr(name, vm) {
                Some(value) => value,
                None => return Err(ExcType::attribute_error(Type::CsvDialect, attr.as_str(vm.interns))),
            },
            _ => return Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(value)))
    }

    /// The `DictReader` and `DictWriter` attributes that are plain Python attributes in
    /// CPython are assignable, except the wrapped `reader` / `writer` and `line_num`;
    /// everything on the `_csv` objects is read-only, with CPython's two wordings for a
    /// member and a getset descriptor.
    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        let slot = match (self.get_mut(vm.heap), name.static_string(vm.interns)) {
            (CsvObject::Reader(_), Some(StaticStrings::Dialect | StaticStrings::LineNum))
            | (CsvObject::Writer(_), Some(StaticStrings::Dialect)) => {
                value.drop_with(vm);
                return Err(ExcType::attribute_error_readonly());
            }
            (CsvObject::DictReader(reader), Some(StaticStrings::Fieldnames)) => {
                // `None` unsets the names, so the next row read is the header again.
                let names = match value {
                    Value::None => None,
                    names => Some(names),
                };
                mem::replace(&mut reader.fieldnames, names).drop_with(vm);
                return Ok(());
            }
            (CsvObject::DictReader(reader), Some(StaticStrings::Restkey)) => &mut reader.restkey,
            (CsvObject::DictReader(reader), Some(StaticStrings::Restval)) => &mut reader.restval,
            (CsvObject::DictReader(reader), Some(StaticStrings::Dialect)) => &mut reader.dialect,
            (CsvObject::DictWriter(writer), Some(StaticStrings::Fieldnames)) => &mut writer.fieldnames,
            (CsvObject::DictWriter(writer), Some(StaticStrings::Restval)) => &mut writer.restval,
            (CsvObject::DictWriter(writer), Some(StaticStrings::Extrasaction)) => &mut writer.extrasaction,
            (CsvObject::Dialect(_), Some(attr)) if Dialect::is_attr(attr) => {
                value.drop_with(vm);
                return Err(ExcType::attribute_error_not_writable(
                    name.as_str(vm.interns),
                    &Type::CsvDialect.to_string(),
                ));
            }
            _ => {
                value.drop_with(vm);
                let type_name = self.py_type_name(vm);
                return Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)));
            }
        };
        mem::replace(slot, value).drop_with(vm);
        Ok(())
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match (self.get(vm.heap).kind(), attr.static_string(vm.interns)) {
            (Kind::Writer, Some(StaticStrings::Writerow)) => writer::writerow(self, args, vm),
            (Kind::Writer, Some(StaticStrings::Writerows)) => writer::writerows(self, args, vm),
            (Kind::DictWriter, Some(StaticStrings::Writerow)) => dict::writer_writerow(self, args, vm),
            (Kind::DictWriter, Some(StaticStrings::Writerows)) => dict::writer_writerows(self, args, vm),
            (Kind::DictWriter, Some(StaticStrings::Writeheader)) => dict::writer_writeheader(self, args, vm),
            _ => {
                let type_name = self.py_type_name(vm);
                Err(ExcType::attribute_error_method(type_name, attr, args, vm))
            }
        }
    }
}

/// `line_num` as a Python `int`.
fn line_num_value(line_num: u64) -> RunResult<Value> {
    i64::try_from(line_num)
        .map(Value::Int)
        .map_err(|_| ExcType::overflow_c_ssize_t())
}
