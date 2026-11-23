use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};

use super::{
    List,
    packed::{PackedList, Packing},
};
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::Heap,
    resource_checks::try_vec_with_capacity,
    types::sequence::SequenceItems,
    value::{VALUE_SIZE, Value},
};

/// The garbage collector only needs to scan generic storage.
#[derive(Debug, Default)]
pub(super) enum ListStorage {
    #[default]
    Empty,
    Packed(PackedList),
    Generic(Vec<Value>),
}

impl ListStorage {
    pub(super) fn view(&self) -> SequenceItems<'_> {
        match self {
            Self::Empty => SequenceItems::from_slice(&[]),
            Self::Packed(PackedList::ByteInts(items)) => SequenceItems::ByteInts(items),
            Self::Packed(PackedList::Ints(items)) => SequenceItems::Ints(items),
            Self::Packed(items) => SequenceItems::Packed(items.view()),
            Self::Generic(items) => SequenceItems::from_slice(items),
        }
    }

    pub(super) fn packing(&self) -> Packing {
        match self {
            Self::Empty => Packing::Empty,
            Self::Packed(items) => items.packing(),
            Self::Generic(_) => Packing::Generic,
        }
    }

    pub(super) fn capacity(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Packed(items) => items.capacity(),
            Self::Generic(items) => items.capacity(),
        }
    }

    /// Leaves room for the pending write so widening needs only one allocation.
    pub(super) fn expand_packed(items: &PackedList, additional: usize, heap: &Heap) -> RunResult<Vec<Value>> {
        let capacity = items
            .len()
            .checked_add(additional)
            .ok_or_else(ExcType::allocation_too_large)?;
        heap.tracker.check_allocation(capacity.saturating_mul(VALUE_SIZE))?;
        let mut values = try_vec_with_capacity(capacity)?;
        for index in 0..items.len() {
            heap.tracker.check_time_every(index)?;
            values.push(items.get(index).expect("checked list index"));
        }
        Ok(values)
    }
}

/// Snapshots encode every storage strategy as a sequence of `Value`s.
impl Serialize for List {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("List", 2)?;
        state.serialize_field("I", &self.items())?;
        state.serialize_field("C", &self.contains_refs)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for List {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename = "List")]
        struct Fields {
            #[serde(rename = "I")]
            items: Vec<Value>,
            #[serde(rename = "C")]
            contains_refs: bool,
        }

        let Fields { items, contains_refs } = Fields::deserialize(deserializer)?;
        let storage = if items.is_empty() {
            ListStorage::Empty
        } else {
            ListStorage::Generic(items)
        };
        Ok(Self { storage, contains_refs })
    }
}
