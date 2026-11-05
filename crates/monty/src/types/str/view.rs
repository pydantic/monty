//! Inline string ranges owning one reference to a heap-backed string.

use std::ops::Range;

use crate::{
    exception_private::RunResult,
    hash::HashValue,
    heap::{Heap, HeapData, HeapId},
};

/// Twelve payload bytes fit beside Value's existing tag without growing a value slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct StringView {
    #[serde(rename = "B")]
    backing: u32,
    #[serde(rename = "S")]
    start: u32,
    #[serde(rename = "L")]
    len: u32,
}

impl StringView {
    /// Returns None rather than truncating a backing ID or byte bound.
    pub(crate) fn new(backing: HeapId, start: usize, len: usize) -> Option<Self> {
        Some(Self {
            backing: u32::try_from(backing.index()).ok()?,
            start: u32::try_from(start).ok()?,
            len: u32::try_from(len).ok()?,
        })
    }

    pub(crate) fn backing(self) -> HeapId {
        HeapId::from_index(self.backing as usize)
    }

    pub(crate) fn start(self) -> usize {
        self.start as usize
    }

    pub(crate) fn byte_len(self) -> usize {
        self.len as usize
    }

    pub(crate) fn range(self) -> Range<usize> {
        self.start()..self.start() + self.byte_len()
    }

    pub(crate) fn hash(self, heap: &Heap) -> RunResult<HashValue> {
        let HeapData::Str(source) = heap.get(self.backing()) else {
            panic!("string view backing is a string");
        };
        source.3.view_hash(source.as_str(), self.range(), &heap.tracker)
    }

    pub(crate) fn as_str(self, heap: &Heap) -> &str {
        let HeapData::Str(source) = heap.get(self.backing()) else {
            panic!("string view backing is a string");
        };
        &source.as_str()[self.range()]
    }
}
