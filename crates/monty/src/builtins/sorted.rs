//! Implementation of the sorted() builtin function.

use std::cmp::Ordering;

use itertools::process_results;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard},
    intern::Interns,
    resource::{DepthGuard, ResourceTracker},
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
    defer_drop_mut!(items, vm);

    // Compute key values if a key function was provided
    let mut keys_guard;
    let (keys, vm) = if let Some(f) = key_fn {
        let keys: Vec<Value> = Vec::with_capacity(items.len());
        // Use a HeapGuard to ensure that if key function evaluation fails partway through,
        // we clean up any keys that were successfully computed
        keys_guard = HeapGuard::new(keys, vm);
        let (keys, vm) = keys_guard.as_parts_mut();
        process_results(
            items.iter().map(|item| {
                let item = item.clone_with_heap(vm.heap);
                vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))
            }),
            |keys_iter| keys.extend(keys_iter),
        )?;
        keys_guard.as_parts()
    } else {
        (&*items, vm)
    };

    // Sort indices rather than items directly, so we can use key values for comparison
    let len = items.len();
    let mut indices: Vec<usize> = (0..len).collect();
    let mut sort_error: Option<crate::exception_private::RunError> = None;
    let guard = std::cell::RefCell::new(DepthGuard::default());

    indices.sort_by(|&a, &b| {
        if sort_error.is_some() {
            return Ordering::Equal;
        }
        if let Err(e) = vm.heap.check_time() {
            sort_error = Some(e.into());
            return Ordering::Equal;
        }
        match keys[a].py_cmp(&keys[b], vm.heap, &mut guard.borrow_mut(), vm.interns) {
            Ok(Some(ord)) => {
                if reverse {
                    ord.reverse()
                } else {
                    ord
                }
            }
            Ok(None) => {
                sort_error = Some(ExcType::type_error(format!(
                    "'<' not supported between instances of '{}' and '{}'",
                    keys[a].py_type(vm.heap),
                    keys[b].py_type(vm.heap)
                )));
                Ordering::Equal
            }
            Err(e) => {
                sort_error = Some(e.into());
                Ordering::Equal
            }
        }
    });

    // Check for sort error
    if let Some(err) = sort_error {
        return Err(err);
    }

    // Rearrange items in sorted order using index permutation
    let mut sorted_items: Vec<Value> = Vec::with_capacity(len);
    for &i in &indices {
        sorted_items.push(std::mem::replace(&mut items[i], Value::Undefined));
    }

    let heap_id = vm.heap.allocate(HeapData::List(List::new(sorted_items)))?;
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
