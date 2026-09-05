//! `itertools.pairwise(iterable)` — successive overlapping pairs.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropWithContext, HeapId, HeapRead},
    types::{
        itertools::{ItertoolsIter, step::next_source},
        tuple::allocate_tuple,
    },
    value::Value,
};

/// Yields `(s0, s1), (s1, s2), …` from a source iterator.
///
/// `previous` keeps each item alive for the step that pairs it with its
/// successor, so it is a second owned ref alongside `source`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Pairwise {
    /// The iterator being paired up; driven twice on the first step. `None`
    /// once exhausted, which both latches the adaptor as spent and releases
    /// the source there and then, as CPython's `pairwise` does.
    source: Option<Value>,
    /// The left half of the next pair, or `None` before the first step.
    previous: Option<Value>,
}

impl Pairwise {
    /// Takes ownership of `source`, which the caller has already resolved with
    /// `py_iter` — `next` drives it directly and never re-resolves it.
    pub(crate) fn new(source: Value) -> Self {
        Self {
            source: Some(source),
            previous: None,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.source {
            on_child(*id);
        }
        if let Some(Value::Ref(id)) = &self.previous {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(source) = &mut self.source {
            source.py_dec_ref_ids(stack);
        }
        if let Some(previous) = &mut self.previous {
            previous.py_dec_ref_ids(stack);
        }
    }
}

/// Pulls one item and pairs it with the one before, priming `previous` on the
/// first step so a source of fewer than two items yields nothing.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Pairwise(pairwise) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Pairwise")
    };
    if pairwise.source.is_none() {
        return Ok(None);
    }
    // Prime `previous` on the first step. A source that stops immediately
    // leaves nothing to pair, so latch and yield nothing.
    if pairwise.previous.is_none() {
        let Some(first) = drive_source(iter, vm)? else {
            return Ok(None);
        };
        let ItertoolsIter::Pairwise(pairwise) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Pairwise")
        };
        pairwise.previous = Some(first);
    }

    let Some(second) = drive_source(iter, vm)? else {
        return Ok(None);
    };
    // Cloned before the `&mut`: `clone_with_heap` needs the heap shared, which
    // `get_mut` excludes. `second` is both yielded and retained.
    let retained = second.clone_with_heap(vm.heap);
    let ItertoolsIter::Pairwise(pairwise) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Pairwise")
    };
    let first = pairwise.previous.replace(retained).expect("previous was primed above");
    Ok(Some(allocate_tuple([first, second].into_iter().collect(), vm.heap)))
}

/// Advances the source, releasing it and `previous` once it stops.
fn drive_source<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let source = {
        let ItertoolsIter::Pairwise(pairwise) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Pairwise")
        };
        pairwise.source.as_ref().map(|source| source.clone_with_heap(vm.heap))
    };
    let Some(source) = source else {
        return Ok(None);
    };
    defer_drop!(source, vm);
    if let Some(item) = next_source(source, vm)? {
        Ok(Some(item))
    } else {
        let ItertoolsIter::Pairwise(pairwise) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Pairwise")
        };
        // Both taken under the one borrow, then dropped: releasing the source
        // here rather than at destruction lets whatever it holds (a generator's
        // frame, a file) be reclaimed as soon as the adaptor is spent.
        let (source, previous) = (pairwise.source.take(), pairwise.previous.take());
        source.drop_with(vm);
        previous.drop_with(vm);
        Ok(None)
    }
}
