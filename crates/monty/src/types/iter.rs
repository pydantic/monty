//! Shared helpers for Python's iteration protocol.
//!
//! Concrete iterator state lives beside its iterable type. This module only
//! provides suspendable iterator validation and synchronous collection machinery.

use monty_types::{ResourceError, ResourceTracker};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapReadOutput},
    resource_checks::check_estimated_size,
    types::PyTrait,
    value::{VALUE_SIZE, Value, ValueRead},
};

/// Validates a user `__iter__` result after any suspension in the method.
pub(crate) const ITER_SOURCE: &str = r"
def __monty_iter(object, get_iter, validate):
    return validate(get_iter(object))
";

/// Invokes the user protocol without re-entering the instance's validation frame.
pub(crate) fn call_get_iter(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let object = args.get_one_arg("get_iter", vm.heap)?;
    defer_drop!(object, vm);
    let Some(HeapReadOutput::Instance(instance)) = object.read_heap(vm) else {
        unreachable!("only instances need frozen iterator validation")
    };
    instance.call_iter(vm)
}

/// Returns an owned iterator unchanged, rejecting invalid user `__iter__` results.
pub(crate) fn call_validate_iterator(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let iterator = args.get_one_arg("validate_iterator", vm.heap)?;
    if iterator.py_is_iterator(vm) {
        Ok(iterator)
    } else {
        let error = ExcType::type_error_iter_returned_non_iterator(&iterator.py_type_name(vm));
        iterator.drop_with(vm);
        Err(error)
    }
}

/// Largest iterator hint used to preallocate a native collection.
const MAX_PREALLOCATION_HINT: usize = 65_536;

/// Validates and clamps an iterator capacity hint before native allocation.
pub(crate) fn checked_preallocation_hint(
    hint: usize,
    elem_size: usize,
    tracker: &ResourceTracker,
) -> Result<usize, ResourceError> {
    let hint = hint.min(MAX_PREALLOCATION_HINT);
    check_estimated_size(hint.saturating_mul(elem_size), tracker)?;
    Ok(hint)
}

/// Adapts a retained Python iterator to Rust's `Iterator` with memory checks.
struct HeapedIterator<'this, 'h, I: CollectIter<'h>> {
    iter: &'this mut I,
    vm: &'this mut VM<'h>,
    error: &'this mut Option<RunError>,
    preallocation_hint: usize,
    yielded: usize,
}

impl<'h, I: CollectIter<'h>> Iterator for HeapedIterator<'_, 'h, I> {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next_value(self.vm) {
            Ok(None) => None,
            Err(error) => {
                *self.error = Some(error);
                None
            }
            Ok(Some(value)) => {
                self.yielded += 1;
                let estimated = self.yielded.saturating_mul(VALUE_SIZE);
                // Size alone does not bound a source whose items are cheap or
                // interned, and the drain reaches no VM dispatch checkpoint of
                // its own, so the time limits need their own poll here.
                let checked = check_estimated_size(estimated, &self.vm.heap.tracker)
                    .and_then(|()| self.vm.heap.tracker.check_time_every(self.yielded));
                match checked {
                    Ok(()) => Some(value),
                    Err(error) => {
                        *self.error = Some(error.into());
                        value.drop_with(self.vm);
                        None
                    }
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.iter.remaining(self.vm).min(self.preallocation_hint);
        (remaining, Some(remaining))
    }
}

/// Interface used to drain a retained Python iterator through Rust's `collect()`.
trait CollectIter<'h> {
    /// Advances the iterator by one item.
    fn next_value(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>>;

    /// Returns an internal remaining-length hint when available.
    fn remaining(&self, vm: &VM<'h>) -> usize;
}

impl<'h> CollectIter<'h> for ValueRead<'h, '_> {
    fn next_value(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_next(vm)
    }

    fn remaining(&self, vm: &VM<'h>) -> usize {
        self.iter_size_hint(vm)
    }
}

/// Collects every item yielded by an iterable into a `Vec`.
pub fn collect_iterable(value: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let iterator = value.py_iter(vm)?.require_synchronous(vm)?;
    collect_python_iterator(iterator, vm)
}

/// Collects every item yielded by an owned iterable into a target collection.
pub(crate) fn collect_owned_iterable<'h, T>(value: Value, vm: &mut VM<'h>) -> RunResult<T>
where
    T: Default + Extend<Value> + DropWithContext<VM<'h>>,
{
    let iterator = value.into_py_iter(vm)?.require_synchronous(vm)?;
    collect_python_iterator(iterator, vm)
}

/// Drains an owned Python iterator with incremental allocation checks.
pub(crate) fn collect_python_iterator<'h, T>(iterator: Value, vm: &mut VM<'h>) -> RunResult<T>
where
    T: Default + Extend<Value> + DropWithContext<VM<'h>>,
{
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    let preallocation_hint = checked_preallocation_hint(iterator.iter_size_hint(vm), VALUE_SIZE, &vm.heap.tracker)?;
    let mut values_guard = DropGuard::new(T::default(), vm);
    let (values, vm) = values_guard.as_parts_mut();
    let mut error = None;
    values.extend(HeapedIterator {
        iter: &mut iterator,
        vm,
        error: &mut error,
        preallocation_hint,
        yielded: 0,
    });
    if let Some(error) = error {
        Err(error)
    } else {
        Ok(values_guard.into_inner())
    }
}

/// Pulls at most `limit` items from an iterable, stopping early.
pub fn collect_iterable_bounded(value: &Value, limit: usize, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let iterator = value.py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    let mut items_guard = DropGuard::new(Vec::new(), vm);
    let (items, vm) = items_guard.as_parts_mut();
    while items.len() < limit {
        match iterator.py_next(vm)? {
            Some(value) => items.push(value),
            None => break,
        }
    }
    Ok(items_guard.into_inner())
}
