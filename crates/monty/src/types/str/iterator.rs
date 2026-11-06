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
        Self::allocate(StringIteratorSource::Inline(text), text.as_bytes().is_ascii(), vm)
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
        self.as_str(vm).chars().count()
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
