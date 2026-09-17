//! `itertools.zip_longest(*iterables, fillvalue=None)` — zip to the longest.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, DropWithContext, HeapId, HeapRead},
    types::{
        TupleVec, allocate_tuple,
        itertools::{ItertoolsIter, step::next_source},
    },
    value::Value,
};

/// Yields one tuple per round, padding sources that have run out.
///
/// A spent source is released and its slot goes `None`, so it is never driven
/// again — CPython swaps in a `repeat(fillvalue)` for the same effect. `active`
/// counts the live slots, so exhaustion is O(1) to detect and `zip_longest()`
/// with no arguments at all stops immediately.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ZipLongest {
    /// One slot per argument; `None` once that source is spent.
    sources: Vec<Option<Value>>,
    fillvalue: Value,
    /// How many slots are still live.
    active: usize,
}

impl ZipLongest {
    /// Takes the sources already resolved by `py_iter`, unlike `chain`'s lazy
    /// arguments: CPython resolves every `zip_longest` argument up front.
    pub(crate) fn new(sources: Vec<Value>, fillvalue: Value) -> Self {
        Self {
            active: sources.len(),
            sources: sources.into_iter().map(Some).collect(),
            fillvalue,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for source in self.sources.iter().flatten() {
            if let Value::Ref(id) = source {
                on_child(*id);
            }
        }
        if let Value::Ref(id) = &self.fillvalue {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for source in self.sources.iter_mut().flatten() {
            source.py_dec_ref_ids(stack);
        }
        self.fillvalue.py_dec_ref_ids(stack);
    }
}

/// Draws one item per slot, padding the spent ones, and stops once none are live.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::ZipLongest(zip) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::ZipLongest")
    };
    if zip.active == 0 {
        return Ok(None);
    }
    let slots = zip.sources.len();
    let fillvalue = zip.fillvalue.clone_with_heap(vm.heap);
    let mut active = zip.active;
    defer_drop!(fillvalue, vm);

    let mut row_guard = DropGuard::new(Vec::with_capacity(slots), vm);
    let (row, vm) = row_guard.as_parts_mut();
    for slot in 0..slots {
        // Re-projected every round: `py_next` re-enters the VM, so no borrow of
        // the adaptor can be held across it.
        let ItertoolsIter::ZipLongest(zip) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::ZipLongest")
        };
        let source = zip.sources[slot].as_ref().map(|source| source.clone_with_heap(vm.heap));
        let item = match source {
            None => None,
            Some(source) => {
                defer_drop!(source, vm);
                next_source(source, vm)?
            }
        };
        if let Some(item) = item {
            row.push(item);
        } else {
            // Newly spent: release the source and pad this slot from now on.
            let ItertoolsIter::ZipLongest(zip) = iter.get_mut(vm.heap) else {
                unreachable!("dispatched on Kind::ZipLongest")
            };
            if let Some(spent) = zip.sources[slot].take() {
                zip.active -= 1;
                active -= 1;
                spent.drop_with(vm);
            }
            row.push(fillvalue.clone_with_heap(vm.heap));
        }
    }

    let (row, vm) = row_guard.into_parts();
    if active == 0 {
        // Every source ran out this round, so the all-padding row is discarded
        // rather than yielded.
        row.drop_with(vm);
        Ok(None)
    } else {
        Ok(Some(allocate_tuple(TupleVec::from_vec(row), vm.heap)))
    }
}
