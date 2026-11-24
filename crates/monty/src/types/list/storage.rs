use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};

use super::{
    List,
    packed::{BoolView, PackedList, Packing},
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
            Self::Packed(PackedList::Floats(items)) => SequenceItems::Floats(items),
            Self::Packed(PackedList::Bools(items)) => SequenceItems::Bools(BoolView(items)),
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

#[cfg(test)]
mod tests {
    use monty_types::ResourceTracker;

    use super::*;
    use crate::types::list::packed::IntWidth;

    #[test]
    fn packed_lists_match_the_legacy_cbor_encoding() {
        #[derive(Serialize, Deserialize)]
        #[serde(rename = "List")]
        struct LegacyList {
            #[serde(rename = "I")]
            items: Vec<Value>,
            #[serde(rename = "C")]
            contains_refs: bool,
        }

        let heap = Heap::new(16, ResourceTracker::default());
        let bytes = |nullable| Packing::Int {
            width: IntWidth::Byte,
            nullable,
        };
        let wide = |nullable| Packing::Int {
            width: IntWidth::Wide,
            nullable,
        };
        for (expected, items) in [
            (Packing::Empty, vec![]),
            (Packing::Nones, vec![Value::None]),
            (bytes(false), vec![Value::Int(255)]),
            (bytes(true), vec![Value::Int(255), Value::None]),
            (wide(false), vec![Value::Int(-257)]),
            (wide(true), vec![Value::None, Value::Int(-257)]),
            (Packing::Bool { nullable: false }, vec![Value::Bool(false)]),
            (Packing::Bool { nullable: true }, vec![Value::Bool(true), Value::None]),
            (
                Packing::Float { nullable: false },
                vec![Value::Float(-0.0), Value::Float(f64::INFINITY)],
            ),
            (
                Packing::Float { nullable: true },
                vec![Value::None, Value::Float(f64::from_bits(0x7ff8_0000_0000_0123))],
            ),
        ] {
            let legacy = LegacyList {
                items,
                contains_refs: false,
            };
            let old_bytes = minicbor_serde::to_vec(&legacy).unwrap();
            let list = List::from_values(legacy.items, &heap);
            assert_eq!(list.storage.packing(), expected);
            if matches!(expected, Packing::Bool { .. }) {
                assert!(matches!(list.items(), SequenceItems::Bools(_)));
            }
            let new_bytes = minicbor_serde::to_vec(&list).unwrap();
            assert_eq!(new_bytes, old_bytes);

            let old_reader: LegacyList = minicbor_serde::from_slice(&new_bytes).unwrap();
            let new_reader: List = minicbor_serde::from_slice(&old_bytes).unwrap();
            assert_eq!(minicbor_serde::to_vec(&old_reader).unwrap(), old_bytes);
            assert_eq!(minicbor_serde::to_vec(&new_reader).unwrap(), new_bytes);
        }
    }
}
