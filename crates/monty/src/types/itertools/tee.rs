//! `itertools.tee(iterable, n=2)` — independent iterators over one source.
//!
//! The read-ahead is a chain of blocks rather than one buffer, which is how
//! CPython arranges it and why neither needs to know which consumers are still
//! alive: a [`Tee`] owns the [`TeeBlock`] it is reading, so a block is freed by
//! ordinary reference counting once every consumer has moved past it — a
//! consumer that is simply dropped releases its block with it.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapObjectRead, HeapReadOutput, HeapReader},
    types::itertools::{ItertoolsIter, step::next_source},
    value::Value,
};

/// How many items one block holds before the chain moves on.
///
/// CPython's `LINKCELLS`, where the number is picked so a block plus its
/// header fills a memory page.
const BLOCK: usize = 57;

/// One consumer of a `tee()` group.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Tee {
    /// The block being read — owned, so everything behind it can be freed as
    /// this consumer moves on, and so dropping this consumer releases it.
    block: Value,
    /// How far into that block this consumer has read.
    index: usize,
}

impl Tee {
    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.block {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.block.py_dec_ref_ids(stack);
    }
}

/// One link of the read-ahead chain, CPython's `_tee_dataobject`.
///
/// Boxed inside [`ItertoolsIter`]: the source, the items and the link to the
/// next block put it past the family's size budget.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TeeBlock {
    /// The iterator every block of this chain reads from.
    source: Value,
    /// What has been read into this block, at most [`BLOCK`] items.
    items: Vec<Value>,
    /// The block that follows, built when this one fills.
    next: Option<Value>,
    /// Whether the source is being read right now, so a consumer stepped from
    /// inside that read is refused rather than driving it again.
    running: bool,
}

impl TeeBlock {
    /// Opens a chain over `source`.
    fn new(source: Value) -> Self {
        Self {
            source,
            items: Vec::new(),
            next: None,
            running: false,
        }
    }

    /// Invokes `on_child` for each heap id this block owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [Some(&self.source), self.next.as_ref()].into_iter().flatten() {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
        for item in &self.items {
            if let Value::Ref(id) = item {
                on_child(*id);
            }
        }
    }

    /// Releases the refs this block owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.source.py_dec_ref_ids(stack);
        if let Some(next) = &mut self.next {
            next.py_dec_ref_ids(stack);
        }
        for item in &mut self.items {
            item.py_dec_ref_ids(stack);
        }
    }
}

/// Builds `consumers` iterators over `source`, all starting at one new chain.
pub(crate) fn new_group(source: Value, consumers: usize, vm: &mut VM<'_>) -> Vec<Value> {
    let block = TeeBlock::new(source);
    let block_id = vm
        .heap
        .allocate(HeapData::Itertools(ItertoolsIter::TeeBlock(Box::new(block))));
    let tees = consumers_at(block_id, 0, consumers, vm);
    // The allocation's own reference belongs to no consumer, so it goes here.
    Value::Ref(block_id).drop_with(vm);
    tees
}

/// Builds `consumers` iterators reading from where `tee` stands.
///
/// `None` when `tee` is not one of these iterators, which is how `tee()` tells
/// a copyable argument from one it must drain. CPython copies through
/// `__copy__` for the same reason: the copies replay from the original's
/// position rather than draining it.
pub(crate) fn fork_group(tee: &Value, consumers: usize, vm: &mut VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = tee else { return None };
    let HeapReadOutput::Itertools(read) = vm.heap.read(*id) else {
        return None;
    };
    let ItertoolsIter::Tee(existing) = read.get(vm.heap) else {
        return None;
    };
    let (block, index) = (existing.block.clone_with_heap(vm.heap), existing.index);
    drop(read);
    defer_drop!(block, vm);
    let Value::Ref(block_id) = block else {
        unreachable!("a tee's block is always a heap value")
    };
    Some(consumers_at(*block_id, index, consumers, vm))
}

/// Makes `consumers` iterators, each reading `block_id` from `index`.
fn consumers_at(block_id: HeapId, index: usize, consumers: usize, vm: &mut VM<'_>) -> Vec<Value> {
    (0..consumers)
        .map(|_| {
            // Each consumer owns its own reference to the block, counted here
            // rather than cloned from a temporary that would need releasing.
            vm.heap.inc_ref(block_id);
            let tee = ItertoolsIter::Tee(Tee {
                block: Value::Ref(block_id),
                index,
            });
            Value::Ref(vm.heap.allocate(HeapData::Itertools(tee)))
        })
        .collect()
}

/// Yields this consumer's next item, reading one from the source when the
/// chain has nothing further for it.
pub(super) fn next<'h>(iter: &mut HeapObjectRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        // Reading the source runs user code that can step other consumers, so
        // the position is re-read each round rather than carried across one.
        let (block_id, index) = position(iter, vm);
        if let Some(item) = read_block(block_id, vm, |block, heap| {
            block.items.get(index).map(|item| item.clone_with_heap(heap))
        }) {
            set_position(iter, block_id, index + 1, vm);
            return Ok(Some(item));
        }
        // A full block hands the consumer on to the next one, which is what
        // lets the block it leaves behind be freed once the others follow.
        if index == BLOCK {
            let next_id = jump(block_id, vm);
            set_position(iter, next_id, 0, vm);
            continue;
        }
        if !fill(block_id, vm)? {
            // The source yielded nothing this time. It is not cleared, so a
            // source that stops and later yields again is read again, as
            // CPython's `teedataobject_getitem` does.
            return Ok(None);
        }
    }
}

/// A `_tee_dataobject` is not itself iterable; only the `_tee`s that read one
/// step it, and Python can never reach a block to call `next()` on it anyway.
pub(super) fn block_next() -> Option<Value> {
    None
}

/// Where this consumer is reading.
fn position<'h>(iter: &HeapObjectRead<'h, ItertoolsIter>, vm: &VM<'h>) -> (HeapId, usize) {
    let ItertoolsIter::Tee(tee) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Tee")
    };
    let Value::Ref(block_id) = tee.block else {
        unreachable!("a tee's block is always a heap value")
    };
    (block_id, tee.index)
}

/// Moves this consumer to `index` of `block_id`, taking a reference to that
/// block when it is a new one and releasing the one being left.
fn set_position<'h>(iter: &mut HeapObjectRead<'h, ItertoolsIter>, block_id: HeapId, index: usize, vm: &mut VM<'h>) {
    let (previous, _) = position(iter, vm);
    if previous == block_id {
        let ItertoolsIter::Tee(tee) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Tee")
        };
        tee.index = index;
        return;
    }
    vm.heap.inc_ref(block_id);
    let ItertoolsIter::Tee(tee) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Tee")
    };
    tee.index = index;
    tee.block = Value::Ref(block_id);
    // Released after the borrow ends, since freeing a block runs cleanup that
    // needs the heap itself.
    Value::Ref(previous).drop_with(vm);
}

/// The block after `block_id`, opening one over the same source if this is the
/// first consumer to reach the end of this one.
fn jump(block_id: HeapId, vm: &mut VM<'_>) -> HeapId {
    let existing = read_block(block_id, vm, |block, _| match &block.next {
        Some(Value::Ref(id)) => Some(*id),
        _ => None,
    });
    if let Some(next_id) = existing {
        return next_id;
    }
    let source = read_block(block_id, vm, |block, heap| block.source.clone_with_heap(heap));
    let next = TeeBlock::new(source);
    let next_id = vm
        .heap
        .allocate(HeapData::Itertools(ItertoolsIter::TeeBlock(Box::new(next))));
    write_block(block_id, vm, |block| block.next = Some(Value::Ref(next_id)));
    next_id
}

/// Reads one item from the source into `block_id`, reporting whether it got one.
fn fill(block_id: HeapId, vm: &mut VM<'_>) -> RunResult<bool> {
    let source = read_block(block_id, vm, |block, heap| block.source.clone_with_heap(heap));
    defer_drop!(source, vm);
    // A source whose `__next__` steps any consumer of this group would
    // otherwise drive it again from inside its own read. CPython refuses the
    // same way rather than working out what the item belongs to.
    if read_block(block_id, vm, |block, _| block.running) {
        return Err(ExcType::tee_reentered());
    }
    write_block(block_id, vm, |block| block.running = true);
    let read = next_source(source, vm);
    write_block(block_id, vm, |block| block.running = false);
    if let Some(item) = read? {
        write_block(block_id, vm, |block| block.items.push(item));
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Reads the block behind `block_id`, with the heap still available for taking
/// references out of it.
///
/// Every access re-reads the block: the loop above re-enters the VM between
/// rounds, so no borrow of one may be held across a round.
fn read_block<T>(block_id: HeapId, vm: &VM<'_>, f: impl FnOnce(&TeeBlock, &HeapReader<'_>) -> T) -> T {
    let HeapReadOutput::Itertools(read) = vm.heap.read(block_id) else {
        unreachable!("a tee's block is always a tee block")
    };
    let ItertoolsIter::TeeBlock(block) = read.get(vm.heap) else {
        unreachable!("a tee's block is always a tee block")
    };
    f(block, vm.heap)
}

/// Mutates the block behind `block_id`.
///
/// The heap is exclusively borrowed for the call, so the closure can only work
/// with values it is handed or already owns.
fn write_block<T>(block_id: HeapId, vm: &mut VM<'_>, f: impl FnOnce(&mut TeeBlock) -> T) -> T {
    let HeapReadOutput::Itertools(mut read) = vm.heap.read(block_id) else {
        unreachable!("a tee's block is always a tee block")
    };
    let ItertoolsIter::TeeBlock(block) = read.get_mut(vm.heap) else {
        unreachable!("a tee's block is always a tee block")
    };
    f(block)
}
