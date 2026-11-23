use monty_types::ResourceTracker;
use serde::{Serialize, Serializer, ser::SerializeSeq};

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
    Ints(&'a [i64]),
    ByteInts(&'a [u8]),
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
            Self::Ints(items) => items.len(),
            Self::ByteInts(items) => items.len(),
            Self::Packed(items) => items.len(),
        }
    }

    /// The callback's element reference cannot escape, so compact storage can materialize a value on demand.
    pub(crate) fn map_item<T>(self, index: usize, f: impl FnOnce(&Value) -> T) -> T {
        match self {
            Self::Values(items) => f(&items[index]),
            Self::Ints(items) => f(&Value::Int(items[index])),
            Self::ByteInts(items) => f(&Value::Int(i64::from(items[index]))),
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
        match self {
            Self::Values(items) => MappedItems::Values(items.iter().map(f)),
            Self::Ints(items) => MappedItems::Ints(items.iter().map(move |&item| f(&Value::Int(item)))),
            Self::ByteInts(items) => {
                MappedItems::ByteInts(items.iter().map(move |&item| f(&Value::Int(i64::from(item)))))
            }
            Self::Packed(items) => MappedItems::Packed(items.iter().map(move |item| f(&item))),
        }
    }

    pub(crate) fn filter_map<T>(self, mut f: impl FnMut(&Value) -> Option<T>) -> impl Iterator<Item = T> {
        match self {
            Self::Values(items) => MappedItems::Values(items.iter().filter_map(f)),
            Self::Ints(items) => MappedItems::Ints(items.iter().filter_map(move |&item| f(&Value::Int(item)))),
            Self::ByteInts(items) => {
                MappedItems::ByteInts(items.iter().filter_map(move |&item| f(&Value::Int(i64::from(item)))))
            }
            Self::Packed(items) => MappedItems::Packed(items.iter().filter_map(move |item| f(&item))),
        }
    }

    pub(crate) fn to_vec(self, heap: &Heap) -> RunResult<Vec<Value>> {
        heap.tracker.check_allocation(self.len().saturating_mul(VALUE_SIZE))?;
        Ok(match self {
            Self::Values(items) => items.iter().map(|item| item.clone_with_heap(heap)).collect(),
            Self::Ints(items) => items.iter().copied().map(Value::Int).collect(),
            Self::ByteInts(items) => items.iter().map(|&item| Value::Int(i64::from(item))).collect(),
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

    pub(crate) fn cloned(self, heap: &Heap) -> impl DoubleEndedIterator<Item = Value> + ExactSizeIterator {
        self.map(|item| item.clone_with_heap(heap))
    }
}

impl Serialize for SequenceItems<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Values(items) => items.serialize(serializer),
            Self::Ints(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for &item in *items {
                    seq.serialize_element(&Value::Int(item))?;
                }
                seq.end()
            }
            Self::ByteInts(items) => serializer.collect_seq(items.iter().map(|&item| Value::Int(i64::from(item)))),
            Self::Packed(items) => serializer.collect_seq(items.iter()),
        }
    }
}

enum MappedItems<V, I, B, P> {
    Values(V),
    Ints(I),
    ByteInts(B),
    Packed(P),
}

impl<T, V: Iterator<Item = T>, I: Iterator<Item = T>, B: Iterator<Item = T>, P: Iterator<Item = T>> Iterator
    for MappedItems<V, I, B, P>
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self {
            Self::Values(items) => items.next(),
            Self::Ints(items) => items.next(),
            Self::ByteInts(items) => items.next(),
            Self::Packed(items) => items.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Values(items) => items.size_hint(),
            Self::Ints(items) => items.size_hint(),
            Self::ByteInts(items) => items.size_hint(),
            Self::Packed(items) => items.size_hint(),
        }
    }
}

impl<
    T,
    V: DoubleEndedIterator<Item = T>,
    I: DoubleEndedIterator<Item = T>,
    B: DoubleEndedIterator<Item = T>,
    P: DoubleEndedIterator<Item = T>,
> DoubleEndedIterator for MappedItems<V, I, B, P>
{
    fn next_back(&mut self) -> Option<T> {
        match self {
            Self::Values(items) => items.next_back(),
            Self::Ints(items) => items.next_back(),
            Self::ByteInts(items) => items.next_back(),
            Self::Packed(items) => items.next_back(),
        }
    }
}

impl<
    T,
    V: ExactSizeIterator<Item = T>,
    I: ExactSizeIterator<Item = T>,
    B: ExactSizeIterator<Item = T>,
    P: ExactSizeIterator<Item = T>,
> ExactSizeIterator for MappedItems<V, I, B, P>
{
}
