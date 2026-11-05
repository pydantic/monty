//! Implementation of the sorted() builtin function.

use monty_types::ResourceTracker;

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, Heap, HeapData},
    resource_checks::check_estimated_size,
    sorting::parse_and_sort,
    types::{List, iter::collect_owned_iterable},
    value::{VALUE_SIZE, Value},
};

fn clone_slice_with_checks(
    slice: &[Value],
    tracker: &ResourceTracker,
    heap: &Heap,
) -> Result<Vec<Value>, Box<(Vec<Value>, RunError)>> {
    let len = slice.len();
    if let Err(e) = check_estimated_size(len.saturating_mul(VALUE_SIZE), tracker) {
        return Err(Box::new((Vec::new(), e.into())));
    }
    let mut cloned = Vec::with_capacity(len);
    for (i, v) in slice.iter().enumerate() {
        if let Err(e) = tracker.check_time_every(i) {
            return Err(Box::new((cloned, e.into())));
        }
        cloned.push(v.clone_with_heap(heap));
    }
    Ok(cloned)
}

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable. CPython's
/// `sorted(iterable, /, *, key=None, reverse=False)` is implemented by
/// converting the iterable to a list and delegating to `list.sort`, so
/// the kwargs and any kwarg-related errors are owned by `sort`. We mirror
/// that by extracting the iterable positionally and handing the rest off
/// to [`parse_and_sort`] — the same entry point `list.sort` uses — so
/// unknown-kwarg errors uniformly read `sort() got an unexpected keyword
/// argument 'X'` without any wording overrides.
pub fn builtin_sorted(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos_iter, kwargs) = args.into_parts();
    let pos_count = pos_iter.len();
    if pos_count != 1 {
        pos_iter.drop_with(vm);
        kwargs.drop_with(vm);
        return Err(ExcType::type_error_expected_exact("sorted", 1, pos_count));
    }
    let iterable = pos_iter.next().expect("checked pos_count == 1");

    let items = if let Value::Ref(id) = &iterable
        && let HeapData::List(list) = vm.heap.get(*id)
    {
        let res = clone_slice_with_checks(list.as_slice(), &vm.heap.tracker, vm.heap);
        iterable.drop_with(vm);
        match res {
            Ok(items) => items,
            Err(boxed) => {
                let (partial, err) = *boxed;
                partial.drop_with(vm);
                kwargs.drop_with(vm);
                return Err(err);
            }
        }
    } else if let Value::Ref(id) = &iterable
        && let HeapData::Tuple(tuple) = vm.heap.get(*id)
    {
        let res = clone_slice_with_checks(tuple.as_slice(), &vm.heap.tracker, vm.heap);
        iterable.drop_with(vm);
        match res {
            Ok(items) => items,
            Err(boxed) => {
                let (partial, err) = *boxed;
                partial.drop_with(vm);
                kwargs.drop_with(vm);
                return Err(err);
            }
        }
    } else {
        match collect_owned_iterable(iterable, vm) {
            Ok(items) => items,
            Err(err) => {
                kwargs.drop_with(vm);
                return Err(err);
            }
        }
    };
    let mut items_guard = DropGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    let sort_args = if kwargs.is_empty() {
        ArgValues::Empty
    } else {
        ArgValues::Kwargs(kwargs)
    };
    parse_and_sort(items, sort_args, vm)?;

    let (items, vm) = items_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)));
    Ok(Value::Ref(heap_id))
}
