//! The session's modules imported from `.py` files: CPython's `sys.modules`,
//! restricted to file modules.

use std::collections::BTreeMap;

use crate::{
    heap::{ContainsHeap, DropWithContext, HeapId},
    value::Value,
};

/// Modules imported from files this session, by fully-qualified dotted name.
///
/// Each file runs once: a later import of the name binds the cached module. A
/// module is cached before its body runs, so a circular import binds it partly
/// initialised, and dropped again if the body raises, so the next import retries.
/// Bundled and host modules are never cached. Every `HeapId` is OWNED.
// TODO: back a live `sys.modules` dict with this, and add `importlib.reload`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModuleCache(BTreeMap<String, CachedModule>);

/// A cached module, and whether its body is still running.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CachedModule {
    module: HeapId,
    initializing: bool,
}

impl ModuleCache {
    /// An owned reference to the module cached as `name`.
    pub fn get(&self, name: &str, heap: &impl ContainsHeap) -> Option<Value> {
        self.0.get(name).map(|cached| {
            heap.heap().inc_ref(cached.module);
            Value::Ref(cached.module)
        })
    }

    /// The module cached as `name`, borrowed.
    pub fn get_id(&self, name: &str) -> Option<HeapId> {
        self.0.get(name).map(|cached| cached.module)
    }

    /// Caches the borrowed `module` as `name` before its body runs, taking a reference to it.
    pub fn insert(&mut self, name: String, module: HeapId, heap: &mut impl ContainsHeap) {
        heap.heap().inc_ref(module);
        let cached = CachedModule {
            module,
            initializing: true,
        };
        if let Some(old) = self.0.insert(name, cached) {
            heap.heap_mut().dec_ref(old.module);
        }
    }

    /// Marks `name` initialised once `module`'s body has returned.
    pub fn finish(&mut self, name: &str, module: HeapId) {
        if let Some(cached) = self.0.get_mut(name)
            && cached.module == module
        {
            cached.initializing = false;
        }
    }

    /// Forgets `name` if it still caches `module`, whose body raised.
    pub fn remove_failed(&mut self, name: &str, module: HeapId, heap: &mut impl ContainsHeap) {
        if self.0.get(name).is_some_and(|cached| cached.module == module) {
            self.0.remove(name);
            heap.heap_mut().dec_ref(module);
        }
    }

    /// Forgets every module whose body never finished, when the execution
    /// running it is abandoned, so the next import runs the file again.
    pub fn remove_unfinished(&mut self, heap: &mut impl ContainsHeap) {
        self.0.retain(|_, cached| {
            if cached.initializing {
                heap.heap_mut().dec_ref(cached.module);
            }
            !cached.initializing
        });
    }
}

impl<C: ContainsHeap> DropWithContext<C> for ModuleCache {
    fn drop_with(self, ctx: &mut C) {
        for cached in self.0.into_values() {
            ctx.heap_mut().dec_ref(cached.module);
        }
    }
}
