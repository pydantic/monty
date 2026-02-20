//! Implementation of the filter() builtin function.
//!
//! This module provides the filter() builtin which filters elements from an iterable
//! based on a predicate function. The implementation supports:
//! - `None` as predicate (filters falsy values)
//! - Builtin functions (len, abs, etc.)
//! - Type constructors (int, str, float, etc.)
//! - User-defined functions (via `vm.evaluate_function`)

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::RunResult,
    heap::{HeapData, HeapGuard},
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the filter() builtin function.
///
/// Filters elements from an iterable based on a predicate function.
/// If the predicate is None, filters out falsy values.
///
/// Note: In Python this returns an iterator, but we return a list for simplicity.
///
/// Examples:
/// ```python
/// filter(lambda x: x > 0, [-1, 0, 1, 2])  # [1, 2]
/// filter(None, [0, 1, False, True, ''])   # [1, True]
/// ```
pub fn builtin_filter(vm: &mut VM<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (function, iterable) = args.get_two_args("filter", vm.heap)?;
    defer_drop!(function, vm);

    let iter = MontyIter::new(iterable, vm.heap, vm.interns)?;
    defer_drop_mut!(iter, vm);

    let out: Vec<Value> = Vec::new();
    let mut out_guard = HeapGuard::new(out, vm);
    let (out, vm) = out_guard.as_parts_mut();

    while let Some(item) = iter.for_next(vm.heap, vm.interns)? {
        let mut item_guard = HeapGuard::new(item, vm);
        let (item, vm) = item_guard.as_parts_mut();
        let should_include = if let Value::None = function {
            // No predicate - use truthiness of element
            item.py_bool(vm.heap, vm.interns)
        } else {
            // Clone for predicate call - the clone is consumed by evaluate_function
            let item_for_predicate = item.clone_with_heap(vm.heap);
            let result = vm.evaluate_function("filter()", function, ArgValues::One(item_for_predicate))?;
            let is_truthy = result.py_bool(vm.heap, vm.interns);
            result.drop_with_heap(vm.heap);
            is_truthy
        };

        if should_include {
            out.push(item_guard.into_inner());
        }
    }

    let (out, vm) = out_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(out)))?;
    Ok(Value::Ref(heap_id))
}
