//! Restores names copied into pre-v16 dumps as ordinary Python string values.
//!
//! The legacy text exists only during decoding; replace it before any other
//! upgrade or Python execution can read the names or traverse their heap edges.

use std::mem;

use serde::{Deserialize, Deserializer};

use crate::{
    heap::{Heap, HeapData, HeapReadMut, HeapReadOutput, HeapReader},
    intern::StringId,
    value::EitherStr,
};

/// The owned names of a heap object, in stable order across the two upgrade passes.
pub(crate) trait StoredNames {
    /// Visits every name, including field names retained separately by namedtuple instances.
    fn names_mut(&mut self) -> impl Iterator<Item = &mut EitherStr>;
}

/// Restores retained names for dump loading and direct serde deserialization alike.
pub(crate) fn deserialize_heap<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Heap, D::Error> {
    let mut heap = Heap::deserialize(deserializer)?;
    upgrade(&mut heap);
    Ok(heap)
}

/// Replaces legacy text with owned string identities before exposing the restored session.
fn upgrade(heap: &mut Heap) {
    let ids = heap.ids_where(|data| {
        matches!(
            data,
            HeapData::Class(_) | HeapData::HostClassType(_) | HeapData::NamedTuple(_) | HeapData::NamedTupleClass(_)
        )
    });
    HeapReader::with(heap, &mut (), |heap, ()| {
        for id in ids {
            match heap.read(id) {
                HeapReadOutput::Class(value) => upgrade_names(&value, heap),
                HeapReadOutput::HostClassType(value) => upgrade_names(&value, heap),
                HeapReadOutput::NamedTuple(value) => upgrade_names(&value, heap),
                HeapReadOutput::NamedTupleClass(value) => upgrade_names(&value, heap),
                _ => unreachable!("filtered to objects with stored names"),
            }
        }
    });
}

/// Takes legacy text out before allocating, so no payload borrow spans heap allocation.
fn upgrade_names<'h>(value: &HeapReadMut<'h, impl StoredNames>, heap: &mut HeapReader<'h>) {
    if !value
        .get_mut(heap)
        .names_mut()
        .any(|name| matches!(name, EitherStr::LegacyText(_)))
    {
        return;
    }
    let pending: Vec<_> = value
        .get_mut(heap)
        .names_mut()
        .map(|name| {
            if matches!(name, EitherStr::LegacyText(_)) {
                let EitherStr::LegacyText(text) = mem::replace(name, EitherStr::Interned(StringId::EMPTY)) else {
                    unreachable!("matched legacy text")
                };
                Some(text)
            } else {
                None
            }
        })
        .collect();
    let replacements: Vec<_> = pending
        .into_iter()
        .map(|text| text.map(|text| EitherStr::new(text, heap)))
        .collect();
    for (name, replacement) in value.get_mut(heap).names_mut().zip(replacements) {
        if let Some(replacement) = replacement {
            *name = replacement;
        }
    }
}
