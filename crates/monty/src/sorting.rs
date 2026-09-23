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

use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    types::{CmpOrder, PyTrait},
    value::Value,
};

/// Length of the runs the sort builds by insertion before it starts merging.
const INSERTION_RUN: usize = 16;

/// Index buffer for the sort, inline for the short sequences most calls pass.
type IndexBuffer = SmallVec<[usize; INSERTION_RUN]>;

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
///
/// `key_context` names the calling builtin in rejected-suspension errors.
pub fn parse_and_sort(
    key_context: &'static str,
    items: &mut [Value],
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let ListSortArgs { key, reverse } = ListSortArgs::from_args(args, vm)?;
    let key_fn = match key {
        Some(v) if matches!(v, Value::None) => {
            v.drop_with(vm);
            None
        }
        other => other,
    };
    defer_drop!(key_fn, vm);
    sort_values(key_context, items, key_fn.as_ref(), reverse.bool(), vm)
}

/// Sorts a vector of values, with optional key function.
/// `key_context` names the calling builtin — see [`parse_and_sort`].
pub fn sort_values(
    key_context: &'static str,
    values: &mut [Value],
    key_fn: Option<&Value>,
    reverse: bool,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    // The index buffer and the merge sort's scratch copy of it are both this size.
    vm.heap
        .tracker
        .check_allocation(values.len().saturating_mul(2 * size_of::<usize>()))?;
    let mut indices = (0..values.len()).collect::<IndexBuffer>();
    if let Some(f) = key_fn {
        // Sort by key function: compute all the keys, sort an index buffer, then
        // rearrange the original values in-place according to the sorted indices.
        let keys: Vec<Value> = Vec::with_capacity(values.len());
        defer_drop_mut!(keys, vm);

        // Each key call re-enters `run()` with a fresh dispatch countdown, so a
        // short key reaches no checkpoint: this is the pass's only clock poll.
        for (i, item) in values.iter().enumerate() {
            vm.heap.tracker.check_time_every(i)?;
            let item = item.clone_with_heap(vm);
            keys.push(vm.evaluate_function(key_context, f, ArgValues::One(item))?);
        }

        sort_indices(&mut indices, keys, reverse, vm)?;
    } else {
        sort_indices(&mut indices, values, reverse, vm)?;
    }

    // A failed sort returns above, leaving the values in their original order like CPython.
    apply_permutation(values, &mut indices);

    Ok(())
}

/// Sorts a vector of indices by comparing items at those positions.
///
/// Compares `values[a]` vs `values[b]` using `py_cmp`, optionally reversing
/// the ordering. If any comparison fails (type error or runtime error), the
/// sort finishes early and the error is returned.
///
/// The `values` slice is typically either the items themselves (no key function)
/// or the pre-computed key values.
pub fn sort_indices(indices: &mut [usize], values: &[Value], reverse: bool, vm: &mut VM<'_>) -> Result<(), RunError> {
    let mut n = 0usize;
    merge_sort_indices(indices, |a, b| {
        n += 1;
        compare_values(n, &values[a], &values[b], reverse, vm)
    })
}

/// Stable merge sort for an index buffer, with a fallible comparator.
///
/// `slice::sort_by` panics when it catches a comparator contradicting itself, which a
/// Python comparison does whenever a `NaN` is involved, so no standard sort can serve
/// [`sort_indices`]. This one validates nothing and returns at the first failed compare.
/// Short runs are insertion-sorted in place first, like CPython's minruns, so an ordinary
/// small sort never merges or allocates.
pub(crate) fn merge_sort_indices(
    indices: &mut [usize],
    mut compare: impl FnMut(usize, usize) -> RunResult<Ordering>,
) -> RunResult<()> {
    for start in (0..indices.len()).step_by(INSERTION_RUN) {
        let end = start.saturating_add(INSERTION_RUN).min(indices.len());
        insertion_sort_run(&mut indices[start..end], &mut compare)?;
    }
    if indices.len() <= INSERTION_RUN {
        return Ok(());
    }
    let mut scratch = IndexBuffer::from_slice(indices);
    let mut width = INSERTION_RUN;
    while width < indices.len() {
        let mut start = 0usize;
        while start < indices.len() {
            let mid = start.saturating_add(width).min(indices.len());
            let end = start.saturating_add(width.saturating_mul(2)).min(indices.len());
            merge_runs(
                &indices[start..mid],
                &indices[mid..end],
                &mut scratch[start..end],
                &mut compare,
            )?;
            start = end;
        }
        indices.copy_from_slice(&scratch);
        width = width.saturating_mul(2);
    }
    Ok(())
}

/// Sorts one short run in place, shifting each index back past the greater ones.
fn insertion_sort_run(
    indices: &mut [usize],
    compare: &mut impl FnMut(usize, usize) -> RunResult<Ordering>,
) -> RunResult<()> {
    for i in 1..indices.len() {
        let mut j = i;
        // Stops at the first index that is not greater, so equal indices keep their order.
        while j > 0 && compare(indices[j], indices[j - 1])? == Ordering::Less {
            indices.swap(j, j - 1);
            j -= 1;
        }
    }
    Ok(())
}

/// Merges two sorted runs into `out`, preferring `left` on ties to keep the sort stable.
fn merge_runs(
    left: &[usize],
    right: &[usize],
    out: &mut [usize],
    compare: &mut impl FnMut(usize, usize) -> RunResult<Ordering>,
) -> RunResult<()> {
    let (mut i, mut j) = (0usize, 0usize);
    for slot in out {
        let take_right = i == left.len() || (j < right.len() && compare(right[j], left[i])? == Ordering::Less);
        *slot = if take_right {
            j += 1;
            right[j - 1]
        } else {
            i += 1;
            left[i - 1]
        };
    }
    Ok(())
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
/// `n` is the caller's running comparison count, keying the amortized time check.
fn compare_values(n: usize, a: &Value, b: &Value, reverse: bool, vm: &mut VM<'_>) -> RunResult<Ordering> {
    vm.heap.tracker.check_time_every(n)?;
    match a.py_cmp(b, vm)? {
        CmpOrder::Ordered(ord) => Ok(if reverse { ord.reverse() } else { ord }),
        // A `NaN` (or `NaN`-carrying container) has no ordering but must not
        // raise: CPython's `sorted`/`list.sort` leave such elements wherever the
        // comparisons happen to place them. Treat it as "equal" — no swap.
        CmpOrder::Unordered => Ok(Ordering::Equal),
        CmpOrder::Incomparable => Err(ExcType::type_error_ordering(
            "<",
            &a.py_type_name(vm),
            &b.py_type_name(vm),
        )),
    }
}
