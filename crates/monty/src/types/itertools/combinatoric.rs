//! What the combinatoric adaptors share.
//!
//! `combinations`, `combinations_with_replacement`, `permutations` and
//! `product` all collect their input into a pool up front and then step a set
//! of indices into it, so none of them re-enters the VM from `next`: the whole
//! step is index arithmetic, and only the result tuple touches the heap.

use serde::{Deserialize, Serialize};

use crate::{
    heap::{Heap, RuntimeRef},
    types::{TupleVec, allocate_tuple},
    value::Value,
};

/// Where a combinatoric adaptor is in its run.
///
/// `Fresh` yields the initial indices untouched, as CPython's first pass does;
/// `Running` steps them; `Done` is terminal, and the constructor starts there
/// when there is nothing to yield at all (`r > n`, an empty `product` pool).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Phase {
    Fresh,
    Running,
    Done,
}

/// Builds the result tuple from the pool items at `indices`.
pub(super) fn gather<H: RuntimeRef>(pool: &[Value<H>], indices: &[usize], heap: &Heap<H>) -> Value<H> {
    let items: TupleVec<H> = indices.iter().map(|&i| pool[i].clone_with_heap(heap)).collect();
    allocate_tuple(items, heap)
}

/// Invokes `on_child` for each heap id the pool owns (GC trace hook).
pub(super) fn pool_child_ids<H: RuntimeRef>(pool: &[Value<H>], on_child: &mut impl FnMut(H)) {
    for item in pool {
        if let Value::Ref(id) = item {
            on_child(*id);
        }
    }
}

/// Releases the refs the pool owns (mirrors [`pool_child_ids`]).
pub(super) fn pool_dec_ref_ids<H: RuntimeRef>(pool: &mut [Value<H>], stack: &mut Vec<H>) {
    for item in pool {
        item.py_dec_ref_ids(stack);
    }
}
