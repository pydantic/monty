//! `itertools.chain(*iterables)` and `itertools.chain.from_iterable(iterable)`
//! — the sources' items, back to back.

use std::mem;

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, HeapId, HeapRead},
    types::itertools::{ItertoolsIter, step::next_source},
    value::Value,
};

/// Yields every item of each source in turn.
///
/// Sources are held UNRESOLVED: CPython calls `iter()` on each only as it
/// reaches it, so `chain([1], 5)` constructs cleanly and raises `TypeError`
/// part-way through consumption.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Chain {
    /// Where the sources come from; emptied by `finish` once the chain can
    /// reach no more of them.
    sources: ChainSources,
    /// The resolved iterator currently being drained.
    current: Option<Value>,
    done: bool,
}

/// The two ways a chain is given its sources.
///
/// One enum rather than two adaptors because everything after the source is
/// taken — resolving it, draining it, ending the chain on a failure — is the
/// same, and CPython too models `from_iterable` as a `chain` over a different
/// `source` iterator.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ChainSources {
    /// `chain(*iterables)`: the arguments, still as passed. Each is moved out
    /// as it is reached, leaving `None` in its slot.
    Args { iterables: Vec<Value>, next: usize },
    /// `chain.from_iterable(iterable)`: the outer iterator, resolved by the
    /// constructor, whose items are the sources.
    Outer(Value),
}

impl ChainSources {
    /// Takes everything these sources still hold, leaving them empty.
    ///
    /// The empty state is an exhausted `Args`, since only a unit variant could
    /// be a `Default` and neither of these is one.
    fn take(&mut self) -> Self {
        mem::replace(
            self,
            Self::Args {
                iterables: Vec::new(),
                next: 0,
            },
        )
    }

    /// Moves the next argument out, or `None` once all are taken or the
    /// sources are an outer iterator (which `next` drives separately).
    fn take_next_arg(&mut self) -> Option<Value> {
        match self {
            Self::Args { iterables, next } => {
                let raw = iterables.get_mut(*next).map(|slot| mem::replace(slot, Value::None));
                *next += 1;
                raw
            }
            Self::Outer(_) => None,
        }
    }

    /// Invokes `on_child` for each heap id these sources own (GC trace hook).
    fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        match self {
            Self::Args { iterables, .. } => {
                for source in iterables {
                    if let Value::Ref(id) = source {
                        on_child(*id);
                    }
                }
            }
            Self::Outer(Value::Ref(id)) => on_child(*id),
            Self::Outer(_) => {}
        }
    }

    /// Releases the refs these sources own (mirrors `for_each_child_id`).
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Args { iterables, .. } => {
                for source in iterables {
                    source.py_dec_ref_ids(stack);
                }
            }
            Self::Outer(outer) => outer.py_dec_ref_ids(stack),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for ChainSources {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Args { iterables, .. } => iterables.drop_with(ctx),
            Self::Outer(outer) => outer.drop_with(ctx),
        }
    }
}

impl Chain {
    /// Takes the arguments unresolved — see the type docs for why.
    pub(crate) fn new(iterables: Vec<Value>) -> Self {
        Self::with_sources(ChainSources::Args { iterables, next: 0 })
    }

    /// Takes the outer iterator of `chain.from_iterable`, already resolved:
    /// CPython resolves that one up front, so `chain.from_iterable(5)` raises
    /// at construction while its items are still resolved lazily.
    pub(crate) fn from_iterable(outer: Value) -> Self {
        Self::with_sources(ChainSources::Outer(outer))
    }

    fn with_sources(sources: ChainSources) -> Self {
        Self {
            sources,
            current: None,
            done: false,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        self.sources.for_each_child_id(&mut on_child);
        if let Some(Value::Ref(id)) = &self.current {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.sources.py_dec_ref_ids(stack);
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
            // No live source: take the next one, or finish.
            let Some(raw) = next_raw_source(iter, vm)? else {
                finish(iter, vm);
                return Ok(None);
            };
            // `into_py_iter` consumes `raw` on both paths, and raises here for a
            // non-iterable source — matching CPython's lazy rejection.
            let resolved = into_py_iter_tracking(iter, raw, vm)?;
            chain_mut(iter, vm).current = Some(resolved);
            continue;
        };

        defer_drop!(current, vm);
        if let Some(item) = next_source(current, vm)? {
            return Ok(Some(item));
        }
        // This source is spent; release it and move to the next argument.
        chain_mut(iter, vm).current.take().drop_with(vm);
    }
}

/// The next source still unresolved: the next argument, or the outer
/// iterator's next item. `None` once there are no more.
///
/// An outer iterator that raises ends the chain as well as propagating, as
/// CPython clears its `source` on any failed `PyIter_Next`.
fn next_raw_source<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Chain(chain) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Chain")
    };
    let outer = match &chain.sources {
        ChainSources::Outer(outer) => Some(outer.clone_with_heap(vm.heap)),
        ChainSources::Args { .. } => None,
    };
    match outer {
        None => Ok(chain_mut(iter, vm).sources.take_next_arg()),
        Some(outer) => {
            defer_drop!(outer, vm);
            match next_source(outer, vm) {
                Ok(item) => Ok(item),
                Err(err) => {
                    finish(iter, vm);
                    Err(err)
                }
            }
        }
    }
}

/// Resolves one source to an iterator.
///
/// A failure here *ends* the chain: CPython clears its source on an `iter()`
/// failure, so the sources after the bad one are never reached and every
/// later `next()` is a plain `StopIteration`. Note the asymmetry — an error
/// raised by a resolved source's `__next__` leaves the chain live, since
/// CPython keeps that iterator in place.
fn into_py_iter_tracking<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, raw: Value, vm: &mut VM<'h>) -> RunResult<Value> {
    match raw.into_py_iter(vm) {
        Ok(resolved) => Ok(resolved),
        Err(err) => {
            finish(iter, vm);
            Err(err)
        }
    }
}

/// Ends the chain, releasing the sources it will now never reach.
///
/// Every way a chain ends comes through here, because CPython `Py_CLEAR`s its
/// source either way: a spent chain that stays bound must not pin its arguments
/// or outer iterator until it is itself destroyed.
///
/// `current` is already `None` at every callsite — the chain only ends while
/// taking the next source — but clearing it keeps this correct for any future
/// path that ends a chain mid-source.
fn finish<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) {
    let chain = chain_mut(iter, vm);
    chain.done = true;
    let sources = chain.sources.take();
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
