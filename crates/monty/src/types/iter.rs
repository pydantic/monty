//! Shared helpers for Python's iteration protocol.
//!
//! Concrete iterator state lives beside its iterable type. This module only
//! implements the `iter`/`next` builtins and common collection machinery.

use monty_types::{ResourceError, ResourceTracker};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, HeapData},
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
            Ok(Value::Ref(vm.heap.allocate(HeapData::CallableIterator(iter))?))
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

/// Validates and clamps an iterator capacity hint before native allocation.
pub(crate) fn checked_preallocation_hint(
    hint: usize,
    elem_size: usize,
    tracker: &ResourceTracker,
) -> Result<usize, ResourceError> {
    const MAX_PREALLOCATION_HINT: usize = 65_536;

    check_estimated_size(hint.saturating_mul(elem_size), tracker)?;
    Ok(hint.min(MAX_PREALLOCATION_HINT))
}

/// Adapts a retained Python iterator to Rust's `Iterator` with memory checks.
struct HeapedIterator<'this, 'h, I: CollectIter<'h>> {
    iter: &'this mut I,
    vm: &'this mut VM<'h>,
    yielded: usize,
}

impl<'h, I: CollectIter<'h>> Iterator for HeapedIterator<'_, 'h, I> {
    type Item = RunResult<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next_value(self.vm) {
            Ok(None) => None,
            Err(error) => Some(Err(error)),
            Ok(Some(value)) => {
                self.yielded += 1;
                let estimated = self.yielded.saturating_mul(VALUE_SIZE);
                match check_estimated_size(estimated, self.vm.heap.tracker()) {
                    Ok(()) => Some(Ok(value)),
                    Err(error) => Some(Err(error.into())),
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.iter.remaining(self.vm);
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
pub(crate) fn collect_owned_iterable<T: FromIterator<Value>>(value: Value, vm: &mut VM<'_>) -> RunResult<T> {
    let iterator = value.into_py_iter(vm)?;
    collect_python_iterator(iterator, vm)
}

/// Drains an owned Python iterator with incremental allocation checks.
fn collect_python_iterator<T: FromIterator<Value>>(iterator: Value, vm: &mut VM<'_>) -> RunResult<T> {
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    HeapedIterator {
        iter: &mut iterator,
        vm,
        yielded: 0,
    }
    .collect()
}

/// Pulls at most `limit` items from an iterable, stopping early.
pub fn collect_iterable_bounded(value: &Value, limit: usize, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let iterator = value.py_iter(vm)?;
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    let mut items = Vec::new();
    while items.len() < limit {
        match iterator.py_next(vm) {
            Ok(Some(value)) => items.push(value),
            Ok(None) => break,
            Err(error) => {
                for item in items {
                    item.drop_with(vm);
                }
                return Err(error);
            }
        }
    }
    Ok(items)
}

/// Implements Python's `next(iterator[, default])` semantics.
pub fn iterator_next(iter_value: &Value, default: Option<Value>, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut default_guard = DropGuard::new(default, vm);
    let vm = default_guard.ctx();

    let Value::Ref(iter_id) = iter_value else {
        return Err(ExcType::type_error_not_iterator(&iter_value.py_type_name(vm)));
    };
    match vm.heap.read(*iter_id).py_next(Some(*iter_id), vm)? {
        Some(item) => Ok(item),
        None => match default_guard.into_inner() {
            Some(default) => Ok(default),
            None => Err(ExcType::stop_iteration()),
        },
    }
}
