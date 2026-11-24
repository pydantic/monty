//! Keep the legacy dump layout: an idle REPL stores its execution state in
//! `repl`, and a suspended REPL stores it in `vm_state`.

use std::sync::Arc;

use ahash::AHashMap;
use monty_types::SourceRange;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};

use super::{CompileOptions, Executor, Heap, Interns, MontyRepl, NameMap, OsPolicy, ReplResolveFutures, ReplSnapshot};
use crate::{
    bytecode::{ExecutionState, ModuleCache, SnapshotDumpRef, SuspendedExecution, VMSnapshot},
    types::SessionRandom,
    value::Value,
};

/// The idle session or the session metadata of a suspended snippet.
#[derive(Deserialize)]
#[serde(rename = "MontyRepl")]
pub(super) struct DumpedRepl {
    script_name: Arc<str>,
    next_input_id: u64,
    global_names: NameMap,
    interns: Interns,
    sources: AHashMap<String, Arc<str>>,
    options: CompileOptions,
    os_policy: Arc<OsPolicy>,
    #[serde(default)]
    legacy_binary_writes: bool,
    cwd: Arc<str>,
    random: SessionRandom,
    #[serde(default)]
    modules: ModuleCache,
    #[serde(default)]
    future_annotations: bool,
    heap: Heap,
    globals: Vec<Value>,
}

impl From<DumpedRepl> for MontyRepl {
    fn from(dump: DumpedRepl) -> Self {
        Self {
            script_name: dump.script_name,
            next_input_id: dump.next_input_id,
            global_names: dump.global_names,
            interns: dump.interns,
            sources: dump.sources,
            options: dump.options,
            os_policy: dump.os_policy,
            legacy_binary_writes: dump.legacy_binary_writes,
            future_annotations: dump.future_annotations,
            heap: dump.heap,
            state: ExecutionState {
                cwd: dump.cwd,
                globals: dump.globals,
                random: dump.random,
                modules: dump.modules,
            },
        }
    }
}

impl Serialize for MontyRepl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ReplDumpRef {
            repl: self,
            snippet: None,
        }
        .serialize(serializer)
    }
}

/// Asks for the legacy placeholders instead of the live state when the
/// accompanying `vm_state` already serializes the latter.
struct ReplDumpRef<'a> {
    repl: &'a MontyRepl,
    snippet: Option<&'a Executor>,
}

impl Serialize for ReplDumpRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let repl = self.repl;
        let placeholder;
        let state = if let Some(snippet) = self.snippet {
            placeholder = ExecutionState::new(Vec::new(), Arc::clone(&snippet.program.cwd));
            &placeholder
        } else {
            &repl.state
        };
        let mut map = serializer.serialize_struct("MontyRepl", 14)?;
        map.serialize_field("script_name", &repl.script_name)?;
        map.serialize_field("next_input_id", &repl.next_input_id)?;
        map.serialize_field("global_names", &repl.global_names)?;
        map.serialize_field("interns", &repl.interns)?;
        map.serialize_field("sources", &repl.sources)?;
        map.serialize_field("options", &repl.options)?;
        map.serialize_field("os_policy", &repl.os_policy)?;
        map.serialize_field("legacy_binary_writes", &repl.legacy_binary_writes)?;
        map.serialize_field("cwd", &state.cwd)?;
        map.serialize_field("random", &state.random)?;
        map.serialize_field("modules", &state.modules)?;
        map.serialize_field("future_annotations", &repl.future_annotations)?;
        map.serialize_field("heap", &repl.heap)?;
        map.serialize_field("globals", &state.globals)?;
        map.end()
    }
}

/// Shared decoding of a suspended snippet's session and in-flight state.
fn restore_session(mut repl: MontyRepl, vm_state: SuspendedExecution) -> (MontyRepl, VMSnapshot) {
    repl.state = vm_state.state;
    (repl, vm_state.snapshot)
}

/// Legacy map for a REPL suspended on one host call or name lookup.
#[derive(Deserialize, Serialize)]
#[serde(rename = "ReplSnapshot")]
pub(super) struct DumpedReplSnapshot<R, E, V> {
    repl: R,
    executor: E,
    vm_state: V,
}

impl Serialize for ReplSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        DumpedReplSnapshot {
            repl: ReplDumpRef {
                repl: &self.repl,
                snippet: Some(&self.executor),
            },
            executor: &self.executor,
            vm_state: SnapshotDumpRef::new(&self.repl.state, &self.vm_state),
        }
        .serialize(serializer)
    }
}

impl From<DumpedReplSnapshot<MontyRepl, Executor, SuspendedExecution>> for ReplSnapshot {
    fn from(dump: DumpedReplSnapshot<MontyRepl, Executor, SuspendedExecution>) -> Self {
        let (repl, vm_state) = restore_session(dump.repl, dump.vm_state);
        Self {
            repl,
            executor: dump.executor,
            vm_state,
        }
    }
}

/// Legacy map for a REPL suspended on unresolved futures.
#[derive(Deserialize, Serialize)]
#[serde(rename = "ReplResolveFutures")]
pub(super) struct DumpedReplFutures<R, E, V, C, P> {
    repl: R,
    executor: E,
    vm_state: V,
    pending_call_ids: C,
    position: P,
}

impl Serialize for ReplResolveFutures {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        DumpedReplFutures {
            repl: ReplDumpRef {
                repl: &self.repl,
                snippet: Some(&self.executor),
            },
            executor: &self.executor,
            vm_state: SnapshotDumpRef::new(&self.repl.state, &self.vm_state),
            pending_call_ids: &self.pending_call_ids,
            position: &self.position,
        }
        .serialize(serializer)
    }
}

impl From<DumpedReplFutures<MontyRepl, Executor, SuspendedExecution, Vec<u32>, SourceRange>> for ReplResolveFutures {
    fn from(dump: DumpedReplFutures<MontyRepl, Executor, SuspendedExecution, Vec<u32>, SourceRange>) -> Self {
        let (repl, vm_state) = restore_session(dump.repl, dump.vm_state);
        Self {
            repl,
            executor: dump.executor,
            vm_state,
            pending_call_ids: dump.pending_call_ids,
            position: dump.position,
        }
    }
}
