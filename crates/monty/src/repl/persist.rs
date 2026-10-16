//! Preserves REPL struct shapes while decoding every root before owner-local relocation.

use std::sync::Arc;

use ahash::AHashMap;
use monty_types::{CompileOptions, OsPolicy, SourceRange};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::SerializeStruct};

use super::{MontyRepl, ReplResolveFutures, ReplSnapshot};
use crate::{
    bytecode::VMSnapshot,
    heap::{HeapIndex, HeapOwner, HeapWire},
    intern::Interns,
    name_map::NameMap,
    run::Executor,
    types::SessionRandom,
    value::Value,
};

impl Serialize for MontyRepl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.owner.inspect(|state| {
            let mut fields = serializer.serialize_struct("MontyRepl", 11)?;
            fields.serialize_field("script_name", &self.script_name)?;
            fields.serialize_field("next_input_id", &self.next_input_id)?;
            fields.serialize_field("global_names", &self.global_names)?;
            fields.serialize_field("interns", &self.interns)?;
            fields.serialize_field("sources", &self.sources)?;
            fields.serialize_field("options", &self.options)?;
            fields.serialize_field("os_policy", &self.os_policy)?;
            fields.serialize_field("cwd", &self.cwd)?;
            fields.serialize_field("random", &self.random)?;
            fields.serialize_field("heap", &state.heap)?;
            fields.serialize_field("globals", &state.globals)?;
            fields.end()
        })
    }
}

impl<'de> Deserialize<'de> for MontyRepl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ReplWire::deserialize(deserializer)?
            .finish(None)
            .map_err(D::Error::custom)
    }
}

impl Serialize for ReplSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.repl.owner.inspect(|state| {
            let mut fields = serializer.serialize_struct("ReplSnapshot", 3)?;
            fields.serialize_field("repl", &self.repl)?;
            fields.serialize_field("executor", &self.executor)?;
            fields.serialize_field("vm_state", state.vm_state.as_ref().expect("suspended VM"))?;
            fields.end()
        })
    }
}

impl<'de> Deserialize<'de> for ReplSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename = "ReplSnapshot")]
        struct Wire {
            repl: ReplWire,
            executor: Executor,
            vm_state: VMSnapshot<HeapIndex>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            repl: wire.repl.finish(Some(wire.vm_state)).map_err(D::Error::custom)?,
            executor: wire.executor,
        })
    }
}

impl Serialize for ReplResolveFutures {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.repl.owner.inspect(|state| {
            let mut fields = serializer.serialize_struct("ReplResolveFutures", 5)?;
            fields.serialize_field("repl", &self.repl)?;
            fields.serialize_field("executor", &self.executor)?;
            fields.serialize_field("vm_state", state.vm_state.as_ref().expect("suspended VM"))?;
            fields.serialize_field("pending_call_ids", &self.pending_call_ids)?;
            fields.serialize_field("position", &self.position)?;
            fields.end()
        })
    }
}

impl<'de> Deserialize<'de> for ReplResolveFutures {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename = "ReplResolveFutures")]
        struct Wire {
            repl: ReplWire,
            executor: Executor,
            vm_state: VMSnapshot<HeapIndex>,
            pending_call_ids: Vec<u32>,
            position: SourceRange,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            repl: wire.repl.finish(Some(wire.vm_state)).map_err(D::Error::custom)?,
            executor: wire.executor,
            pending_call_ids: wire.pending_call_ids,
            position: wire.position,
        })
    }
}

/// Keeps references numeric until any surrounding suspended VM has also been decoded.
#[derive(Deserialize)]
#[serde(rename = "MontyRepl")]
struct ReplWire {
    script_name: Arc<str>,
    next_input_id: u64,
    global_names: NameMap,
    interns: Interns,
    sources: AHashMap<String, Arc<str>>,
    options: CompileOptions,
    os_policy: Arc<OsPolicy>,
    cwd: Arc<str>,
    random: SessionRandom,
    heap: HeapWire,
    globals: Vec<Value<HeapIndex>>,
}

impl ReplWire {
    /// Reconstructs idle and suspended roots together, independent of serialized field order.
    fn finish(self, snapshot: Option<VMSnapshot<HeapIndex>>) -> Result<MontyRepl, &'static str> {
        Ok(MontyRepl {
            script_name: self.script_name,
            next_input_id: self.next_input_id,
            global_names: self.global_names,
            interns: self.interns,
            sources: self.sources,
            options: self.options,
            os_policy: self.os_policy,
            cwd: self.cwd,
            random: self.random,
            owner: HeapOwner::from_wire(self.heap, self.globals, snapshot)?,
        })
    }
}
