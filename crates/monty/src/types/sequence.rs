use crate::{
    exception_private::RunResult,
    heap::Heap,
    types::{List, NamedTuple, Tuple},
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
        if let Some(list) = value.get_as::<List>(heap) {
            Some(list.items())
        } else {
            Self::from_tuple(value, heap)
        }
    }

    /// Borrows the elements of a plain or named tuple.
    pub(crate) fn from_tuple(value: &Value, heap: &'a Heap) -> Option<Self> {
        match value {
            _ if let Some(tuple) = value.get_as::<Tuple>(heap) => Some(Self::from_slice(tuple.as_slice())),
            _ if let Some(tuple) = value.get_as::<NamedTuple>(heap) => Some(Self::from_slice(tuple.items())),
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

    /// Returns an owned clone and retains any heap reference held by the element.
    #[must_use]
    pub(crate) fn clone_item(self, index: usize, heap: &Heap) -> Value {
        self.map_item(index, |item| item.clone_with_heap(heap))
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

    /// Appends clones with deadline checks. The caller owns the partial copy on error.
    pub(crate) fn extend_cloned(self, output: &mut Vec<Value>, heap: &Heap) -> RunResult<()> {
        for (index, item) in self.cloned(heap).enumerate() {
            output.push(item);
            heap.tracker.check_time_every(index)?;
        }
        Ok(())
    }

    pub(crate) fn cloned(self, heap: &Heap) -> impl DoubleEndedIterator<Item = Value> + ExactSizeIterator {
        self.map(|item| item.clone_with_heap(heap))
    }
}
