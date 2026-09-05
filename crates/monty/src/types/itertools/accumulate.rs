//! `itertools.accumulate(iterable, func=None, *, initial=None)` — running totals.

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropWithContext, HeapId, HeapRead},
    types::itertools::{ItertoolsIter, step::next_item},
    value::Value,
};

/// Yields the running total of `source`, combined by `func` or by `+`.
///
/// `total` doubles as the `initial` slot: given one it starts there and
/// `pending` makes the first `next` yield it without touching the source, which
/// is why `accumulate([], initial=x)` still yields `x`. CPython keeps `initial`
/// as a separate field, but the family shares one 64-byte budget — see
/// [`ItertoolsIter`]. Exhaustion does not latch: `total` is kept, so a source
/// that yields again is accumulated onto where it left off.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Accumulate {
    source: Value,
    /// `Value::None` selects `+`, as CPython's absent `binop` does.
    func: Value,
    /// The running total; `None` until the first item is taken.
    total: Option<Value>,
    /// Whether `total` holds an `initial` still to be yielded untouched.
    pending: bool,
}

impl Accumulate {
    /// Takes ownership of all three, with `source` already resolved by `py_iter`.
    pub(crate) fn new(source: Value, func: Value, initial: Option<Value>) -> Self {
        Self {
            source,
            func,
            pending: initial.is_some(),
            total: initial,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.func {
            on_child(*id);
        }
        if let Some(Value::Ref(id)) = &self.total {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.source.py_dec_ref_ids(stack);
        self.func.py_dec_ref_ids(stack);
        if let Some(total) = &mut self.total {
            total.py_dec_ref_ids(stack);
        }
    }
}

/// Yields the initial if one is waiting, else folds the next item into the total.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let ItertoolsIter::Accumulate(accumulate) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Accumulate")
    };
    // An `initial` is yielded before the source is touched at all, so
    // `accumulate([], initial=x)` still produces one value.
    if accumulate.pending {
        let initial = accumulate
            .total
            .as_ref()
            .expect("pending is only set alongside an initial")
            .clone_with_heap(vm.heap);
        let ItertoolsIter::Accumulate(accumulate) = iter.get_mut(vm.heap) else {
            unreachable!("dispatched on Kind::Accumulate")
        };
        accumulate.pending = false;
        return Ok(Some(initial));
    }

    let func = accumulate.func.clone_with_heap(vm.heap);
    let source = accumulate.source.clone_with_heap(vm.heap);
    let total = accumulate.total.as_ref().map(|total| total.clone_with_heap(vm.heap));
    defer_drop!(func, vm);
    defer_drop!(total, vm);

    let Some(item) = next_item(source, vm)? else {
        return Ok(None);
    };
    let combined = match total {
        // The first item becomes the total untouched — never `func(item)`.
        None => item,
        Some(total) => {
            // Guarded: combining re-enters the VM through `func` or a user
            // `__add__`, either of which may raise while the item is held.
            defer_drop!(item, vm);
            combine(total, item, func, vm)?
        }
    };

    let yielded = combined.clone_with_heap(vm.heap);
    let ItertoolsIter::Accumulate(accumulate) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::Accumulate")
    };
    let previous = accumulate.total.replace(combined);
    previous.drop_with(vm);
    Ok(Some(yielded))
}

/// Folds `item` into `total`, by `func` when one was given and by `+` otherwise.
///
/// Both are borrowed: `func` gets its own references, and the caller still owns
/// the item until its guard ends.
fn combine(total: &Value, item: &Value, func: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if matches!(func, Value::None) {
        total.py_add(item, vm)
    } else {
        let args = ArgValues::Two(total.clone_with_heap(vm.heap), item.clone_with_heap(vm.heap));
        vm.evaluate_function("accumulate()", func, args)
    }
}
