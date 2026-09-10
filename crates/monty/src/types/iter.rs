//! Shared helpers for Python's iteration protocol.
//!
//! Concrete iterator state lives beside its iterable type. This module only
//! implements the `iter`/`next` builtins and common collection machinery.

use monty_types::{ResourceError, ResourceTracker};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData},
    resource_checks::check_estimated_size,
    types::{PyTrait, callable_iterator::CallableIterator},
    value::{VALUE_SIZE, Value, ValueRead},
};

/// Creates an iterator from the `iter()` constructor call.
///
/// The one-argument form enters the ordinary iteration protocol; the
/// two-argument form creates a callable iterator which stops at its sentinel.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IterArgs { object, sentinel } = IterArgs::from_args(args, vm)?;

    if let Some(sentinel) = sentinel {
        if object.is_callable(vm.heap) {
            let iter = CallableIterator::new(object, sentinel);
            Ok(Value::Ref(vm.heap.allocate(HeapData::CallableIterator(iter))))
        } else {
            object.drop_with(vm);
            sentinel.drop_with(vm);
            Err(ExcType::type_error("iter(v, w): v must be callable"))
        }
    } else {
        let iterator = object.py_iter(vm);
        object.drop_with(vm);
        iterator
    }
}

/// Argument shape for `iter(object, /)` and `iter(object, sentinel, /)`.
#[derive(FromArgs)]
#[from_args(name = "iter", style = unpack)]
struct IterArgs {
    #[from_args(pos_only)]
    object: Value,
    #[from_args(pos_only, default)]
    sentinel: Option<Value>,
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
                // its own, so `max_duration` needs its own poll here.
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
    let iterator = value.py_iter(vm)?;
    collect_python_iterator(iterator, vm)
}

/// Collects every item yielded by an owned iterable into a target collection.
pub(crate) fn collect_owned_iterable<'h, T>(value: Value, vm: &mut VM<'h>) -> RunResult<T>
where
    T: Default + Extend<Value> + DropWithContext<VM<'h>>,
{
    let iterator = value.into_py_iter(vm)?;
    collect_python_iterator(iterator, vm)
}

/// Drains an owned Python iterator with incremental allocation checks.
fn collect_python_iterator<'h, T>(iterator: Value, vm: &mut VM<'h>) -> RunResult<T>
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
    let iterator = value.py_iter(vm)?;
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

/// Implements Python's `next(iterator[, default])` semantics.
pub fn iterator_next(iter_value: &Value, default: Option<Value>, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut default_guard = DropGuard::new(default, vm);
    let vm = default_guard.ctx();

    let Value::Ref(iter_id) = iter_value else {
        return Err(ExcType::type_error_not_iterator(&iter_value.py_type_name(vm)));
    };
    match vm.heap.read(*iter_id).py_next(vm)? {
        Some(item) => Ok(item),
        None => match default_guard.into_inner() {
            Some(default) => Ok(default),
            None => Err(ExcType::stop_iteration()),
        },
    }
}
