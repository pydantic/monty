use crate::{
    exception_private::RunResult,
    heap::{Heap, HeapData},
    value::{VALUE_SIZE, Value},
};

/// Read-only access to built-in sequence elements while the heap is borrowed.
#[derive(Clone, Copy)]
pub(crate) struct SequenceItems<'a>(&'a [Value]);

impl<'a> SequenceItems<'a> {
    pub(crate) fn from_slice(items: &'a [Value]) -> Self {
        Self(items)
    }

    /// Borrows list, tuple, and named tuple elements without invoking Python iteration.
    pub(crate) fn from_value(value: &Value, heap: &'a Heap) -> Option<Self> {
        Self::from_heap_data(heap.get(value.ref_id()?))
    }

    /// Borrows elements from a payload the caller already looked up.
    pub(crate) fn from_heap_data(data: &'a HeapData) -> Option<Self> {
        match data {
            HeapData::List(list) => Some(list.items()),
            HeapData::Tuple(tuple) => Some(Self::from_slice(tuple.as_slice())),
            HeapData::NamedTuple(tuple) => Some(Self::from_slice(tuple.items())),
            _ => None,
        }
    }

    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    /// The callback's element reference cannot escape, so compact storage can materialize a value on demand.
    pub(crate) fn map_item<T>(self, index: usize, f: impl FnOnce(&Value) -> T) -> T {
        f(&self.0[index])
    }

    pub(crate) fn map<T>(self, f: impl FnMut(&Value) -> T) -> impl DoubleEndedIterator<Item = T> + ExactSizeIterator {
        self.0.iter().map(f)
    }

    pub(crate) fn filter_map<T>(self, f: impl FnMut(&Value) -> Option<T>) -> impl Iterator<Item = T> {
        self.0.iter().filter_map(f)
    }

    pub(crate) fn to_vec(self, heap: &Heap) -> RunResult<Vec<Value>> {
        heap.tracker.check_allocation(self.len().saturating_mul(VALUE_SIZE))?;
        Ok(self.cloned(heap).collect())
    }

    pub(crate) fn cloned(self, heap: &Heap) -> impl DoubleEndedIterator<Item = Value> + ExactSizeIterator {
        self.map(|item| item.clone_with_heap(heap))
    }
}
