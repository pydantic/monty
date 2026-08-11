//! `itertools.chain(*iterables)` — the arguments' items, back to back.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropWithContext, HeapId, HeapRead},
    types::itertools::ItertoolsIter,
    value::Value,
};

/// Yields every item of each argument in turn.
///
/// `sources` holds the arguments UNRESOLVED: CPython calls `iter()` on each only
/// as it reaches it, so `chain([1], 5)` constructs cleanly and raises
/// `TypeError` part-way through consumption.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Chain {
    /// The arguments, still as passed; resolved one at a time by `next`.
    sources: Vec<Value>,
    started: usize,
    /// The resolved iterator currently being drained.
    current: Option<Value>,
    done: bool,
}

impl Chain {
    /// Takes the arguments unresolved — see the type docs for why.
    pub(crate) fn new(sources: Vec<Value>) -> Self {
        Self {
            sources,
            started: 0,
            current: None,
            done: false,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for source in &self.sources {
            if let Value::Ref(id) = source {
                on_child(*id);
            }
        }
        if let Some(Value::Ref(id)) = &self.current {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for source in &mut self.sources {
            source.py_dec_ref_ids(stack);
        }
        if let Some(current) = &mut self.current {
            current.py_dec_ref_ids(stack);
        }
    }
}

/// Drains the current source, then resolves the next one, until all are spent.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        let ItertoolsIter::Chain(chain) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Chain")
        };
        if chain.done {
            return Ok(None);
        }

        let Some(current) = chain.current.as_ref().map(|c| c.clone_with_heap(vm.heap)) else {
            // No live source: resolve the next argument, or finish.
            let Some(raw) = chain.sources.get(chain.started).map(|s| s.clone_with_heap(vm.heap)) else {
                let ItertoolsIter::Chain(chain) = iter.get_mut(vm.heap) else {
                    unreachable!("dispatched on Kind::Chain")
                };
                chain.done = true;
                return Ok(None);
            };
            // `into_py_iter` consumes `raw` on both paths, and raises here for a
            // non-iterable argument — matching CPython's lazy rejection.
            let resolved = into_py_iter_tracking(iter, raw, vm)?;
            let ItertoolsIter::Chain(chain) = iter.get_mut(vm.heap) else {
                unreachable!("dispatched on Kind::Chain")
            };
            chain.current = Some(resolved);
            continue;
        };

        defer_drop!(current, vm);
        let item = {
            let mut read = current.read(vm);
            read.py_next(vm)
        };

        if let Some(item) = item? {
            return Ok(Some(item));
        }
        // This source is spent; release it and move to the next argument.
        let ItertoolsIter::Chain(chain) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Chain")
        };
        chain.current.take().drop_with(vm);
    }
}

/// Resolves one argument to an iterator, marking it started first so a
/// `TypeError` from a non-iterable does not leave it to be retried.
fn into_py_iter_tracking<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, raw: Value, vm: &mut VM<'h>) -> RunResult<Value> {
    let ItertoolsIter::Chain(chain) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Chain")
    };
    chain.started += 1;
    raw.into_py_iter(vm)
}
