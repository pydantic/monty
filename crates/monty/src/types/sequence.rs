//! Borrowed element access for built-in lists, tuples, and named tuples.
//!
//! VM operations such as unpacking can read these sequences without invoking Python iteration.
//! [`SequenceItems::map_item`] lends the callback an existing `Value` slot or materializes one
//! packed element for the duration of the call.

use monty_types::ResourceTracker;
use serde::{Serialize, Serializer};

use super::list::PackedView;
use crate::{
    exception_private::RunResult,
    heap::{Heap, HeapData},
    value::{VALUE_SIZE, Value},
};

/// Read-only access to built-in sequence elements while the heap is borrowed.
#[derive(Clone, Copy)]
pub(crate) enum SequenceItems<'a> {
    Values(&'a [Value]),
    Packed(PackedView<'a>),
}

impl<'a> SequenceItems<'a> {
    pub(crate) fn from_slice(items: &'a [Value]) -> Self {
        Self::Values(items)
    }

    /// Borrows list, tuple, and named tuple elements without invoking Python iteration.
    pub(crate) fn from_value(value: &Value, heap: &'a Heap) -> Option<Self> {
        match heap.get(value.ref_id()?) {
            HeapData::List(list) => Some(list.items()),
            data => Self::from_tuple(data),
        }
    }

    /// Borrows the elements of a plain or named tuple.
    pub(crate) fn from_tuple(data: &'a HeapData) -> Option<Self> {
        match data {
            HeapData::Tuple(tuple) => Some(Self::from_slice(tuple.as_slice())),
            HeapData::NamedTuple(tuple) => Some(Self::from_slice(tuple.items())),
            _ => None,
        }
    }

    pub(crate) fn len(self) -> usize {
        match self {
            Self::Values(items) => items.len(),
            Self::Packed(items) => items.len(),
        }
    }

    /// The callback's element reference cannot escape, so compact storage can materialize a value on demand.
    pub(crate) fn map_item<T>(self, index: usize, f: impl FnOnce(&Value) -> T) -> T {
        match self {
            Self::Values(items) => f(&items[index]),
            Self::Packed(items) => f(&items.get(index).expect("checked list index")),
        }
    }

    /// Returns an owned clone and retains any heap reference held by the element.
    #[must_use]
    pub(crate) fn clone_item(self, index: usize, heap: &Heap) -> Value {
        self.map_item(index, |item| item.clone_with_heap(heap))
    }

    pub(crate) fn map<T>(
        self,
        mut f: impl FnMut(&Value) -> T,
    ) -> impl DoubleEndedIterator<Item = T> + ExactSizeIterator {
        (0..self.len()).map(move |index| self.map_item(index, &mut f))
    }

    pub(crate) fn filter_map<T>(self, mut f: impl FnMut(&Value) -> Option<T>) -> impl Iterator<Item = T> {
        (0..self.len()).filter_map(move |index| self.map_item(index, &mut f))
    }

    pub(crate) fn to_vec(self, heap: &Heap) -> RunResult<Vec<Value>> {
        heap.tracker.check_allocation(self.len().saturating_mul(VALUE_SIZE))?;
        Ok(match self {
            Self::Values(items) => items.iter().map(|item| item.clone_with_heap(heap)).collect(),
            Self::Packed(items) => items.iter().collect(),
        })
    }

    /// Appends clones with deadline checks. The caller owns the partial copy on error.
    pub(crate) fn extend_cloned(self, output: &mut Vec<Value>, heap: &Heap) -> RunResult<()> {
        if let Self::Values(items) = self {
            for chunk in items.chunks(ResourceTracker::LOOP_CHECK_INTERVAL) {
                output.extend(chunk.iter().map(|item| item.clone_with_heap(heap)));
                heap.tracker.check_time_every(chunk.len() - 1)?;
            }
            return Ok(());
        }
        for (index, item) in self.cloned(heap).enumerate() {
            output.push(item);
            heap.tracker.check_time_every(index)?;
        }
        Ok(())
    }

    pub(crate) fn extend_cloned_reversed(self, output: &mut Vec<Value>, heap: &Heap) {
        match self {
            Self::Values(items) => output.extend(items.iter().rev().map(|item| item.clone_with_heap(heap))),
            Self::Packed(items) => items.extend_reversed(output),
        }
    }

    pub(crate) fn cloned(self, heap: &Heap) -> impl DoubleEndedIterator<Item = Value> + ExactSizeIterator {
        self.map(|item| item.clone_with_heap(heap))
    }
}

impl Serialize for SequenceItems<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Values(items) => items.serialize(serializer),
            Self::Packed(items) => serializer.collect_seq(items.iter()),
        }
    }
}
