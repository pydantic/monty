//! Implementation of the sorted() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{HeapData, HeapGuard},
    resource::ResourceTracker,
    sorting::sort_values,
    types::{List, MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable.
/// Supports `key` and `reverse` keyword arguments matching Python's
/// `sorted(iterable, /, *, key=None, reverse=False)` signature.
pub fn builtin_sorted(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let SortedArgs { iterable, key, reverse } = SortedArgs::from_args(args, vm.heap, vm.interns)?;
    // `key = None` is equivalent to "no key function" — drop the value and
    // pass `None` through so `sort_values` skips the key call.
    let key_fn = match key {
        Value::None => {
            key.drop_with_heap(vm);
            None
        }
        _ => Some(key),
    };
    defer_drop!(key_fn, vm);
    defer_drop!(reverse, vm);
    let reverse = reverse.py_bool(vm);

    let items: Vec<_> = MontyIter::new(iterable, vm)?.collect(vm)?;
    let mut items_guard = HeapGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    sort_values(items, key_fn.as_ref(), reverse, vm)?;

    let (items, vm) = items_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}

/// Argument shape for `sorted(iterable, /, *, key=None, reverse=False)`.
///
/// `name = "sorted"` is used in error messages. `key` and `reverse` are held
/// as `Value` so the caller can treat `key=None` as "no key function" and
/// truthy-evaluate `reverse` without a strict-type check.
#[derive(FromArgs)]
#[from_args(name = "sorted")]
struct SortedArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(default = Value::None)]
    key: Value,
    #[from_args(default = Value::Bool(false))]
    reverse: Value,
}
