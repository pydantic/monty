//! The `callable_iterator` produced by the two-argument `iter(callable, sentinel)`.
//!
//! Unlike every other iterator, this one re-enters Python on each step, which
//! drives the two unusual things about it: the heap borrow must be released
//! before the call (see [`HeapRead::py_next`]), and its two owned refs live in
//! the iterator itself rather than in a container it points at.

use std::mem;

use serde::{Deserialize, Serialize};

use crate::{
    ResourceTracker,
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, HeapId, HeapItem, HeapRead},
    types::{PyTrait, Type},
    value::Value,
};

/// Calls `callable()` for each item until a result compares `==` to `sentinel`.
///
/// `callable` and `sentinel` are OWNED refs held directly here — not in a
/// referenced container — so `py_dec_ref_ids` and `for_each_child_id` must both
/// enumerate them or the GC will under-trace and free a live object.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CallableIterator {
    /// Invoked with no arguments to produce each item.
    callable: Value,
    /// Iteration stops once a result compares `==` to this.
    sentinel: Value,
    /// Latches on exhaustion so a spent iterator never calls `callable` again.
    done: bool,
}

impl CallableIterator {
    /// Takes ownership of `callable` and `sentinel` from the caller.
    pub(crate) fn new(callable: Value, sentinel: Value) -> Self {
        Self {
            callable,
            sentinel,
            done: false,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.callable {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.sentinel {
            on_child(*id);
        }
    }
}

impl HeapItem for CallableIterator {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.callable.py_dec_ref_ids(stack);
        self.sentinel.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, CallableIterator> {
    fn py_type(&self, _: &VM<'h, impl ResourceTracker>) -> Type {
        Type::CallableIterator
    }

    fn py_len(&self, _: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let self_id = self_id.expect("heap values have an id");
        vm.heap.inc_ref(self_id);
        Ok(Value::Ref(self_id))
    }

    /// Calls `callable()` and yields the result unless it `==` `sentinel`.
    ///
    /// The heap borrow is resolved to owned clones and released BEFORE the call:
    /// `callable` re-enters Python and may reach this same iterator through a
    /// nested `next()`, which would alias the `UnsafeCell` if a borrow were held
    /// across it (`iter__reentrant.py` covers exactly that).
    fn py_next(&mut self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        let resolved = {
            let this = self.get(vm.heap);
            if this.done {
                None
            } else {
                Some((
                    this.callable.clone_with_heap(vm.heap),
                    this.sentinel.clone_with_heap(vm.heap),
                ))
            }
        };
        let Some((callable, sentinel)) = resolved else {
            return Ok(None);
        };

        let next = callable_next(callable, sentinel, vm)?;
        if next.is_none() {
            // Exhausted. CPython's `calliter_iternext` `Py_CLEAR`s both refs
            // here, so a spent-but-still-referenced iterator does not pin the
            // callable's captured state; take them out under the borrow and drop
            // once it has ended.
            let (callable, sentinel) = {
                let this = self.get_mut(vm.heap);
                this.done = true;
                (
                    mem::replace(&mut this.callable, Value::None),
                    mem::replace(&mut this.sentinel, Value::None),
                )
            };
            callable.drop_with(vm);
            sentinel.drop_with(vm);
        }
        Ok(next)
    }
}

/// Calls `callable()` once, returning `None` when the result `==` `sentinel`.
///
/// Takes both values by ownership: the caller has already cloned them out of the
/// iterator so that no heap borrow is live across the re-entrant call.
fn callable_next(callable: Value, sentinel: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
    defer_drop!(callable, vm);
    defer_drop!(sentinel, vm);
    let result = vm.evaluate_function("iter(callable, sentinel)", callable, ArgValues::Empty)?;
    let mut result = DropGuard::new(result, vm);
    let (result_ref, vm) = result.as_parts_mut();
    if result_ref.py_eq(sentinel, vm)? {
        // result == sentinel: iterator exhausted; `result` dropped by the guard.
        Ok(None)
    } else {
        Ok(Some(result.into_inner()))
    }
}
