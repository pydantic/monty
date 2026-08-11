//! `itertools.compress(data, selectors)` — the data items whose selector is true.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, HeapId, HeapRead},
    types::{PyTrait, itertools::ItertoolsIter},
    value::Value,
};

/// Yields each `data` item whose parallel `selectors` item is truthy.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Compress {
    data: Value,
    /// Pulled in lockstep with `data`; its truthiness gates each item.
    selectors: Value,
    /// Latches once either side runs out, so neither is driven again.
    done: bool,
}

impl Compress {
    /// Takes ownership of both iterators, which the caller has already resolved
    /// with `py_iter`.
    pub(crate) fn new(data: Value, selectors: Value) -> Self {
        Self {
            data,
            selectors,
            done: false,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.data {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.selectors {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.data.py_dec_ref_ids(stack);
        self.selectors.py_dec_ref_ids(stack);
    }
}

/// Pulls one item from each side per step, yielding the datum when its selector
/// is truthy and stopping as soon as *either* side runs out.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        let ItertoolsIter::Compress(compress) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Compress")
        };
        if compress.done {
            return Ok(None);
        }
        let data = compress.data.clone_with_heap(vm.heap);
        let selectors = compress.selectors.clone_with_heap(vm.heap);
        // Guarded because a source that raises unwinds past both clones.
        defer_drop!(data, vm);
        defer_drop!(selectors, vm);

        let Some(datum) = step(iter, data, vm)? else {
            return Ok(None);
        };
        // `datum` outlives the selector step, which may raise, and is handed
        // back only when kept — hence a guard rather than `defer_drop!`.
        let mut datum_guard = DropGuard::new(datum, vm);
        let vm = datum_guard.ctx();
        let Some(selector) = step(iter, selectors, vm)? else {
            return Ok(None);
        };
        let keep = {
            defer_drop!(selector, vm);
            selector.py_bool(vm)?
        };
        if keep {
            let (datum, _) = datum_guard.into_parts();
            return Ok(Some(datum));
        }
    }
}

/// Advances one side, latching `done` when it runs out so a `None` outcome
/// means the whole adaptor is spent rather than just this step.
fn step<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, source: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let item = {
        let mut read = source.read(vm);
        read.py_next(vm)
    };
    if let Some(item) = item? {
        Ok(Some(item))
    } else {
        let ItertoolsIter::Compress(compress) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Compress")
        };
        compress.done = true;
        Ok(None)
    }
}
