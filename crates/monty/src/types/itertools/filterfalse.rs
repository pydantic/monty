//! `itertools.filterfalse(predicate, iterable)` — the items a predicate rejects.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{HeapId, HeapRead},
    predicate::call_predicate,
    types::{
        PyTrait,
        itertools::{ItertoolsIter, step::next_tested},
    },
    value::Value,
};

/// Yields the items of `source` for which `predicate` is false.
///
/// No spent flag: unlike the `while` adaptors this never stops early, so the
/// source's own exhaustion is the only end condition.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FilterFalse {
    /// `Value::None` selects the truth test, as `filter(None, ...)` does.
    predicate: Value,
    source: Value,
}

impl FilterFalse {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(predicate: Value, source: Value) -> Self {
        Self { predicate, source }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.predicate {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.predicate.py_dec_ref_ids(stack);
        self.source.py_dec_ref_ids(stack);
    }
}

/// Pulls items until one the predicate rejects, which is the one yielded.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let mut steps = 0usize;
    loop {
        // Native loop: the VM's dispatch checkpoint is per-`run()`, so a
        // discarding pass over an infinite source reaches none. Poll the
        // tracker so `max_duration` still bites (see `VM::run`'s
        // `CHECK_INTERVAL`).
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let ItertoolsIter::FilterFalse(filter) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::FilterFalse")
        };
        let predicate = filter.predicate.clone_with_heap(vm.heap);
        let source = filter.source.clone_with_heap(vm.heap);

        let Some((item, truthy)) = next_tested(predicate, source, vm, |predicate, item, vm| {
            if matches!(predicate, Value::None) {
                item.py_bool(vm)
            } else {
                call_predicate(predicate, item, "filterfalse()", vm)
            }
        })?
        else {
            return Ok(None);
        };
        if !truthy {
            return Ok(Some(item));
        }
        // Accepted by the predicate, so this one is filtered out.
        item.drop_with(vm);
    }
}
