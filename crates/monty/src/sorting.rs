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
    exception_private::{ExcType, RunError},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

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
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> Result<(), RunError> {
    let mut sort_error: Option<RunError> = None;

    indices.sort_by(|&a, &b| {
        if sort_error.is_some() {
            return Ordering::Equal;
        }
        if let Err(e) = heap.check_time() {
            sort_error = Some(e.into());
            return Ordering::Equal;
        }
        match values[a].py_cmp(&values[b], heap, interns) {
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
                    values[a].py_type(heap),
                    values[b].py_type(heap)
                )));
                Ordering::Equal
            }
            Err(e) => {
                sort_error = Some(e.into());
                Ordering::Equal
            }
        }
    });

    match sort_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Rearranges `items` in-place according to a permutation of indices.
///
/// After calling this, `items[i]` will hold the value that was originally at
/// `items[indices[i]]`. The algorithm chases permutation cycles and swaps
/// elements into their final positions, using O(1) extra memory beyond the
/// `indices` slice (which is mutated to track visited positions).
///
/// Each element is moved at most twice (one swap = two moves), so the total
/// work is O(n) moves. This is at most 2x the moves of building a fresh
/// `Vec`, but avoids allocating a second buffer.
pub fn apply_permutation(items: &mut [Value], indices: &mut [usize]) {
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
