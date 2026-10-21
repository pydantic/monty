//! Stable sorting through Python's fallible `<` protocol.
//!
//! Comparisons only reorder indices; values stay owned by the caller until a
//! successful sort applies the permutation. No total-order contract is imposed
//! on user comparisons, which may be inconsistent, mutate objects, or raise.

use std::mem::size_of_val;

use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::RunResult,
    value::Value,
};

/// Maximum sort length using inline indices and no merge scratch buffer.
const INSERTION_SORT_THRESHOLD: usize = 8;

/// Shared `sorted`/`list.sort` keywords. The `sort` name matches CPython's
/// argument errors, and `LaxBool` truth-tests rather than type-checks `reverse`.
#[derive(FromArgs)]
#[from_args(name = "sort")]
struct ListSortArgs {
    #[from_args(kw_only, default)]
    key: Option<Value>,
    #[from_args(kw_only, default = LaxBool::new(false))]
    reverse: LaxBool,
}

/// Parses sorting keywords and sorts detached items in place.
pub fn parse_and_sort(items: &mut [Value], args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
    let ListSortArgs { key, reverse } = ListSortArgs::from_args(args, vm)?;
    let key_fn = match key {
        Some(Value::None) => None,
        other => other,
    };
    defer_drop!(key_fn, vm);
    sort_values(items, key_fn.as_ref(), reverse.bool(), vm)
}

/// Sorts values stably, evaluating each optional key once in input order.
/// Comparisons and key calls may fail without losing any owned values.
pub fn sort_values(values: &mut [Value], key_fn: Option<&Value>, reverse: bool, vm: &mut VM<'_>) -> RunResult<()> {
    if values.len() > INSERTION_SORT_THRESHOLD {
        vm.heap
            .tracker
            .check_allocation(values.len().saturating_mul(size_of::<usize>()))?;
    }
    let mut indices = SmallVec::<[usize; INSERTION_SORT_THRESHOLD]>::with_capacity(values.len());
    indices.extend(0..values.len());
    if let Some(f) = key_fn {
        let keys: Vec<Value> = Vec::with_capacity(values.len());
        defer_drop_mut!(keys, vm);
        for (i, item) in values.iter().enumerate() {
            // Re-entry restarts the VM countdown, so short keys need this poll.
            vm.heap.tracker.check_time_every(i)?;
            let item = item.clone_with_heap(vm);
            keys.push(vm.evaluate_function(f, ArgValues::One(item))?);
        }
        sort_indices(&mut indices, keys, reverse, vm)?;
    } else {
        sort_indices(&mut indices, values, reverse, vm)?;
    }
    apply_permutation(values, &mut indices);
    Ok(())
}

/// Sorts indices using only truth-tested `<`, stopping on the first exception.
/// Reversing input and output keeps descending ties stable without calling `>`.
pub fn sort_indices(indices: &mut [usize], values: &[Value], reverse: bool, vm: &mut VM<'_>) -> RunResult<()> {
    if indices.len() < 2 {
        return Ok(());
    }
    if reverse {
        indices.reverse();
    }
    if indices.len() <= INSERTION_SORT_THRESHOLD {
        insertion_sort_indices(indices, values, vm)?;
    } else {
        merge_sort_indices(indices, values, vm)?;
    }
    if reverse {
        indices.reverse();
    }
    Ok(())
}

/// Insertion-sorts small index buffers without allocating or moving owned values.
fn insertion_sort_indices(indices: &mut [usize], values: &[Value], vm: &mut VM<'_>) -> RunResult<()> {
    for unsorted in 1..indices.len() {
        let mut current = unsorted;
        while current > 0 && values[indices[current]].py_lt_operator(&values[indices[current - 1]], vm)? {
            indices.swap(current, current - 1);
            current -= 1;
        }
    }
    Ok(())
}

/// Merge-sorts larger index buffers with periodic time checks during comparisons.
fn merge_sort_indices(indices: &mut [usize], values: &[Value], vm: &mut VM<'_>) -> RunResult<()> {
    vm.heap.tracker.check_allocation(size_of_val(indices))?;
    let mut scratch = indices.to_vec();
    let mut comparisons = 0;
    let mut width = 1;
    while width < indices.len() {
        for start in (0..indices.len()).step_by(width.saturating_mul(2)) {
            let mid = (start + width).min(indices.len());
            let end = mid.saturating_add(width).min(indices.len());
            let (mut left, mut right) = (start, mid);
            for slot in &mut scratch[start..end] {
                let take_right = if left == mid {
                    true
                } else if right == end {
                    false
                } else {
                    comparisons += 1;
                    vm.heap.tracker.check_time_every(comparisons)?;
                    values[indices[right]].py_lt_operator(&values[indices[left]], vm)?
                };
                *slot = if take_right {
                    let index = indices[right];
                    right += 1;
                    index
                } else {
                    let index = indices[left];
                    left += 1;
                    index
                };
            }
        }
        indices.copy_from_slice(&scratch);
        width = width.saturating_mul(2);
    }
    Ok(())
}

/// Applies a permutation in place, mutating the index buffer into the identity.
/// Swaps preserve ownership even when items carry interpreter references.
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
