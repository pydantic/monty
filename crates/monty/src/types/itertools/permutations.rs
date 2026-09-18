//! `itertools.permutations(iterable, r=None)` — `r`-length orderings.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    heap::{HeapId, HeapRead},
    types::itertools::{
        ItertoolsIter,
        combinatoric::{Phase, gather, pool_child_ids, pool_dec_ref_ids},
    },
    value::Value,
};

/// Yields the `r`-permutations of a pool, in the order the pool was given.
///
/// CPython's algorithm: `indices` is a full permutation of the pool whose
/// first `r` entries are the next result, and `cycles[i]` counts how many
/// more times slot `i` swaps with the tail before the slot to its left moves.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Permutations {
    /// Every item of the source, collected up front as CPython's `pool` tuple.
    pool: Vec<Value>,
    /// A permutation of `0..n`; empty once `Done` from the start.
    indices: Vec<usize>,
    /// `r` counters, `n, n-1, …, n-r+1` to begin with.
    cycles: Vec<usize>,
    r: usize,
    phase: Phase,
}

impl Permutations {
    /// Takes the collected pool; `r` has been range-checked by the constructor.
    pub(crate) fn new(pool: Vec<Value>, r: usize) -> Self {
        let n = pool.len();
        // No `r`-permutation of fewer than `r` items, so nothing is allocated.
        let (indices, cycles, phase) = if r > n {
            (Vec::new(), Vec::new(), Phase::Done)
        } else {
            ((0..n).collect(), (n - r + 1..=n).rev().collect(), Phase::Fresh)
        };
        Self {
            pool,
            indices,
            cycles,
            r,
            phase,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        pool_child_ids(&self.pool, &mut on_child);
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        pool_dec_ref_ids(&mut self.pool, stack);
    }

    /// Moves `indices` to the next permutation, or reports that there is none.
    ///
    /// From the right, each slot's cycle is spent by one; a slot with cycles
    /// left swaps with the tail entry its count names and the step is done,
    /// while an exhausted one rotates the tail back into order, resets, and
    /// hands the step to the slot on its left. Every slot exhausted at once is
    /// the end.
    fn advance(&mut self) -> bool {
        match self.phase {
            Phase::Done => false,
            Phase::Fresh => {
                self.phase = Phase::Running;
                true
            }
            Phase::Running => {
                let n = self.indices.len();
                for i in (0..self.r).rev() {
                    self.cycles[i] -= 1;
                    if self.cycles[i] == 0 {
                        self.indices[i..].rotate_left(1);
                        self.cycles[i] = n - i;
                    } else {
                        let j = self.cycles[i];
                        self.indices.swap(i, n - j);
                        return true;
                    }
                }
                self.phase = Phase::Done;
                false
            }
        }
    }
}

/// Steps the indices and gathers the pool items the first `r` select.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> Option<Value> {
    let ItertoolsIter::Permutations(permutations) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Permutations")
    };
    if !permutations.advance() {
        return None;
    }
    let ItertoolsIter::Permutations(permutations) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Permutations")
    };
    Some(gather(
        &permutations.pool,
        &permutations.indices[..permutations.r],
        vm.heap,
    ))
}
