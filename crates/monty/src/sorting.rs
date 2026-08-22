//! Shared sorting utilities for `sorted()` and `list.sort()`.
//!
//! Both `sorted()` and `list.sort()` use index-based sorting: they build
//! a vector of indices `[0, 1, 2, ...]`, sort the indices by comparing the
//! corresponding items (or key values), then rearrange items according to
//! the sorted indices.
//!
//! This module provides [`sort_indices`] for the comparison step and
//! [`apply_permutation`] for the in-place rearrangement step.

use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{RunError, RunResult},
    types::RichCmpOp,
    value::Value,
};

/// Maximum index-buffer length sorted without a scratch allocation.
const INSERTION_SORT_THRESHOLD: usize = 8;

/// Argument shape for `list.sort(*, key=None, reverse=False)` and, by
/// extension, the kwargs accepted by the `sorted()` builtin. Both fields
/// are keyword-only (CPython rejects positional `key`/`reverse`). `key` is
/// held as a raw `Option<Value>` so callers can normalise `key=None` to
/// "no key"; `reverse` uses [`LaxBool`] to match CPython's `bool()`-style
/// truth test (so `reverse=[]` is `False`, not a `TypeError`).
#[derive(FromArgs)]
#[from_args(name = "sort")]
struct ListSortArgs {
    #[from_args(kw_only, default)]
    key: Option<Value>,
    #[from_args(kw_only, default = LaxBool::new(false))]
    reverse: LaxBool,
}

/// Parses `key`/`reverse` kwargs and sorts `items` in place. The single
/// entry point for sorting used by both `list.sort` and the `sorted()`
/// builtin — sharing here is what makes unknown-kwarg errors uniformly
/// read `sort() got an unexpected keyword argument 'X'` (matching
/// CPython, whose `sorted` delegates to `list.sort` internally).
pub fn parse_and_sort(items: &mut [Value], args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
    let ListSortArgs { key, reverse } = ListSortArgs::from_args(args, vm)?;
    let key_fn = match key {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with(vm);
            None
        }
        other => other,
    };
    defer_drop!(key_fn, vm);
    sort_values(items, key_fn.as_ref(), reverse.bool(), vm)
}

/// Sorts a vector of values, with optional key function.
pub fn sort_values(values: &mut [Value], key_fn: Option<&Value>, reverse: bool, vm: &mut VM<'_>) -> RunResult<()> {
    let keys = Vec::new();

    defer_drop_mut!(keys, vm);
    let mut indices = (0..values.len()).collect::<SmallVec<[usize; INSERTION_SORT_THRESHOLD]>>();

    let compare_values = if let Some(f) = key_fn {
        // Sort by key function: compute all the keys, sort an index buffer, then
        // rearrange the original values in-place according to the sorted indices.
        keys.reserve(values.len());

        // Each key call re-enters `run()` with a fresh dispatch countdown, so a
        // short key reaches no checkpoint: this is the pass's only clock poll.
        for (i, item) in values.iter().enumerate() {
            vm.heap.tracker.check_time_every(i)?;
            let item = item.clone_with_heap(vm);
            keys.push(vm.evaluate_function("sorted() key argument", f, ArgValues::One(item))?);
        }

        keys.as_slice()
    } else {
        &*values
    };

    sort_indices(&mut indices, compare_values, reverse, vm)?;
    apply_permutation(values, &mut indices);
    Ok(())
}

/// Sorts a vector of indices by comparing items at those positions.
///
/// Compares `values[a] < values[b]`, swapping operands for reverse order.
/// If any comparison fails, the sort finishes early and returns the error.
///
/// The `values` slice is typically either the items themselves (no key function)
/// or the pre-computed key values.
pub fn sort_indices(indices: &mut [usize], values: &[Value], reverse: bool, vm: &mut VM<'_>) -> Result<(), RunError> {
    let mut comparisons = 0;
    if indices.len() <= INSERTION_SORT_THRESHOLD {
        return insertion_sort_indices(indices, values, reverse, &mut comparisons, vm);
    }

    let mut scratch = indices.to_vec();
    let mut width = 1;
    while width < indices.len() {
        let stride = width.saturating_mul(2);
        for start in (0..indices.len()).step_by(stride) {
            let mid = start.saturating_add(width).min(indices.len());
            let end = start.saturating_add(stride).min(indices.len());
            merge_indices(
                indices,
                &mut scratch,
                values,
                start,
                mid,
                end,
                reverse,
                &mut comparisons,
                vm,
            )?;
        }
        indices.copy_from_slice(&scratch);
        width = stride;
    }
    Ok(())
}

/// Stably insertion-sorts a small index buffer using Python's `<` predicate.
fn insertion_sort_indices(
    indices: &mut [usize],
    values: &[Value],
    reverse: bool,
    comparisons: &mut usize,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    for unsorted in 1..indices.len() {
        let mut current = unsorted;
        while current > 0
            && comes_before(
                &values[indices[current]],
                &values[indices[current - 1]],
                reverse,
                comparisons,
                vm,
            )?
        {
            indices.swap(current, current - 1);
            current -= 1;
        }
    }
    Ok(())
}

/// Stably merges adjacent sorted index ranges using Python's `<` predicate.
///
/// The right item moves first only when it is strictly less than the left;
/// equality and partial-order ties retain their original order.
#[expect(clippy::too_many_arguments)]
fn merge_indices(
    indices: &[usize],
    scratch: &mut [usize],
    values: &[Value],
    start: usize,
    mid: usize,
    end: usize,
    reverse: bool,
    comparisons: &mut usize,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let (mut left, mut right) = (start, mid);
    for destination in &mut scratch[start..end] {
        let take_right = if left == mid {
            true
        } else if right == end {
            false
        } else {
            comes_before(
                &values[indices[right]],
                &values[indices[left]],
                reverse,
                comparisons,
                vm,
            )?
        };
        *destination = if take_right {
            let index = indices[right];
            right += 1;
            index
        } else {
            let index = indices[left];
            left += 1;
            index
        };
    }
    Ok(())
}

/// Tests whether `lhs` precedes `rhs` in the requested sort direction.
fn comes_before(lhs: &Value, rhs: &Value, reverse: bool, comparisons: &mut usize, vm: &mut VM<'_>) -> RunResult<bool> {
    *comparisons += 1;
    vm.heap.tracker.check_time_every(*comparisons)?;
    if reverse {
        rhs.py_rich_compare_bool(lhs, RichCmpOp::Lt, vm)
    } else {
        lhs.py_rich_compare_bool(rhs, RichCmpOp::Lt, vm)
    }
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
