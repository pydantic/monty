//! `itertools.combinations(iterable, r)` and
//! `itertools.combinations_with_replacement(iterable, r)` — `r`-length
//! subsequences, in the order the pool was given.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    heap::{HeapId, HeapRead},
    types::{
        Type,
        itertools::{
            ItertoolsIter,
            combinatoric::{Phase, gather, pool_child_ids, pool_dec_ref_ids},
        },
    },
    value::Value,
};

/// Yields the `r`-combinations of a pool, with or without replacement.
///
/// One struct serves both callables: they differ only in whether an index may
/// repeat the one to its left, which changes the ceiling each slot climbs to
/// and how the slots after a stepped one are refilled. `py_type` keeps them
/// distinct Python types.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Combinations {
    /// Every item of the source, collected up front as CPython's `pool` tuple.
    pool: Vec<Value>,
    /// One position into `pool` per slot of the next result; empty once `Done`
    /// from the start, so `combinations([], 10**9)` allocates nothing.
    indices: Vec<usize>,
    /// Whether this is the `_with_replacement` flavour.
    replacement: bool,
    phase: Phase,
}

impl Combinations {
    /// Takes the collected pool; `r` has been range-checked and, for the
    /// replacement flavour, preflighted by the constructor.
    pub(crate) fn new(pool: Vec<Value>, r: usize, replacement: bool) -> Self {
        let n = pool.len();
        // CPython's `stopped`: without replacement there is no `r`-subsequence
        // of fewer than `r` items; with it, only an empty pool has none (and
        // even that yields the single empty tuple when `r == 0`).
        let stopped = if replacement { n == 0 && r > 0 } else { r > n };
        let (indices, phase) = if stopped {
            (Vec::new(), Phase::Done)
        } else if replacement {
            (vec![0; r], Phase::Fresh)
        } else {
            ((0..r).collect(), Phase::Fresh)
        };
        Self {
            pool,
            indices,
            replacement,
            phase,
        }
    }

    /// The Python type — one per callable, though the state is shared.
    pub(crate) fn py_type(&self) -> Type {
        if self.replacement {
            Type::ItertoolsCombinationsWithReplacement
        } else {
            Type::ItertoolsCombinations
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

    /// Moves `indices` to the next combination, or reports that there is none.
    ///
    /// The rightmost slot below its ceiling steps up and every slot after it
    /// is refilled: to the run following it without replacement, to a copy of
    /// it with. No slot below its ceiling means the last combination is out.
    fn advance(&mut self) -> bool {
        match self.phase {
            Phase::Done => false,
            Phase::Fresh => {
                self.phase = Phase::Running;
                true
            }
            Phase::Running => {
                let (n, r) = (self.pool.len(), self.indices.len());
                let pivot = if self.replacement {
                    (0..r).rev().find(|&i| self.indices[i] != n - 1)
                } else {
                    (0..r).rev().find(|&i| self.indices[i] != i + n - r)
                };
                match pivot {
                    None => {
                        self.phase = Phase::Done;
                        false
                    }
                    Some(i) => {
                        let stepped = self.indices[i] + 1;
                        for (offset, slot) in self.indices[i..].iter_mut().enumerate() {
                            *slot = if self.replacement { stepped } else { stepped + offset };
                        }
                        true
                    }
                }
            }
        }
    }
}

/// Steps the indices and gathers the pool items they select.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> Option<Value> {
    let ItertoolsIter::Combinations(combinations) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Combinations")
    };
    if !combinations.advance() {
        return None;
    }
    let ItertoolsIter::Combinations(combinations) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Combinations")
    };
    Some(gather(&combinations.pool, &combinations.indices, vm.heap))
}
