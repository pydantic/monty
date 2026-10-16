//! `itertools.dropwhile(predicate, iterable)` — everything after the leading run.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{HeapRead, Reference, RuntimeRef},
    predicate::call_predicate,
    types::itertools::{
        ItertoolsIter,
        step::{next_item, next_tested},
    },
    value::Value,
};

#[derive(monty_macros::Relocate)]
/// Discards items while `predicate` holds, then yields the rest untested.
///
/// `dropping` clears on the first item the predicate rejects, and that item is
/// the first one yielded; the predicate is never consulted again but stays
/// owned until destruction, as CPython holds `lz->func` for the iterator's
/// whole life. The source stays owned too: unlike `takewhile` this adaptor
/// never latches, so every later `next` drives it again.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DropWhile<H: Reference> {
    predicate: Value<H>,
    source: Value<H>,
    /// Whether the leading run is still being discarded.
    dropping: bool,
}

impl<H: RuntimeRef> DropWhile<H> {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(predicate: Value<H>, source: Value<H>) -> Self {
        Self {
            predicate,
            source,
            dropping: true,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(H)) {
        if let Value::Ref(id) = &self.predicate {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<H>) {
        self.predicate.py_dec_ref_ids(stack);
        self.source.py_dec_ref_ids(stack);
    }
}

/// Skips items while the predicate holds; once it has failed, yields straight
/// through without consulting it again.
pub(super) fn next<'h, H: RuntimeRef>(
    iter: &mut HeapRead<'h, ItertoolsIter<H>>,
    vm: &mut VM<'h, H>,
) -> RunResult<Option<Value<H>>> {
    let mut steps = 0usize;
    loop {
        // Native loop: the VM's dispatch checkpoint is per-`run()`, so a
        // discarding pass over an infinite source reaches none. Poll the
        // tracker so the time limits still bite (see `VM::run`'s
        // `CHECK_INTERVAL`).
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let ItertoolsIter::DropWhile(drop_while) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::DropWhile")
        };
        let dropping = drop_while.dropping;
        let source = drop_while.source.clone_with_heap(vm.heap);
        // Past the leading run the predicate is never consulted again, so the
        // item is yielded untested and the predicate is not even cloned.
        if !dropping {
            return next_item(source, vm);
        }
        let predicate = drop_while.predicate.clone_with_heap(vm.heap);

        let Some((item, accepted)) = next_tested(predicate, source, vm, |predicate, item, vm| {
            call_predicate(predicate, item, "dropwhile()", vm)
        })?
        else {
            return Ok(None);
        };
        if !accepted {
            let ItertoolsIter::DropWhile(drop_while) = iter.get_mut(vm.heap) else {
                unreachable!("dispatched on Kind::DropWhile")
            };
            // Retained, not released: only the flag says it is done with.
            drop_while.dropping = false;
            return Ok(Some(item));
        }
        // Still in the leading run, so this item is discarded.
        item.drop_with(vm);
    }
}
