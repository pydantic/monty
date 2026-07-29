//! `itertools.islice(iterable, [start,] stop[, step])` — a slice of an iterator.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{HeapId, HeapRead},
    types::itertools::ItertoolsIter,
    value::Value,
};

/// Yields the items of `source` at positions `start, start+step, …` below `stop`.
///
/// Mirrors CPython's `isliceobject`: skipped items are discarded as they are
/// pulled rather than buffered.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Islice {
    source: Value,
    /// How many items have been pulled from `source` so far.
    consumed: usize,
    /// Position, in source order, of the item to yield next.
    next_index: usize,
    /// Exclusive upper bound, or `None` when `stop` was `None`.
    stop: Option<usize>,
    /// Positions between yields; always at least 1.
    step: usize,
    /// Latches once the source or `stop` is reached.
    done: bool,
}

impl Islice {
    /// `start` seeds `next_index` so leading items are skipped on the first
    /// step: CPython does not touch the iterator until the first `next()`.
    pub(crate) fn new(source: Value, start: usize, stop: Option<usize>, step: usize) -> Self {
        Self {
            source,
            consumed: 0,
            next_index: start,
            stop,
            step,
            done: false,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the ref this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.source.py_dec_ref_ids(stack);
    }
}

/// Discards items up to `next_index`, yields the one there, then advances by
/// `step` — the shape of CPython's `islice_next`.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        let ItertoolsIter::Islice(islice) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Islice")
        };
        if islice.done {
            return Ok(None);
        }
        // Reaching `stop` spends the adaptor without pulling again, so a
        // bounded slice never over-consumes its source.
        if islice.stop.is_some_and(|stop| islice.consumed >= stop) {
            finish(iter, vm);
            return Ok(None);
        }
        let skipping = islice.consumed < islice.next_index;
        let source = islice.source.clone_with_heap(vm.heap);
        defer_drop!(source, vm);

        let item = {
            let mut read = source.read(vm);
            read.py_next(vm)
        };
        let Some(item) = item? else {
            finish(iter, vm);
            return Ok(None);
        };

        let ItertoolsIter::Islice(islice) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Islice")
        };
        islice.consumed += 1;
        if skipping {
            // Not at `next_index` yet — discard and try again.
            item.drop_with(vm);
            continue;
        }
        // `step` can push `next_index` past `stop`; the bound check above
        // handles that on the following call, so saturating is enough here.
        islice.next_index = islice.next_index.saturating_add(islice.step);
        return Ok(Some(item));
    }
}

/// Latches the adaptor as spent, so neither `stop` nor an exhausted source
/// drives it again.
fn finish<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) {
    let ItertoolsIter::Islice(islice) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Islice")
    };
    islice.done = true;
}
