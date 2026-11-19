use std::{mem, sync::Arc};

use super::ModuleCache;
use crate::{
    heap::{ContainsHeap, DropWithContext},
    types::SessionRandom,
    value::Value,
};

/// State shared by all VM turns of a run or REPL session.
#[derive(Debug)]
pub(crate) struct ExecutionState {
    /// Global values indexed by the session's name slots.
    pub(crate) globals: Vec<Value>,
    /// Virtual working directory, including changes made by `os.chdir`.
    pub(crate) cwd: Arc<str>,
    /// The module generator and state for initializing unseeded generators.
    pub(crate) random: SessionRandom,
    /// Imported file modules, including any whose body is still executing.
    pub(crate) modules: ModuleCache,
}

impl ExecutionState {
    /// Initializes a run or session before its first turn.
    pub(crate) fn new(globals: Vec<Value>, cwd: Arc<str>) -> Self {
        Self {
            globals,
            cwd,
            random: SessionRandom::default(),
            modules: ModuleCache::default(),
        }
    }

    /// Releases the session's owned heap references before its heap is dropped.
    pub(crate) fn clear_heap_values(&mut self, heap: &mut impl ContainsHeap) {
        mem::take(&mut self.globals).drop_with(heap);
        mem::take(&mut self.modules).drop_with(heap);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for ExecutionState {
    fn drop_with(mut self, ctx: &mut C) {
        self.clear_heap_values(ctx);
    }
}
