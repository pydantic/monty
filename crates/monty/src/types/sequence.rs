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

    pub(crate) fn list_or_tuple(value: &Value, heap: &'a Heap) -> Option<Self> {
        match heap.get(value.ref_id()?) {
            HeapData::List(list) => Some(list.items()),
            HeapData::Tuple(tuple) => Some(Self::from_slice(tuple.as_slice())),
            _ => None,
        }
    }

    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    /// The borrow ends before the result is returned, so an element can be materialized on demand.
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
