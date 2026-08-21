//! `itertools.takewhile(predicate, iterable)` — the leading run that passes.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{DropWithContext, HeapId, HeapRead},
    predicate::call_predicate,
    types::itertools::{ItertoolsIter, step::next_tested},
    value::Value,
};

/// Yields items from `source` until `predicate` first returns false.
///
/// Both fields go `None` together at that point, which latches the adaptor as
/// spent and releases them there and then: CPython stops calling the predicate
/// — and stops touching the source — once it has failed, so the rejected item
/// is the last thing either value ever sees. A source that merely runs out is
/// not a rejection and does not latch, so both stay `Some`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TakeWhile {
    predicate: Option<Value>,
    source: Option<Value>,
}

impl TakeWhile {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(predicate: Value, source: Value) -> Self {
        Self {
            predicate: Some(predicate),
            source: Some(source),
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.predicate {
            on_child(*id);
        }
        if let Some(Value::Ref(id)) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(predicate) = &mut self.predicate {
            predicate.py_dec_ref_ids(stack);
        }
        if let Some(source) = &mut self.source {
            source.py_dec_ref_ids(stack);
        }
    }
}

/// Pulls one item and yields it if the predicate holds, latching on the first
/// rejection so nothing after it is examined.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::TakeWhile(take) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::TakeWhile")
    };
    let (Some(predicate), Some(source)) = (&take.predicate, &take.source) else {
        return Ok(None);
    };
    let predicate = predicate.clone_with_heap(vm.heap);
    let source = source.clone_with_heap(vm.heap);

    // Exhaustion does NOT latch: CPython only sets `stop` when the predicate
    // fails, so a source that raises `StopIteration` and later yields again is
    // re-driven on the next call rather than treated as spent.
    let Some((item, accepted)) = next_tested(predicate, source, vm, |predicate, item, vm| {
        call_predicate(predicate, item, "takewhile()", vm)
    })?
    else {
        return Ok(None);
    };

    if accepted {
        Ok(Some(item))
    } else {
        // The rejected item is discarded rather than yielded, and the latch
        // means nothing after it is ever examined.
        item.drop_with(vm);
        finish(iter, vm);
        Ok(None)
    }
}

/// Latches the adaptor as spent, so neither the source nor the predicate is
/// reached again.
///
/// Releasing both here rather than at destruction lets whatever the source
/// holds (a generator's frame, a file) be reclaimed as soon as the run ends,
/// matching what `pairwise` and `islice` do when they stop.
fn finish<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) {
    let ItertoolsIter::TakeWhile(take) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::TakeWhile")
    };
    // Both taken under the one borrow, then dropped.
    let (predicate, source) = (take.predicate.take(), take.source.take());
    predicate.drop_with(vm);
    source.drop_with(vm);
}
