//! `itertools.tee(iterable, n=2)` — independent iterators over one source.
//!
//! The two types here are one structure: every iterator `tee()` returns is a
//! [`Tee`] holding a slot in a shared [`TeeBuffer`], which owns the source and
//! the items read from it but not yet taken by every slot. An item is pulled
//! from the source once, by whichever slot reaches it first, and buffered until
//! the last one has passed it.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapObjectRead, HeapReadOutput, HeapReader},
    types::itertools::{ItertoolsIter, step::next_source},
    value::Value,
};

/// One consumer of a `tee()` group.
///
/// Holds no items of its own: its position lives in the buffer's `positions`,
/// so the buffer can tell how far behind the slowest consumer is without
/// reaching back into the iterators that hold it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Tee {
    /// The shared buffer — owned, so the group outlives the `tee()` call.
    buffer: Value,
    /// Which of the buffer's positions this consumer advances.
    slot: usize,
}

impl Tee {
    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.buffer {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.buffer.py_dec_ref_ids(stack);
    }
}

/// The read-ahead every `Tee` of one group draws from.
///
/// Boxed inside [`ItertoolsIter`]: four fields, two of them owning
/// collections, put it well past the family's size budget.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TeeBuffer {
    /// The iterator the items come from; `None` once it has run out, which is
    /// how a spent group stops driving it again.
    source: Option<Value>,
    /// Items read from the source and still owed to at least one slot.
    /// `items[0]` is the item at absolute index [`Self::base`].
    items: VecDeque<Value>,
    base: usize,
    /// The next absolute index each slot will read. A slot whose `Tee` has
    /// been released keeps its entry, so see `limitations/itertools.md` for
    /// what that costs.
    positions: Vec<usize>,
    /// Whether the source is being read right now, so a slot stepped from
    /// inside that read is refused rather than driving it again.
    running: bool,
}

impl TeeBuffer {
    /// Takes the resolved source and opens `consumers` slots, all at the start.
    pub(crate) fn new(source: Value, consumers: usize) -> Self {
        Self {
            source: Some(source),
            items: VecDeque::new(),
            base: 0,
            positions: vec![0; consumers],
            running: false,
        }
    }

    /// Invokes `on_child` for each heap id this buffer owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.source {
            on_child(*id);
        }
        for item in &self.items {
            if let Value::Ref(id) = item {
                on_child(*id);
            }
        }
    }

    /// Releases the refs this buffer owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(source) = &mut self.source {
            source.py_dec_ref_ids(stack);
        }
        for item in &mut self.items {
            item.py_dec_ref_ids(stack);
        }
    }

    /// Opens `consumers` more slots, each starting where `slot` has reached.
    ///
    /// This is what `tee()` over an existing `_tee` does: CPython copies the
    /// iterator rather than draining it, so the copies replay from where the
    /// original stands and advancing the original afterwards leaves them be.
    fn fork(&mut self, slot: usize, consumers: usize) -> Vec<usize> {
        let position = self.positions[slot];
        (0..consumers)
            .map(|_| {
                self.positions.push(position);
                self.positions.len() - 1
            })
            .collect()
    }

    /// Where in `items` the item at `position` sits, if it has been read from
    /// the source already.
    fn offset_of(&self, position: usize) -> Option<usize> {
        let offset = position.checked_sub(self.base)?;
        (offset < self.items.len()).then_some(offset)
    }
}

/// Builds `consumers` iterators over `source`, sharing one buffer.
pub(crate) fn new_group(source: Value, consumers: usize, vm: &mut VM<'_>) -> Vec<Value> {
    let buffer = TeeBuffer::new(source, consumers);
    let buffer_id = vm
        .heap
        .allocate(HeapData::Itertools(ItertoolsIter::TeeBuffer(Box::new(buffer))));
    let tees = (0..consumers)
        .map(|slot| {
            // Each `Tee` owns its own reference to the buffer, counted here
            // rather than cloned from a temporary `Value` that would then need
            // releasing itself.
            vm.heap.inc_ref(buffer_id);
            let tee = ItertoolsIter::Tee(Tee {
                buffer: Value::Ref(buffer_id),
                slot,
            });
            Value::Ref(vm.heap.allocate(HeapData::Itertools(tee)))
        })
        .collect();
    // The allocation's own reference belongs to no tee, so it goes here — and
    // a `tee(x, 0)` group is freed on the spot.
    Value::Ref(buffer_id).drop_with(vm);
    tees
}

/// Builds `consumers` iterators sharing `tee`'s buffer, from where it stands.
///
/// `None` when `tee` is not one of these iterators, which is how `tee()` tells
/// a copyable argument from one it must drain.
pub(crate) fn fork_group(tee: &Value, consumers: usize, vm: &mut VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = tee else { return None };
    let HeapReadOutput::Itertools(read) = vm.heap.read(*id) else {
        return None;
    };
    let ItertoolsIter::Tee(existing) = read.get(vm.heap) else {
        return None;
    };
    let (buffer, slot) = (existing.buffer.clone_with_heap(vm.heap), existing.slot);
    drop(read);
    defer_drop!(buffer, vm);
    let Value::Ref(buffer_id) = buffer else {
        unreachable!("a tee's buffer is always a heap value")
    };
    let HeapReadOutput::Itertools(mut buffer_read) = vm.heap.read(*buffer_id) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    let ItertoolsIter::TeeBuffer(tee_buffer) = buffer_read.get_mut(vm.heap) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    let slots = tee_buffer.fork(slot, consumers);
    Some(
        slots
            .into_iter()
            .map(|slot| {
                let buffer = buffer.clone_with_heap(vm.heap);
                let tee = ItertoolsIter::Tee(Tee { buffer, slot });
                Value::Ref(vm.heap.allocate(HeapData::Itertools(tee)))
            })
            .collect(),
    )
}

/// Yields this slot's next item, reading one from the source when the buffer
/// has nothing left for it.
pub(super) fn next<'h>(iter: &mut HeapObjectRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Tee(tee) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Tee")
    };
    let (buffer, slot) = (tee.buffer.clone_with_heap(vm.heap), tee.slot);
    defer_drop!(buffer, vm);
    let Value::Ref(buffer_id) = buffer else {
        unreachable!("a tee's buffer is always a heap value")
    };
    let buffer_id = *buffer_id;

    let mut steps = 0usize;
    loop {
        // Driving the source can run user code that steps other slots, so the
        // whole state is re-read each round rather than carried across it.
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;

        let buffered = read_buffer(buffer_id, vm, |buffer, heap| {
            let position = buffer.positions[slot];
            buffer
                .offset_of(position)
                .map(|offset| buffer.items[offset].clone_with_heap(heap))
        });
        if let Some(item) = buffered {
            take(buffer_id, slot, vm);
            return Ok(Some(item));
        }

        let source = read_buffer(buffer_id, vm, |buffer, heap| {
            buffer.source.as_ref().map(|source| source.clone_with_heap(heap))
        });
        let Some(source) = source else {
            return Ok(None);
        };
        defer_drop!(source, vm);
        // A source whose `__next__` steps any iterator of this group would
        // otherwise drive it again from inside its own read. CPython refuses
        // the same way rather than working out what the item belongs to.
        if read_buffer(buffer_id, vm, |buffer, _| buffer.running) {
            return Err(ExcType::tee_reentered());
        }
        write_buffer(buffer_id, vm, |buffer| buffer.running = true);
        let read = next_source(source, vm);
        write_buffer(buffer_id, vm, |buffer| buffer.running = false);
        if let Some(item) = read? {
            // Appended at the end, which is where a freshly read item belongs
            // however far the other slots moved while it was being read.
            write_buffer(buffer_id, vm, |buffer| buffer.items.push_back(item));
        } else {
            let spent = write_buffer(buffer_id, vm, |buffer| buffer.source.take());
            spent.drop_with(vm);
        }
    }
}

/// A `_tee_dataobject` is not itself iterable; only the `_tee`s that hold one
/// read from it, and Python can never reach it to call `next()` anyway.
pub(super) fn buffer_next() -> Option<Value> {
    None
}

/// Advances `slot` past the item it just took, dropping whatever no slot can
/// reach any more.
///
/// Trimming to the slowest slot is what keeps a group that advances together
/// from buffering the whole source.
fn take(buffer_id: HeapId, slot: usize, vm: &mut VM<'_>) {
    let dropped = write_buffer(buffer_id, vm, |buffer| {
        buffer.positions[slot] += 1;
        let slowest = buffer.positions.iter().copied().min().unwrap_or(buffer.base);
        let mut dropped = Vec::new();
        while buffer.base < slowest {
            if let Some(item) = buffer.items.pop_front() {
                dropped.push(item);
            }
            buffer.base += 1;
        }
        dropped
    });
    dropped.drop_with(vm);
}

/// Reads the buffer behind `buffer_id`, with the heap still available for
/// taking references out of it.
///
/// Every access re-reads the buffer: the loop above re-enters the VM between
/// rounds, so no borrow of it may be held across one.
fn read_buffer<T>(buffer_id: HeapId, vm: &VM<'_>, f: impl FnOnce(&TeeBuffer, &HeapReader<'_>) -> T) -> T {
    let HeapReadOutput::Itertools(read) = vm.heap.read(buffer_id) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    let ItertoolsIter::TeeBuffer(buffer) = read.get(vm.heap) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    f(buffer, vm.heap)
}

/// Mutates the buffer behind `buffer_id`.
///
/// The heap is exclusively borrowed for the call, so anything the closure
/// hands back must be a value it already owned — a ref it takes out is
/// released by the caller.
fn write_buffer<T>(buffer_id: HeapId, vm: &mut VM<'_>, f: impl FnOnce(&mut TeeBuffer) -> T) -> T {
    let HeapReadOutput::Itertools(mut read) = vm.heap.read(buffer_id) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    let ItertoolsIter::TeeBuffer(buffer) = read.get_mut(vm.heap) else {
        unreachable!("a tee's buffer is always a tee buffer")
    };
    f(buffer)
}
