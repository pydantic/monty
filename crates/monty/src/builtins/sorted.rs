//! Implementation of the sorted() builtin function.

use itertools::Itertools;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard},
    intern::Interns,
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
pub fn builtin_sorted(vm: &mut VM<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, key_fn, reverse) = parse_sorted_args(args, vm.heap, vm.interns)?;
    defer_drop!(key_fn, vm);

    let items: Vec<_> = MontyIter::new(iterable, vm.heap, vm.interns)?.collect(vm.heap, vm.interns)?;
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
                    let item = item.clone_with_heap(vm.heap);
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

        sort_indices(&mut indices, compare_values, reverse, vm.heap, vm.interns)?;

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
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Value, Option<Value>, bool)> {
    let (mut positional, kwargs) = args.into_parts();
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    // Extract the single required positional argument
    let positional_len = positional.len();
    let Some(iterable) = positional.next() else {
        positional.drop_with_heap(heap);
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("sorted expected 1 argument, got {positional_len}"),
        )
        .into());
    };

    // Reject extra positional arguments
    if positional.len() > 0 {
        let total = positional_len;
        iterable.drop_with_heap(heap);
        positional.drop_with_heap(heap);
        return Err(
            SimpleException::new_msg(ExcType::TypeError, format!("sorted expected 1 argument, got {total}")).into(),
        );
    }

    // Parse keyword arguments: key and reverse
    let mut iterable_guard = HeapGuard::new(iterable, heap);
    let heap = iterable_guard.heap();
    let mut key_guard = HeapGuard::new(None::<Value>, heap);
    let (key_val, heap) = key_guard.as_parts_mut();
    let mut reverse_guard = HeapGuard::new(None::<Value>, heap);
    let (reverse_val, heap) = reverse_guard.as_parts_mut();

    for (kw_key, value) in kwargs {
        defer_drop!(kw_key, heap);
        let mut value = HeapGuard::new(value, heap);

        let Some(keyword_name) = kw_key.as_either_str(value.heap()) else {
            return Err(ExcType::type_error("keywords must be strings"));
        };

        let key_str = keyword_name.as_str(interns);
        let old = if key_str == "key" {
            key_val.replace(value.into_inner())
        } else if key_str == "reverse" {
            reverse_val.replace(value.into_inner())
        } else {
            return Err(ExcType::type_error(format!(
                "'{key_str}' is an invalid keyword argument for sorted()"
            )));
        };

        old.drop_with_heap(heap);
    }

    // Convert reverse to bool (default false)
    let reverse_val = reverse_guard.into_inner();
    let heap = key_guard.heap();
    let reverse = if let Some(v) = reverse_val {
        let result = v.py_bool(heap, interns);
        v.drop_with_heap(heap);
        result
    } else {
        false
    };

    // Handle key function (None means no key function)
    let key_fn = match key_guard.into_inner() {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with_heap(iterable_guard.heap());
            None
        }
        other => other,
    };

    Ok((iterable_guard.into_inner(), key_fn, reverse))
}
