//! `itertools.product(*iterables, repeat=1)` — the cartesian product.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    heap::{HeapId, HeapRead},
    types::{
        TupleVec, allocate_tuple,
        itertools::{
            ItertoolsIter,
            combinatoric::{Phase, pool_child_ids, pool_dec_ref_ids},
        },
    },
    value::Value,
};

/// Yields one tuple per element of the product, rightmost slot fastest.
///
/// `repeat` is applied by indexing rather than by copying the pools: result
/// slot `k` reads `pools[k % pools.len()]`, so `product('ab', repeat=10**6)`
/// costs its index vector and nothing more up front. CPython shares the pool
/// tuples between slots for the same reason.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Product {
    /// The argument iterables, each collected up front.
    pools: Vec<Vec<Value>>,
    /// One position per result slot — `pools.len() * repeat` of them; empty
    /// once `Done` from the start.
    indices: Vec<usize>,
    phase: Phase,
}

impl Product {
    /// Takes the collected pools; `repeat` has been range-checked and the
    /// index vector preflighted by the constructor, which also passes no
    /// pools at all for `repeat=0`.
    pub(crate) fn new(pools: Vec<Vec<Value>>, repeat: usize) -> Self {
        let slots = pools.len() * repeat;
        // An empty pool empties the product — unless there are no slots, in
        // which case the single empty tuple is still yielded.
        let (indices, phase) = if slots > 0 && pools.iter().any(Vec::is_empty) {
            (Vec::new(), Phase::Done)
        } else {
            (vec![0; slots], Phase::Fresh)
        };
        Self { pools, indices, phase }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for pool in &self.pools {
            pool_child_ids(pool, &mut on_child);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for pool in &mut self.pools {
            pool_dec_ref_ids(pool, stack);
        }
    }

    /// Moves `indices` to the next element, or reports that there is none.
    ///
    /// An odometer: the rightmost slot steps, and one that wraps carries into
    /// the slot to its left. Every slot wrapping at once is the end, which for
    /// no slots at all comes straight after the initial empty tuple.
    fn advance(&mut self) -> bool {
        match self.phase {
            Phase::Done => false,
            Phase::Fresh => {
                self.phase = Phase::Running;
                true
            }
            Phase::Running => {
                let width = self.pools.len();
                for (k, index) in self.indices.iter_mut().enumerate().rev() {
                    *index += 1;
                    if *index == self.pools[k % width].len() {
                        *index = 0;
                    } else {
                        return true;
                    }
                }
                self.phase = Phase::Done;
                false
            }
        }
    }
}

/// Steps the indices and gathers one item per slot from its pool.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> Option<Value> {
    let ItertoolsIter::Product(product) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Product")
    };
    if !product.advance() {
        return None;
    }
    let ItertoolsIter::Product(product) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Product")
    };
    let width = product.pools.len();
    let items: TupleVec = product
        .indices
        .iter()
        .enumerate()
        .map(|(k, &i)| product.pools[k % width][i].clone_with_heap(vm.heap))
        .collect();
    Some(allocate_tuple(items, vm.heap))
}
