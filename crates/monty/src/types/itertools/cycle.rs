//! `itertools.cycle(iterable)` — the source's items, repeating forever.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropWithContext, HeapId, HeapRead},
    types::itertools::ItertoolsIter,
    value::{VALUE_SIZE, Value},
};

/// Yields the source's items, then replays the saved copy indefinitely.
///
/// Buffering is unavoidable — a spent iterator cannot be rewound — so a `cycle`
/// over a long source holds every item for as long as it lives.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Cycle {
    /// The iterator being drained on the first pass; `None` once spent.
    source: Option<Value>,
    /// Every item seen so far, replayed once `source` runs out.
    saved: Vec<Value>,
    index: usize,
}

impl Cycle {
    pub(crate) fn new(source: Value) -> Self {
        Self {
            source: Some(source),
            saved: Vec::new(),
            index: 0,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.source {
            on_child(*id);
        }
        for item in &self.saved {
            if let Value::Ref(id) = item {
                on_child(*id);
            }
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(source) = &mut self.source {
            source.py_dec_ref_ids(stack);
        }
        for item in &mut self.saved {
            item.py_dec_ref_ids(stack);
        }
    }

    /// Bytes held in `saved`, on top of the inline struct.
    ///
    /// Charged slot-by-slot as the buffer grows (see [`next`]), so it MUST be
    /// reported here too: `py_estimate_size` is what `on_free` releases, and a
    /// flat estimate would return less than was taken.
    pub(crate) fn buffered_size(&self) -> usize {
        self.saved.len() * VALUE_SIZE
    }
}

/// Yields from the source while it lasts, saving each item, then replays.
///
/// Returns `None` only for an empty source: `cycle([])` is empty rather than an
/// infinite loop, matching CPython.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Cycle(cycle) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Cycle")
    };

    if let Some(source) = cycle.source.as_ref().map(|s| s.clone_with_heap(vm.heap)) {
        defer_drop!(source, vm);
        let item = {
            let mut read = source.read(vm);
            read.py_next(vm)
        };
        if let Some(item) = item? {
            // `saved` grows once per item with no bound of its own, so the slot
            // is charged with the tracker BEFORE it is taken — otherwise a long
            // `cycle` grows the buffer entirely outside `max_memory`. Charged
            // before the clone so a rejection leaves nothing to release.
            if let Err(error) = vm.heap.track_growth(VALUE_SIZE) {
                item.drop_with(vm);
                return Err(error.into());
            }
            // Saved AND yielded, so the buffer holds its own reference.
            let retained = item.clone_with_heap(vm.heap);
            let ItertoolsIter::Cycle(cycle) = iter.get_mut(vm.heap) else {
                unreachable!("dispatched on Kind::Cycle")
            };
            cycle.saved.push(retained);
            return Ok(Some(item));
        }
        // First pass over; drop the source and fall through to replay.
        let ItertoolsIter::Cycle(cycle) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Cycle")
        };
        cycle.source.take().drop_with(vm);
    }

    let ItertoolsIter::Cycle(cycle) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Cycle")
    };
    if cycle.saved.is_empty() {
        return Ok(None);
    }
    let item = cycle.saved[cycle.index].clone_with_heap(vm.heap);
    let ItertoolsIter::Cycle(cycle) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Cycle")
    };
    cycle.index = (cycle.index + 1) % cycle.saved.len();
    Ok(Some(item))
}
