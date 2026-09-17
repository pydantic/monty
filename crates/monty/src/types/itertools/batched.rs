//! `itertools.batched(iterable, n, *, strict=False)` — consecutive n-tuples.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapId, HeapRead},
    resource_checks::check_estimated_size,
    types::{
        TupleVec, allocate_tuple,
        itertools::{ItertoolsIter, step::next_source},
    },
    value::{VALUE_SIZE, Value},
};

/// Yields tuples of up to `n` consecutive items from `source`.
///
/// A SHORT batch does not latch — CPython only clears its source when a batch
/// comes back empty, or when `strict` rejects a short one — so a source that
/// stutters to a stop and yields again is batched from where it left off.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Batched {
    /// `None` once the source is spent, which latches the adaptor.
    source: Option<Value>,
    n: usize,
    /// Whether a short final batch raises instead of being yielded.
    strict: bool,
}

impl Batched {
    /// Takes ownership of `source`, already resolved by `py_iter`; `n` is
    /// validated as at least one by the constructor.
    pub(crate) fn new(source: Value, n: usize, strict: bool) -> Self {
        Self {
            source: Some(source),
            n,
            strict,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(source) = &mut self.source {
            source.py_dec_ref_ids(stack);
        }
    }
}

/// Fills one batch, latching only when it comes back empty or `strict` rejects it.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Batched(batched) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Batched")
    };
    let Some(source) = batched.source.as_ref().map(|source| source.clone_with_heap(vm.heap)) else {
        return Ok(None);
    };
    let (n, strict) = (batched.n, batched.strict);
    defer_drop!(source, vm);

    // One-shot preflight from the size hint, as `deque_extend` does, capped at
    // `n` because a batch keeps at most that many however long the source. A
    // hint-less source gets no estimate and falls back to the hard limit.
    let hint = source.read(vm).iter_size_hint(vm);
    check_estimated_size(hint.min(n).saturating_mul(VALUE_SIZE), &vm.heap.tracker)?;

    // Still not preallocated to `n`: the preflight above bounds an exact-hint
    // source, but `n` is an arbitrary user integer and a hint-less source gets
    // no estimate. The batch grows with what the source actually yields.
    let mut batch_guard = DropGuard::new(Vec::new(), vm);
    let (batch, vm) = batch_guard.as_parts_mut();
    while batch.len() < n {
        // A large `n` fills for a long time without reaching the VM's dispatch
        // checkpoint, so both limits are only enforceable from in here. Memory
        // as well as time because a hint-less source (`count()`) gets no
        // preflight above, and a Rust-side source reaches no checkpoint of its
        // own. `batch.len()` is monotonic, so it keys the amortization.
        vm.heap.tracker.check_memory_time_every(batch.len())?;
        let Some(item) = next_source(source, vm)? else {
            break;
        };
        batch.push(item);
    }

    let empty = batch.is_empty();
    let short = batch.len() < n;
    // An empty batch means the source is spent on a batch boundary, and under
    // `strict` a short one is about to raise. Both release the source here
    // rather than at destruction; a short batch on its own does NOT, so a
    // source that stutters to a stop is still driven again.
    if empty || (short && strict) {
        finish(iter, vm);
    }
    // Checked before `strict`, as CPython does: running out on a boundary is an
    // ordinary stop, not an incomplete batch.
    if empty {
        return Ok(None);
    }
    if short && strict {
        // The partial batch is dropped by the guard as this unwinds.
        return Err(ExcType::batched_incomplete());
    }

    let (batch, vm) = batch_guard.into_parts();
    Ok(Some(allocate_tuple(TupleVec::from_vec(batch), vm.heap)))
}

/// Latches the adaptor, releasing the source as soon as it stops being reachable.
fn finish<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) {
    let ItertoolsIter::Batched(batched) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Batched")
    };
    batched.source.take().drop_with(vm);
}
