//! Preserves standalone snapshot structs without exposing separately deserializable arenas.

use monty_types::SourceRange;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::SerializeStruct};

use super::{ResolveFutures, Snapshot};
use crate::{
    bytecode::VMSnapshot,
    heap::{HeapIndex, HeapOwner, HeapWire},
    run::Executor,
};

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.owner.inspect(|state| {
            let mut fields = serializer.serialize_struct("Snapshot", 3)?;
            fields.serialize_field("executor", &self.executor)?;
            fields.serialize_field("vm_state", state.vm_state.as_ref().expect("suspended VM"))?;
            fields.serialize_field("heap", &state.heap)?;
            fields.end()
        })
    }
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Numeric roots may precede their heap in both map and sequence formats.
        #[derive(Deserialize)]
        #[serde(rename = "Snapshot")]
        struct Wire {
            executor: Executor,
            vm_state: VMSnapshot<HeapIndex>,
            heap: HeapWire,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            executor: wire.executor,
            owner: HeapOwner::from_wire(wire.heap, Vec::new(), Some(wire.vm_state)).map_err(D::Error::custom)?,
        })
    }
}

impl Serialize for ResolveFutures {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.owner.inspect(|state| {
            let mut fields = serializer.serialize_struct("ResolveFutures", 5)?;
            fields.serialize_field("executor", &self.executor)?;
            fields.serialize_field("vm_state", state.vm_state.as_ref().expect("suspended VM"))?;
            fields.serialize_field("heap", &state.heap)?;
            fields.serialize_field("pending_call_ids", &self.pending_call_ids)?;
            fields.serialize_field("position", &self.position)?;
            fields.end()
        })
    }
}

impl<'de> Deserialize<'de> for ResolveFutures {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Scheduler metadata stays outside the owner; its Python roots do not.
        #[derive(Deserialize)]
        #[serde(rename = "ResolveFutures")]
        struct Wire {
            executor: Executor,
            vm_state: VMSnapshot<HeapIndex>,
            heap: HeapWire,
            pending_call_ids: Vec<u32>,
            position: SourceRange,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            executor: wire.executor,
            owner: HeapOwner::from_wire(wire.heap, Vec::new(), Some(wire.vm_state)).map_err(D::Error::custom)?,
            pending_call_ids: wire.pending_call_ids,
            position: wire.position,
        })
    }
}
