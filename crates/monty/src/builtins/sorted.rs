//! Implementation of the sorted() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData, HeapGuard},
    resource::ResourceTracker,
    sorting::parse_and_sort,
    types::{List, MontyIter},
    value::Value,
};

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
pub fn builtin_sorted(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (mut pos_iter, kwargs) = args.into_parts();
    let pos_count = pos_iter.len();
    if pos_count != 1 {
        pos_iter.drop_with_heap(vm);
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_expected_exact("sorted", 1, pos_count));
    }
    let iterable = pos_iter.next().expect("checked pos_count == 1");

    let items: Vec<_> = MontyIter::new(iterable, vm)?.collect(vm)?;
    let mut items_guard = HeapGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    let sort_args = if kwargs.is_empty() {
        ArgValues::Empty
    } else {
        ArgValues::Kwargs(kwargs)
    };
    parse_and_sort(items, sort_args, vm)?;

    let (items, vm) = items_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}
