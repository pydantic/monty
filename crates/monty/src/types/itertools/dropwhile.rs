//! `itertools.dropwhile(predicate, iterable)` — everything after the leading run.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, DropWithContext, HeapId, HeapRead},
    types::itertools::{ItertoolsIter, predicate::call_predicate},
    value::Value,
};

/// Discards items while `predicate` holds, then yields the rest untested.
///
/// `predicate` is `Some` only while still dropping — it is released on the
/// first item it rejects, since it is never called again after that (matching
/// CPython), and that item is the first one yielded. The source stays owned
/// throughout: unlike `takewhile` this adaptor never latches, so every later
/// `next` drives it again.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DropWhile {
    predicate: Option<Value>,
    source: Value,
}

impl DropWhile {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(predicate: Value, source: Value) -> Self {
        Self {
            predicate: Some(predicate),
            source,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = &self.predicate {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(predicate) = &mut self.predicate {
            predicate.py_dec_ref_ids(stack);
        }
        self.source.py_dec_ref_ids(stack);
    }
}

/// Skips items while the predicate holds; once it has failed, yields straight
/// through without consulting it again.
pub(super) fn next<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        let ItertoolsIter::DropWhile(drop_while) = iter.get(vm.heap) else {
            unreachable!("dispatched on Kind::DropWhile")
        };
        let predicate = drop_while.predicate.as_ref().map(|p| p.clone_with_heap(vm.heap));
        let source = drop_while.source.clone_with_heap(vm.heap);
        defer_drop!(predicate, vm);
        defer_drop!(source, vm);

        let item = {
            let mut read = source.read(vm);
            read.py_next(vm)
        };
        let Some(item) = item? else {
            return Ok(None);
        };
        let Some(predicate) = predicate else {
            return Ok(Some(item));
        };
        // Still dropping: the item is tested, and kept only once the predicate
        // fails — so it needs a guard across a call that may raise.
        let mut item_guard = DropGuard::new(item, vm);
        let (item, vm) = item_guard.as_parts_mut();
        if !call_predicate(predicate, item, "dropwhile()", vm)? {
            let ItertoolsIter::DropWhile(drop_while) = iter.get_mut(vm.heap) else {
                unreachable!("dispatched on Kind::DropWhile")
            };
            // Released as it stops being reachable, rather than at destruction.
            drop_while.predicate.take().drop_with(vm);
            let (item, _) = item_guard.into_parts();
            return Ok(Some(item));
        }
    }
}
