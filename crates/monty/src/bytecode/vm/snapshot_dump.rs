//! The persisted `vm_state` map predates the split between turn-local and
//! session state. Keep the two together on disk, not in the live VM.

use std::sync::Arc;

use serde::{Serialize, Serializer, ser::SerializeStruct};

use super::{CaughtException, ExecutionState, ModuleCache, Scheduler, SerializedFrame, VMSnapshot};
use crate::{os_dispatch::PendingEffect, types::SessionRandom, value::Value};

/// Both halves of a suspended one-shot run, serialized as the legacy `vm_state` map.
#[derive(Debug, serde::Deserialize)]
#[serde(from = "DumpedExecution")]
pub(crate) struct SuspendedExecution {
    pub(crate) state: ExecutionState,
    pub(crate) snapshot: VMSnapshot,
}

impl Serialize for SuspendedExecution {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SnapshotDumpRef::new(&self.state, &self.snapshot).serialize(serializer)
    }
}

/// Borrowed serialization adapter shared by one-shot and REPL suspensions.
pub(crate) struct SnapshotDumpRef<'a> {
    state: &'a ExecutionState,
    snapshot: &'a VMSnapshot,
}

impl<'a> SnapshotDumpRef<'a> {
    /// Writes both halves without copying or taking ownership of heap values.
    pub(crate) fn new(state: &'a ExecutionState, snapshot: &'a VMSnapshot) -> Self {
        Self { state, snapshot }
    }
}

impl Serialize for SnapshotDumpRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_struct("VMSnapshot", 11)?;
        map.serialize_field("stack", &self.snapshot.stack)?;
        map.serialize_field("globals", &self.state.globals)?;
        map.serialize_field("frames", &self.snapshot.frames)?;
        map.serialize_field("exception_stack", &self.snapshot.exception_stack)?;
        map.serialize_field("instruction_ip", &self.snapshot.instruction_ip)?;
        map.serialize_field("scheduler", &self.snapshot.scheduler)?;
        map.serialize_field("pending_effect", &self.snapshot.pending_effect)?;
        map.serialize_field("pending_lookup_effect", &self.snapshot.pending_lookup_effect)?;
        map.serialize_field("cwd", &self.state.cwd)?;
        map.serialize_field("random", &self.state.random)?;
        map.serialize_field("modules", &self.state.modules)?;
        map.end()
    }
}

/// Owned legacy `vm_state` map, split into its live owners on load.
#[derive(serde::Deserialize)]
#[serde(rename = "VMSnapshot")]
struct DumpedExecution {
    stack: Vec<Value>,
    globals: Vec<Value>,
    frames: Vec<SerializedFrame>,
    exception_stack: Vec<CaughtException>,
    instruction_ip: usize,
    scheduler: Scheduler,
    pending_effect: Option<PendingEffect>,
    pending_lookup_effect: Option<super::PendingLookupEffect>,
    cwd: Arc<str>,
    random: SessionRandom,
    #[serde(default)]
    modules: ModuleCache,
}

impl From<DumpedExecution> for SuspendedExecution {
    fn from(dump: DumpedExecution) -> Self {
        Self {
            state: ExecutionState {
                globals: dump.globals,
                cwd: dump.cwd,
                random: dump.random,
                modules: dump.modules,
            },
            snapshot: VMSnapshot {
                stack: dump.stack,
                frames: dump.frames,
                exception_stack: dump.exception_stack,
                instruction_ip: dump.instruction_ip,
                scheduler: dump.scheduler,
                pending_effect: dump.pending_effect,
                pending_lookup_effect: dump.pending_lookup_effect,
            },
        }
    }
}
