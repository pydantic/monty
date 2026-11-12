//! String iterators retaining their source representation.

use super::{InlineStr, allocate_char};
use crate::{
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::{HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StringId,
    types::{PyTrait, Type},
    value::Value,
};

/// Source representation retained by a string iterator.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum StringIteratorSource {
    Intern(StringId),
    Inline(InlineStr),
    Heap(HeapId),
}

/// Iterator over Unicode code points with CPython's ASCII/non-ASCII type split.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StringIterator {
    source: StringIteratorSource,
    byte_offset: usize,
    ascii: bool,
}

impl StringIterator {
    /// Allocates an iterator over an interned string.
    pub(crate) fn from_intern(id: StringId, vm: &mut VM<'_>) -> Value {
        let ascii = vm.interns.str_is_ascii(id);
        Self::allocate(StringIteratorSource::Intern(id), ascii, vm)
    }

    /// Allocates an iterator over a string stored inside its `Value`.
    pub(crate) fn from_inline(text: InlineStr, vm: &mut VM<'_>) -> Value {
        Self::allocate(StringIteratorSource::Inline(text), text.is_ascii(), vm)
    }

    /// Allocates an iterator retaining a heap string.
    pub(super) fn from_heap(id: HeapId, ascii: bool, vm: &mut VM<'_>) -> Value {
        Self::allocate(StringIteratorSource::Heap(id), ascii, vm)
    }

    /// Returns the Python-visible iterator type for this string representation.
    pub(crate) fn py_type(&self) -> Type {
        if self.ascii {
            Type::StrAsciiIterator
        } else {
            Type::StrIterator
        }
    }

    /// Returns the heap source when the iterator retains one.
    pub(crate) fn source_id(&self) -> Option<HeapId> {
        match self.source {
            StringIteratorSource::Intern(_) => None,
            StringIteratorSource::Inline(_) => None,
            StringIteratorSource::Heap(id) => Some(id),
        }
    }

    /// Returns the number of Unicode code points not yet yielded.
    pub(crate) fn size_hint(&self, vm: &VM<'_>) -> usize {
        bytecount::num_chars(self.as_str(vm).as_bytes())
    }

    /// Borrows the unconsumed string suffix.
    fn as_str<'a>(&'a self, vm: &'a VM<'_>) -> &'a str {
        let string = match &self.source {
            StringIteratorSource::Intern(id) => vm.interns.get_str(*id),
            StringIteratorSource::Inline(text) => text.as_str(),
            StringIteratorSource::Heap(id) => match vm.heap.get(*id) {
                HeapData::Str(string) => string.as_str(),
                _ => unreachable!("string iterator must retain a string"),
            },
        };
        &string[self.byte_offset..]
    }

    /// Allocates an iterator and retains a heap source when present.
    fn allocate(source: StringIteratorSource, ascii: bool, vm: &mut VM<'_>) -> Value {
        let source_id = match source {
            StringIteratorSource::Heap(id) => Some(id),
            StringIteratorSource::Intern(_) => None,
            StringIteratorSource::Inline(_) => None,
        };
        let id = vm.heap.allocate(HeapData::StringIterator(Self {
            source,
            byte_offset: 0,
            ascii,
        }));
        if let Some(source_id) = source_id {
            vm.heap.inc_ref(source_id);
        }
        Value::Ref(id)
    }
}

impl HeapItem for StringIterator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(id) = self.source_id() {
            stack.push(id);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, StringIterator> {
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let next = self.get(vm.heap).as_str(vm).chars().next();
        if let Some(character) = next {
            let value = allocate_char(character);
            self.get_mut(vm.heap).byte_offset += character.len_utf8();
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use monty_types::{PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        defer_drop,
        heap::{Heap, HeapReader},
        identity::Identity,
        intern::CompileInterns,
        run::{Program, SessionTables},
        types::str::allocate_string_on_heap,
    };

    #[test]
    fn iterator_protocol_and_size_hints_across_storage_kinds() {
        let texts = ["", "ascii", "aé🐍b"];
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        let ids: Vec<_> = {
            let mut interns = CompileInterns::direct(&mut tables.interns);
            texts.iter().map(|text| interns.intern(text)).collect()
        };

        HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let vm = &mut VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            for (text, id) in texts.into_iter().zip(ids) {
                let source = allocate_string_on_heap(text, vm.heap);
                defer_drop!(source, vm);
                let source_id = source.ref_id().unwrap();
                let iterators = vec![
                    StringIterator::from_intern(id, vm),
                    StringIterator::from_inline(InlineStr::new(text).unwrap(), vm),
                    StringIterator::from_heap(source_id, text.is_ascii(), vm),
                ];
                defer_drop!(iterators, vm);

                for iterator in iterators {
                    let mut read = vm.heap.read_as::<StringIterator>(iterator.ref_id().unwrap()).unwrap();
                    assert!(read.py_is_iterable(vm));
                    assert_eq!(read.py_len(vm), None);
                    assert_eq!(read.py_eq_impl(&Value::None, vm).unwrap(), None);
                    let same = read.py_iter(vm).unwrap().require_synchronous(vm).unwrap();
                    defer_drop!(same, vm);
                    assert!(Identity::new(same) == Identity::new(iterator));

                    let mut remaining = text.chars().count();
                    assert_eq!(read.get(vm.heap).size_hint(vm), remaining);
                    for expected in text.chars() {
                        let item = read.py_next(vm).unwrap().unwrap();
                        defer_drop!(item, vm);
                        assert_eq!(item.as_str(vm).unwrap(), expected.to_string());
                        remaining -= 1;
                        assert_eq!(read.get(vm.heap).size_hint(vm), remaining);
                    }
                    assert!(read.py_next(vm).unwrap().is_none());
                    assert_eq!(read.get(vm.heap).size_hint(vm), 0);
                }
            }
        });
    }
}
