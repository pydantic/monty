//! Implementation of the sorted() builtin function.

use itertools::Itertools;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData, HeapGuard},
    resource::ResourceTracker,
    sorting::{apply_permutation, sort_indices},
    types::{List, MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the sorted() builtin function.
///
/// Returns a new sorted list from the items in an iterable.
/// Supports `key` and `reverse` keyword arguments matching Python's
/// `sorted(iterable, /, *, key=None, reverse=False)` signature.
pub fn builtin_sorted(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, key_fn, reverse) = parse_sorted_args(args, vm)?;
    defer_drop!(key_fn, vm);

    let items: Vec<_> = MontyIter::new(iterable, vm)?.collect(vm)?;
    let mut items_guard = HeapGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    {
        // Compute key values if a key function was provided, otherwise we'll sort by the items themselves
        let mut keys_guard;
        let (compare_values, vm) = if let Some(f) = key_fn {
            let keys: Vec<Value> = Vec::with_capacity(items.len());
            // Use a HeapGuard to ensure that if key function evaluation fails partway through,
            // we clean up any keys that were successfully computed
            keys_guard = HeapGuard::new(keys, vm);
            let (keys, vm) = keys_guard.as_parts_mut();
            items
                .iter()
                .map(|item| {
                    let item = item.clone_with_heap(vm);
                    vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))
                })
                .process_results(|keys_iter| keys.extend(keys_iter))?;
            keys_guard.as_parts()
        } else {
            (&*items, vm)
        };

        // Sort indices by comparing key values (or items themselves if no key)
        let len = compare_values.len();
        let mut indices: Vec<usize> = (0..len).collect();

        sort_indices(&mut indices, compare_values, reverse, vm)?;

        // Rearrange items in-place according to the sorted permutation
        apply_permutation(items, &mut indices);
    }

    let (items, vm) = items_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}

/// Parses the arguments for `sorted(iterable, /, *, key=None, reverse=False)`.
///
/// Returns `(iterable, key_fn, reverse)` where `key_fn` is `None` when no key
/// function was provided (or `None` was explicitly passed), and `reverse` defaults
/// to `false`.
fn parse_sorted_args(
    args: ArgValues,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(Value, Option<Value>, bool)> {
    let (mut positional, kwargs) = args.into_parts();

    // Extract the single required positional argument
    let positional_len = positional.len();
    let Some(iterable) = positional.next() else {
        kwargs.drop_with_heap(vm);
        positional.drop_with_heap(vm);
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("sorted expected 1 argument, got {positional_len}"),
        )
        .into());
    };

    // Reject extra positional arguments
    if positional.len() > 0 {
        let total = positional_len;
        kwargs.drop_with_heap(vm);
        iterable.drop_with_heap(vm);
        positional.drop_with_heap(vm);
        return Err(
            SimpleException::new_msg(ExcType::TypeError, format!("sorted expected 1 argument, got {total}")).into(),
        );
    }

    // Parse keyword arguments: key and reverse
    let mut iterable_guard = HeapGuard::new(iterable, vm);
    let vm = iterable_guard.heap();
    let (key_arg, reverse_arg) = kwargs.parse_named_kwargs_pair(
        "sorted",
        "key",
        "reverse",
        vm.heap,
        vm.interns,
        |_func_name, key_str| {
            // CPython currently reuses the list.sort()-style wording here rather than
            // saying "sorted() got ...", so match that exact user-visible message.
            ExcType::type_error_unexpected_keyword("sort", key_str)
        },
    )?;

    // Convert reverse to bool (default false)
    let reverse = if let Some(v) = reverse_arg {
        let result = v.py_bool(vm);
        v.drop_with_heap(vm);
        result
    } else {
        false
    };

    // Handle key function (None means no key function)
    let key_fn = match key_arg {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with_heap(iterable_guard.heap());
            None
        }
        other => other,
    };

    Ok((iterable_guard.into_inner(), key_fn, reverse))
}
