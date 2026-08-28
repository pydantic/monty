//! `itertools.repeat(object, times=?)` — one object handed out over and over.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{HeapId, HeapRead},
    types::{LazyHeapSet, PyTrait, itertools::ItertoolsIter},
    value::Value,
};

/// Yields the same `object` `remaining` more times, or forever when unbounded.
///
/// `object` is an OWNED ref held directly, so `py_dec_ref_ids` and
/// `for_each_child_id` must enumerate it or the GC under-traces. Deliberately
/// NOT released on exhaustion — CPython keeps it too, hence `repeat(1, 0)`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Repeat {
    /// Handed out by reference on each step; the same object every time.
    object: Value,
    /// Yields left, or `None` for an unbounded `repeat(x)`.
    remaining: Option<usize>,
}

impl Repeat {
    /// Takes ownership of `object`. `remaining` of `None` repeats forever;
    /// CPython clamps a negative `times` to zero, so callers pass `Some(0)`.
    pub(crate) fn new(object: Value, remaining: Option<usize>) -> Self {
        Self { object, remaining }
    }

    /// Returns the exact number of values not yet yielded, and `0` when
    /// unbounded (see [`ItertoolsIter::size_hint`]).
    pub(crate) fn size_hint(&self) -> usize {
        self.remaining.unwrap_or(0)
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.object {
            on_child(*id);
        }
    }

    /// Releases the ref this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.object.py_dec_ref_ids(stack);
    }
}

/// Hands out a new reference to the *same* object, so `a is b` holds across
/// steps, until `remaining` runs out. Infallible, unlike every other adaptor,
/// but keeps the fallible signature for a uniform dispatch shape.
#[expect(clippy::unnecessary_wraps, reason = "uniform per-adaptor dispatch signature")]
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Repeat(repeat) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Repeat")
    };
    match repeat.remaining {
        Some(0) => Ok(None),
        remaining => {
            let item = repeat.object.clone_with_heap(vm.heap);
            if let Some(remaining) = remaining {
                let ItertoolsIter::Repeat(repeat) = iter.get_mut(vm.heap) else {
                    unreachable!("dispatched on Kind::Repeat")
                };
                repeat.remaining = Some(remaining - 1);
            }
            Ok(Some(item))
        }
    }
}

/// Matches CPython's `repeat_repr`: `repeat(obj)` when unbounded, otherwise
/// `repeat(obj, remaining)` — the count shown is what is *left*, so it shrinks
/// as the iterator is consumed and reads `0` once spent.
pub(super) fn repr_fmt<'h>(
    iter: &HeapRead<'h, ItertoolsIter>,
    f: &mut impl Write,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    // Bound the Rust stack the same way `repr_sequence_fmt` does: cycles are
    // caught by `heap_ids`, but a deeply nested chain of repeats is not.
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;

    let (object, remaining) = {
        let ItertoolsIter::Repeat(repeat) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::Repeat")
        };
        (repeat.object.clone_with_heap(vm.heap), repeat.remaining)
    };
    defer_drop!(object, vm);
    f.write_str("repeat(")?;
    object.py_repr_fmt(f, vm, heap_ids)?;
    if let Some(remaining) = remaining {
        write!(f, ", {remaining}")?;
    }
    f.write_str(")")?;
    Ok(())
}
