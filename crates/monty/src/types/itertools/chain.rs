//! `itertools.chain(*iterables)` — the arguments' items, back to back.

use std::mem;

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
    /// The arguments, still as passed; resolved one at a time by `next`, and
    /// dropped wholesale by `finish` once the chain can reach no more of them.
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
    let mut steps = 0usize;
    loop {
        // Native loop: the VM's dispatch checkpoint is per-`run()`, so a
        // discarding pass over an infinite source reaches none. Poll the
        // tracker so `max_duration` still bites (see `VM::run`'s
        // `CHECK_INTERVAL`).
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let ItertoolsIter::Chain(chain) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Chain")
        };
        if chain.done {
            return Ok(None);
        }

        let Some(current) = chain.current.as_ref().map(|c| c.clone_with_heap(vm.heap)) else {
            // No live source: resolve the next argument, or finish.
            let Some(raw) = chain.sources.get(chain.started).map(|s| s.clone_with_heap(vm.heap)) else {
                finish(iter, vm);
                return Ok(None);
            };
            // `into_py_iter` consumes `raw` on both paths, and raises here for a
            // non-iterable argument — matching CPython's lazy rejection.
            let resolved = into_py_iter_tracking(iter, raw, vm)?;
            chain_mut(iter, vm).current = Some(resolved);
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
        chain_mut(iter, vm).current.take().drop_with(vm);
    }
}

/// Resolves one argument to an iterator, marking it started first so a
/// `TypeError` from a non-iterable does not leave it to be retried.
///
/// A failure here also *ends* the chain: CPython clears its source on an
/// `iter()` failure, so the arguments after the bad one are never reached and
/// every later `next()` is a plain `StopIteration`. Note the asymmetry — an
/// error raised by a resolved source's `__next__` leaves the chain live, since
/// CPython keeps that iterator in place.
fn into_py_iter_tracking<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, raw: Value, vm: &mut VM<'h>) -> RunResult<Value> {
    chain_mut(iter, vm).started += 1;
    match raw.into_py_iter(vm) {
        Ok(resolved) => Ok(resolved),
        Err(err) => {
            finish(iter, vm);
            Err(err)
        }
    }
}

/// Ends the chain, releasing the arguments it will now never reach.
///
/// Both ways a chain ends come through here, because CPython `Py_CLEAR`s its
/// source either way: a spent chain that stays bound must not pin its arguments
/// until it is itself destroyed.
///
/// `current` is already `None` at both callsites — the chain only ends while
/// resolving the next argument — but clearing it keeps this correct for any
/// future path that ends a chain mid-source.
fn finish<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) {
    let chain = chain_mut(iter, vm);
    chain.done = true;
    let sources = mem::take(&mut chain.sources);
    let current = chain.current.take();
    // Dropping these can free the chain's own referrers, so it happens once
    // `chain` (and its borrow of the heap) is out of the way.
    sources.drop_with(vm);
    current.drop_with(vm);
}

/// The `Chain` behind an iterator already dispatched as one.
///
/// `next` re-borrows the heap around every step that can run user code, so the
/// same match-or-`unreachable!` appeared at each one; naming it once keeps those
/// steps readable.
fn chain_mut<'r, 'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &'r mut VM<'h>) -> &'r mut Chain {
    let ItertoolsIter::Chain(chain) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Chain")
    };
    chain
}
