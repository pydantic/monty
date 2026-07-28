//! `itertools.count(start=0, step=1)` — the unbounded arithmetic progression.

use std::{fmt::Write, mem};

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, HeapId, HeapRead},
    types::{LazyHeapSet, PyTrait, itertools::ItertoolsIter},
    value::Value,
};

/// Yields `start`, `start + step`, `start + 2 * step`, … without ever stopping.
///
/// Both fields are OWNED refs held directly rather than inside a container (a
/// count over big integers stores `LongInt` refs), so `py_dec_ref_ids` and
/// `for_each_child_id` must enumerate them or the GC under-traces.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Count {
    /// The value the next `__next__` hands out; advanced by `step` afterwards.
    current: Value,
    /// Added to `current` after each yield, and reported by `repr`.
    step: Value,
}

impl Count {
    /// Takes ownership of `start` and `step`, which the caller has already
    /// checked are numbers — [`next`] relies on `current + step` being defined.
    pub(crate) fn new(start: Value, step: Value) -> Self {
        Self { current: start, step }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.current {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.step {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.current.py_dec_ref_ids(stack);
        self.step.py_dec_ref_ids(stack);
    }
}

/// Hands out `current`, then advances it by `step`.
///
/// Never returns `Ok(None)` — `count` is infinite, so consumption stops only on
/// a host-configured limit. The addition promotes to `LongInt` on overflow, as
/// CPython switches off its `ssize_t` fast path.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    // Resolve to owned clones and release the borrow before `py_add`, which
    // allocates on int -> LongInt promotion and so mutates the heap.
    let (item, step) = {
        let ItertoolsIter::Count(count) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Count")
        };
        (
            count.current.clone_with_heap(vm.heap),
            count.step.clone_with_heap(vm.heap),
        )
    };
    defer_drop!(step, vm);
    // `item` needs a guard rather than `defer_drop!`: a failed `py_add` must
    // release it, but the success path hands it back to the caller.
    let mut item_guard = DropGuard::new(item, vm);
    let (item, vm) = item_guard.as_parts();
    let next = item.py_add(step, vm)?;

    let ItertoolsIter::Count(count) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Count")
    };
    let previous = mem::replace(&mut count.current, next);
    previous.drop_with(vm);
    Ok(Some(item_guard.into_inner()))
}

/// Matches CPython's `count_repr`: the step is shown unless it is an *integer*
/// equal to 1, so `count(0, 1.0)` still prints its step.
pub(super) fn repr_fmt<'h>(
    iter: &HeapRead<'h, ItertoolsIter>,
    f: &mut impl Write,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    let (current, step) = {
        let ItertoolsIter::Count(count) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Count")
        };
        (
            count.current.clone_with_heap(vm.heap),
            count.step.clone_with_heap(vm.heap),
        )
    };
    defer_drop!(current, vm);
    defer_drop!(step, vm);
    f.write_str("count(")?;
    current.py_repr_fmt(f, vm, heap_ids)?;
    // `Float(1.0)` is deliberately absent: CPython's check is
    // `PyLong_Check(step) && step == 1`, so `count(0, 1.0)` keeps its step. No
    // `Bool` arm either — the constructor already widened it to `Int`.
    if !matches!(step, Value::Int(1)) {
        f.write_str(", ")?;
        step.py_repr_fmt(f, vm, heap_ids)?;
    }
    f.write_str(")")?;
    Ok(())
}
