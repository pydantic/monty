//! Shared sorting utilities for `sorted()` and `list.sort()`.
//!
//! Both `sorted()` and `list.sort()` use index-based sorting: they build
//! a vector of indices `[0, 1, 2, ...]`, sort the indices by comparing the
//! corresponding items (or key values), then rearrange items according to
//! the sorted indices.
//!
//! This module provides [`sort_indices`] for the comparison step and
//! [`apply_permutation`] for the in-place rearrangement step.

use std::cmp::Ordering;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Sorts a vector of values, with optional key function.
pub fn sort_values(
    values: &mut [Value],
    key_fn: Option<&Value>,
    reverse: bool,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    if let Some(f) = key_fn {
        // Sort by key function: compute all the keys, sort an index buffer, then
        // rearrange the original values in-place according to the sorted indices.
        let mut indices = (0..values.len()).collect::<Vec<_>>();
        let keys: Vec<Value> = Vec::with_capacity(values.len());
        defer_drop_mut!(keys, vm);

        for item in values.iter() {
            let item = item.clone_with_heap(vm);
            keys.push(vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))?);
        }

        // 2. Sort indices by comparing key values (or values themselves if no key)
        sort_indices(&mut indices, keys, reverse, vm)?;

        // 3. Rearrange values in-place in the detached buffer.
        apply_permutation(values, &mut indices);

        Ok(())
    } else {
        // With no key function can sort directly on the original array
        let mut sort_result: RunResult<()> = Ok(());
        values.sort_by(|a, b| compare_values(a, b, reverse, &mut sort_result, vm));
        sort_result
    }
}

/// Sorts a vector of indices by comparing items at those positions.
///
/// Compares `values[a]` vs `values[b]` using `py_cmp`, optionally reversing
/// the ordering. If any comparison fails (type error or runtime error), the
/// sort finishes early and the error is returned.
///
/// The `values` slice is typically either the items themselves (no key function)
/// or the pre-computed key values.
pub fn sort_indices(
    indices: &mut [usize],
    values: &[Value],
    reverse: bool,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Result<(), RunError> {
    let mut sort_result: RunResult<()> = Ok(());
    indices.sort_by(|&a, &b| compare_values(&values[a], &values[b], reverse, &mut sort_result, vm));
    sort_result
}

/// Rearranges `items` in-place according to a permutation of indices.
///
/// After calling this, `items[i]` will hold the element that was originally at
/// `items[indices[i]]`. The algorithm chases permutation cycles and swaps
/// elements into their final positions, using O(1) extra memory beyond the
/// `indices` slice (which is mutated to track visited positions).
///
/// The helper is generic so callers can avoid allocating a second buffer when
/// reordering either raw `Value`s or compound structures that already own their
/// contents. Each element is moved at most twice (one swap = two moves), so
/// the total work is O(n) moves while preserving the target permutation.
pub fn apply_permutation<T>(items: &mut [T], indices: &mut [usize]) {
    for i in 0..items.len() {
        if indices[i] == i {
            continue;
        }
        let mut current = i;
        loop {
            let target = indices[current];
            indices[current] = current;
            if target == i {
                break;
            }
            items.swap(current, target);
            current = target;
        }
    }
}

/// Helper for the sort functions which compares two values, handling any exceptions and timeouts.
fn compare_values(
    a: &Value,
    b: &Value,
    reverse: bool,
    sort_result: &mut RunResult<()>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Ordering {
    if sort_result.is_err() {
        // short-circuit if we've already encountered an error in a previous comparison
        return Ordering::Equal;
    }
    if let Err(e) = vm.heap.check_time() {
        *sort_result = Err(e.into());
        return Ordering::Equal;
    }
    let err = match a.py_cmp(b, vm) {
        Ok(Some(ord)) => return if reverse { ord.reverse() } else { ord },
        Ok(None) => ExcType::type_error(format!(
            "'<' not supported between instances of '{}' and '{}'",
            a.py_type(vm),
            b.py_type(vm)
        )),
        Err(e) => e.into(),
    };
    *sort_result = Err(err);
    Ordering::Equal
}
