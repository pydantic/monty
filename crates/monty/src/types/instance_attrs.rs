use std::mem;

use hashbrown::HashTable;
use smallvec::SmallVec;

use super::Dict;
use crate::{
    heap::{ContainsHeap, DropWithContext, Heap, HeapData, HeapId, HeapItem},
    intern::{Interns, StringId},
    value::{EitherStr, Value},
};

const HASH_THRESHOLD: usize = 8;
const MAX_SHARED_ATTRS: usize = 32;

/// Names remain available after dictionary mode is selected, for older slot-backed instances.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(from = "LayoutData")]
pub(crate) struct InstanceLayout {
    names: Vec<StringId>,
    dictionary: bool,
    #[serde(skip)]
    index: HashTable<usize>,
}

impl InstanceLayout {
    fn rebuild_index(&mut self) {
        if self.names.len() <= HASH_THRESHOLD {
            return;
        }
        for (slot, name) in self.names.iter().enumerate() {
            self.index
                .insert_unique(name_hash(*name), slot, |slot| name_hash(self.names[*slot]));
        }
    }

    pub fn dictionary(&self) -> bool {
        self.dictionary
    }

    fn find(&self, id: StringId) -> Option<usize> {
        if self.index.is_empty() {
            self.names.iter().position(|name| *name == id)
        } else {
            self.index.find(name_hash(id), |slot| self.names[*slot] == id).copied()
        }
    }

    pub fn slot(&self, name: &EitherStr, interns: &Interns) -> Option<usize> {
        let id = match name {
            EitherStr::Interned(id) => Some(*id),
            EitherStr::Heap(s) => interns.get_string_id_by_name(s),
        }?;
        self.find(id)
    }

    /// Use dictionaries for non-interned names or more than 32 shared names.
    pub fn admit(&mut self, id: Option<StringId>) -> Option<usize> {
        if self.dictionary {
            return None;
        }
        let Some(id) = id else {
            self.dictionary = true;
            return None;
        };
        if let Some(slot) = self.find(id) {
            return Some(slot);
        }
        if self.names.len() == MAX_SHARED_ATTRS {
            self.dictionary = true;
            return None;
        }

        let slot = self.names.len();
        self.names.push(id);
        if self.names.len() > HASH_THRESHOLD {
            if self.index.is_empty() {
                self.rebuild_index();
            } else {
                self.index
                    .insert_unique(name_hash(id), slot, |slot| name_hash(self.names[*slot]));
            }
        }
        Some(slot)
    }
}

#[derive(serde::Deserialize)]
struct LayoutData {
    names: Vec<StringId>,
    dictionary: bool,
}

impl From<LayoutData> for InstanceLayout {
    fn from(data: LayoutData) -> Self {
        let mut layout = Self {
            names: data.names,
            dictionary: data.dictionary,
            index: HashTable::new(),
        };
        layout.rebuild_index();
        layout
    }
}

fn name_hash(id: StringId) -> u64 {
    // Spread sequential IDs across bucket bits and hash fingerprints.
    (id.index() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum InstanceAttrs {
    #[serde(rename = "S")]
    Slots {
        #[serde(rename = "V")]
        values: SmallVec<[Value; 4]>,
        // Assignment order belongs to the instance, even when slot numbers are shared.
        #[serde(rename = "O")]
        order: SmallVec<[u8; 4]>,
    },
    #[serde(rename = "D")]
    Dict(Dict),
}

impl Default for InstanceAttrs {
    fn default() -> Self {
        Self::Slots {
            values: SmallVec::new(),
            order: SmallVec::new(),
        }
    }
}

impl InstanceAttrs {
    pub fn get<'a>(
        &'a self,
        layout: &InstanceLayout,
        name: &EitherStr,
        heap: &Heap,
        interns: &Interns,
    ) -> Option<&'a Value> {
        match self {
            Self::Slots { values, .. } => values
                .get(layout.slot(name, interns)?)
                .filter(|v| !matches!(v, Value::Undefined)),
            Self::Dict(dict) => dict.get_by_str(name.as_str(interns), heap, interns),
        }
    }

    pub fn set_slot(&mut self, slot: usize, value: Value) -> Option<Value> {
        let Self::Slots { values, order } = self else {
            unreachable!()
        };
        if values.len() <= slot {
            values.resize_with(slot + 1, || Value::Undefined);
        }
        let previous = mem::replace(&mut values[slot], value);
        if matches!(previous, Value::Undefined) {
            order.push(u8::try_from(slot).expect("shared layout has at most 32 slots"));
            None
        } else {
            Some(previous)
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Slots { order, .. } => order.len(),
            Self::Dict(dict) => dict.len(),
        }
    }

    pub fn iter<'a>(&'a self, layout: &'a InstanceLayout) -> impl Iterator<Item = (AttrKey<'a>, &'a Value)> {
        let (slots, dict) = match self {
            Self::Slots { values, order } => (Some((values, order)), None),
            Self::Dict(dict) => (None, Some(dict)),
        };
        slots
            .into_iter()
            .flat_map(move |(values, order)| {
                order.iter().map(move |slot| {
                    let slot = usize::from(*slot);
                    (AttrKey::Interned(layout.names[slot]), &values[slot])
                })
            })
            .chain(
                dict.into_iter()
                    .flat_map(Dict::iter)
                    .map(|(key, value)| (AttrKey::Borrowed(key), value)),
            )
    }

    pub fn for_each_ref(&self, mut visit: impl FnMut(HeapId)) {
        match self {
            Self::Slots { values, .. } => {
                for value in values {
                    if let Value::Ref(id) = value {
                        visit(*id);
                    }
                }
            }
            Self::Dict(dict) => {
                for (key, value) in dict {
                    for value in [key, value] {
                        if let Value::Ref(id) = value {
                            visit(*id);
                        }
                    }
                }
            }
        }
    }
}

impl HeapItem for InstanceAttrs {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Slots { values, .. } => {
                for value in values {
                    value.py_dec_ref_ids(stack);
                }
            }
            Self::Dict(dict) => dict.py_dec_ref_ids(stack),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for InstanceAttrs {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Slots { values, .. } => values.drop_with(ctx),
            Self::Dict(dict) => dict.drop_with(ctx),
        }
    }
}

pub(crate) fn class_layout(class: HeapId, heap: &Heap) -> &InstanceLayout {
    let HeapData::Class(class) = heap.get(class) else {
        unreachable!("instance owns a class")
    };
    class.instance_layout()
}

/// Slot keys are stored once as IDs; dictionary keys remain borrowed until copied or exported.
pub(crate) enum AttrKey<'a> {
    Interned(StringId),
    Borrowed(&'a Value),
}

impl AttrKey<'_> {
    pub fn clone_with_heap(&self, heap: &Heap) -> Value {
        match self {
            Self::Interned(id) => Value::InternString(*id),
            Self::Borrowed(value) => value.clone_with_heap(heap),
        }
    }
}

/// The dictionary cursor starts at the same assignment position if a copy hook converts the source.
pub(crate) struct InstanceAttrCursor {
    pub table: super::hash_table::TableCursor,
    pub index: usize,
    pub expected_len: usize,
    pub dictionary: bool,
}

impl InstanceAttrCursor {
    pub fn new(expected_len: usize) -> Self {
        Self {
            table: super::hash_table::TableCursor::new(expected_len),
            index: 0,
            expected_len,
            dictionary: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_values_spill_on_the_fifth_slot() {
        let mut attrs = InstanceAttrs::default();
        for slot in 0..4 {
            assert!(attrs.set_slot(slot, Value::Int(i64::try_from(slot).unwrap())).is_none());
        }
        let InstanceAttrs::Slots { values, .. } = &attrs else {
            panic!()
        };
        assert!(!values.spilled());

        assert!(attrs.set_slot(7, Value::Int(70)).is_none());
        let InstanceAttrs::Slots { values, order } = &attrs else {
            panic!()
        };
        assert!(values.spilled());
        assert!(matches!(values[4], Value::Undefined));
        assert_eq!(order.as_slice(), &[0, 1, 2, 3, 7]);
        assert!(matches!(attrs.set_slot(7, Value::Int(71)), Some(Value::Int(70))));
        assert_eq!(attrs.len(), 5);
    }

    #[test]
    fn layout_indices_survive_restore_and_dictionary_mode() {
        let mut layout = InstanceLayout::default();
        for index in 0..32 {
            assert_eq!(
                layout.admit(Some(StringId::from_index(index))),
                Some(usize::from(index))
            );
        }
        let bytes = serde_json::to_vec(&layout).unwrap();
        let mut restored: InstanceLayout = serde_json::from_slice(&bytes).unwrap();
        assert!(!restored.index.is_empty());
        for index in 0..32 {
            assert_eq!(restored.find(StringId::from_index(index)), Some(usize::from(index)));
        }
        assert_eq!(restored.admit(Some(StringId::from_index(32))), None);
        assert!(restored.dictionary());
        assert_eq!(restored.find(StringId::from_index(7)), Some(7));
        assert_eq!(restored.admit(Some(StringId::from_index(7))), None);
    }
}
