//! Shared sorting utilities for `sorted()`, `list.sort()` and other sorts over Python values.
//!
//! Sorts use [`timsort`], a port of CPython's algorithm. Most are index-based: they build a vector of indices
//! `[0, 1, 2, ...]`, sort the indices by comparing the corresponding items (or key values), then rearrange the items
//! according to the sorted indices. Short lists without a key are sorted in place.
//!
//! Lists of plain numbers or of short strings skip the port: their `<` is a total order, so any stable sort leaves them
//! in CPython's order, and Rust's is faster.
//!
//! This module provides [`sort_indices`] and [`sort_indices_by`] for the comparison step and
//! [`apply_permutation`] for the in-place rearrangement step.

mod timsort;

use std::cmp::Ordering;

use smallvec::SmallVec;
use timsort::{SortAborted, sort_short, timsort};

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{RunError, RunResult},
    expressions::CmpOperator,
    heap::HeapData,
    types::{CmpOrder, PyTrait, long_int::i64_cmp_f64},
    value::Value,
};

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

/// Sorts `values` in place, ordering by `key_fn(value)` when given, else by the values themselves.
///
/// On a failed comparison the values are left partly sorted, as CPython leaves them.
pub fn sort_values(values: &mut [Value], key_fn: Option<&Value>, reverse: bool, vm: &mut VM<'_>) -> RunResult<()> {
    if let Some(f) = key_fn {
        let keys: Vec<Value> = Vec::with_capacity(values.len());
        defer_drop_mut!(keys, vm);

        // Each key call re-enters `run()` with a fresh dispatch countdown, so a
        // short key reaches no checkpoint: this is the pass's only clock poll.
        for (i, item) in values.iter().enumerate() {
            vm.heap.tracker.check_time_every(i)?;
            let item = item.clone_with_heap(vm);
            keys.push(vm.evaluate_function(f, ArgValues::One(item))?);
        }
        let mut indices = identity_permutation(values.len());
        let result = sort_indices(&mut indices, keys, reverse, vm);
        apply_permutation(values, &mut indices);
        result
    } else if let Some(order) = TotalOrder::of(values, vm) {
        let vm: &VM<'_> = vm;
        with_reversal(values, reverse, |values| {
            values.sort_by(|a, b| order.cmp(a, b, vm));
            Ok(())
        })
    } else if values.len() < SHORT_SORT_LEN {
        // Short lists need no merge, so the values are sorted where they are.
        with_reversal(values, reverse, |values| {
            let mut cmp = Comparer::new(vm);
            let sorted = sort_short(values, |a, b| cmp.lt(|vm| py_lt(a, b, vm)));
            cmp.finish(sorted)
        })
    } else {
        let mut indices = identity_permutation(values.len());
        let result = sort_indices(&mut indices, values, reverse, vm);
        apply_permutation(values, &mut indices);
        result
    }
}

/// Stably sorts `indices` so that `values[indices[0]], values[indices[1]], ...` ascend under Python's `<`.
///
/// This is CPython's `list.sort`: the same algorithm making the same `<` calls, so input that is not totally
/// ordered (`NaN`, sets, comparisons that raise) ends in the same order and raises the same error.
/// `values` is typically the items themselves or their precomputed keys.
pub fn sort_indices(indices: &mut [usize], values: &[Value], reverse: bool, vm: &mut VM<'_>) -> Result<(), RunError> {
    if let Some(order) = TotalOrder::of(values, vm) {
        let vm: &VM<'_> = vm;
        with_reversal(indices, reverse, |indices| {
            indices.sort_by(|&a, &b| order.cmp(&values[a], &values[b], vm));
            Ok(())
        })
    } else {
        sort_indices_by(indices, reverse, vm, |a, b, vm| py_lt(&values[a], &values[b], vm))
    }
}

/// Stably sorts `indices` as CPython's `list.sort` would, with `lt(a, b)` standing in for `<` between the elements
/// that ids `a` and `b` name; for elements that are not plain values, such as CPython's `(key, order)` tuples.
pub fn sort_indices_by<'h>(
    indices: &mut [usize],
    reverse: bool,
    vm: &mut VM<'h>,
    mut lt: impl FnMut(usize, usize, &mut VM<'h>) -> RunResult<bool>,
) -> RunResult<()> {
    with_reversal(indices, reverse, |indices| {
        let mut cmp = Comparer::new(vm);
        let sorted = timsort(indices, |a, b| cmp.lt(|vm| lt(a, b, vm)));
        cmp.finish(sorted)
    })
}

/// Lists shorter than this are sorted by [`sort_short`], without an index buffer.
const SHORT_SORT_LEN: usize = 64;

/// The ids `0..len` in order, the starting point of an index sort; inline for the short lists most sorts see.
fn identity_permutation(len: usize) -> SmallVec<[usize; 32]> {
    (0..len).collect()
}

/// Runs `sort` with `items` reversed before and after when `reverse` is set, which is how CPython sorts in reverse
/// while keeping equal elements in their original order. The second reversal happens even when the sort fails.
fn with_reversal<T>(items: &mut [T], reverse: bool, sort: impl FnOnce(&mut [T]) -> RunResult<()>) -> RunResult<()> {
    if reverse {
        items.reverse();
    }
    let result = sort(items);
    if reverse {
        items.reverse();
    }
    result
}

/// Element kinds whose `<` is a total order that runs no user code and cannot raise.
///
/// A stable sort under such an order has only one possible result, CPython's, and how many comparisons it made
/// cannot be observed, so these sorts may use Rust's faster `sort_by`.
#[derive(Clone, Copy)]
enum TotalOrder {
    /// Every element is a [`Number`] other than `NaN`.
    Numbers,
    /// Every element is a `str` of at most [`MAX_FAST_STR_LEN`] bytes.
    Strs,
}

/// Longer strings sort through [`timsort`], whose comparisons poll the clock: `sort_by` cannot stop early, and
/// comparing many references to one long string would otherwise run unbounded work past the time limit.
const MAX_FAST_STR_LEN: usize = 256;

impl TotalOrder {
    /// The total order `values` share, if any: one pass over them before a sort.
    fn of(values: &[Value], vm: &VM<'_>) -> Option<Self> {
        let first = values.first()?;
        if Number::of(first).is_some() {
            let is_ordered = |v: &Value| Number::of(v).is_some_and(|n| !n.is_nan());
            values.iter().all(is_ordered).then_some(Self::Numbers)
        } else {
            let is_short = |v: &Value| str_of(v, vm).is_some_and(|s| s.len() <= MAX_FAST_STR_LEN);
            values.iter().all(is_short).then_some(Self::Strs)
        }
    }

    /// Compares two elements of a list that [`TotalOrder::of`] accepted.
    #[expect(
        clippy::inline_always,
        reason = "runs once per comparison; inlining it measurably speeds up sorts"
    )]
    #[inline(always)]
    fn cmp(self, a: &Value, b: &Value, vm: &VM<'_>) -> Ordering {
        // The `None`s cannot happen: `of` checked every element.
        let ordering = match self {
            Self::Numbers => Number::of(a).zip(Number::of(b)).and_then(|(a, b)| a.partial_cmp(b)),
            Self::Strs => str_of(a, vm).zip(str_of(b, vm)).map(|(a, b)| a.cmp(b)),
        };
        ordering.unwrap_or(Ordering::Equal)
    }
}

/// The comparison a sort makes, Python's `<`, with the sort's time checks.
///
/// A failed comparison is kept here and the sort told only to stop, which keeps each comparison's result small.
struct Comparer<'v, 'h> {
    vm: &'v mut VM<'h>,
    /// Comparisons made so far, spacing the time checks.
    count: usize,
    error: Option<RunError>,
}

impl<'v, 'h> Comparer<'v, 'h> {
    fn new(vm: &'v mut VM<'h>) -> Self {
        Self {
            vm,
            count: 0,
            error: None,
        }
    }

    /// Answers `a < b` by running `lt`, or stops the sort with the error stashed.
    #[expect(
        clippy::inline_always,
        reason = "runs once per comparison; inlining it measurably speeds up sorts"
    )]
    #[inline(always)]
    fn lt(&mut self, lt: impl FnOnce(&mut VM<'h>) -> RunResult<bool>) -> Result<bool, SortAborted> {
        self.count += 1;
        let lt = match self.vm.heap.tracker.check_time_every(self.count) {
            Ok(()) => lt(self.vm),
            Err(e) => Err(e.into()),
        };
        lt.map_err(|e| {
            self.error = Some(e);
            SortAborted
        })
    }

    /// Turns the sort's outcome back into the error that stopped it, if any.
    fn finish(self, sorted: Result<(), SortAborted>) -> RunResult<()> {
        sorted.map_err(|SortAborted| {
            self.error
                .expect("only `Comparer::lt` aborts a sort, and it stashes its error first")
        })
    }
}

/// Python's `a < b`, the only comparison the sort makes.
///
/// Only heap values other than `str` can answer `<` themselves (sets, `Counter`, `Decimal`), so the rest skip
/// [`VM::cmp_values`]'s rich-comparison hooks; the common pairs are answered inline.
#[expect(
    clippy::inline_always,
    reason = "runs once per comparison; inlining it measurably speeds up sorts"
)]
#[inline(always)]
pub(crate) fn py_lt(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Ok(a < b),
        (Value::Float(a), Value::Float(b)) => Ok(a < b),
        // A `NaN` operand is unordered, so `<` is `False`.
        _ if let (Some(a), Some(b)) = (Number::of(a), Number::of(b)) => Ok(a.partial_cmp(b) == Some(Ordering::Less)),
        _ if let (Some(a), Some(b)) = (str_of(a, vm), str_of(b, vm)) => Ok(a < b),
        (Value::Ref(_), _) | (_, Value::Ref(_)) => vm.cmp_values(CmpOperator::Lt, a, b),
        _ => match a.py_cmp(b, vm)? {
            CmpOrder::Ordered(ordering) => Ok(ordering.is_lt()),
            CmpOrder::Unordered => Ok(false),
            // Raises CPython's `TypeError`.
            CmpOrder::Incomparable => vm.cmp_values(CmpOperator::Lt, a, b),
        },
    }
}

/// Equality as CPython's tuple comparison asks it of an element pair, identity first.
///
/// Numbers compare by value: Monty's immediate floats have no identity, so a `NaN` is never equal to another
/// (see `limitations/language.md`), where [`Value::py_eq`] would call two `NaN`s with the same bits identical.
pub(crate) fn py_eq_item(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match (Number::of(a), Number::of(b)) {
        (Some(a), Some(b)) => Ok(a.partial_cmp(b) == Some(Ordering::Equal)),
        _ => a.py_eq(b, vm),
    }
}

/// A number whose order needs no VM: an `int` that fits in `i64`, a `bool`, or a `float`.
#[derive(Clone, Copy)]
enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    fn of(value: &Value) -> Option<Self> {
        match value {
            Value::Int(i) => Some(Self::Int(*i)),
            Value::Bool(b) => Some(Self::Int(i64::from(*b))),
            Value::Float(f) => Some(Self::Float(*f)),
            _ => None,
        }
    }

    fn is_nan(self) -> bool {
        matches!(self, Self::Float(f) if f.is_nan())
    }

    /// Python's exact numeric order; `None` when either side is `NaN`.
    fn partial_cmp(self, other: Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => Some(a.cmp(&b)),
            (Self::Float(a), Self::Float(b)) => a.partial_cmp(&b),
            (Self::Int(a), Self::Float(b)) => i64_cmp_f64(a, b),
            (Self::Float(a), Self::Int(b)) => i64_cmp_f64(b, a).map(Ordering::reverse),
        }
    }
}

/// The text of a `str` value, interned or on the heap.
fn str_of<'a>(value: &Value, vm: &'a VM<'_>) -> Option<&'a str> {
    match value {
        Value::InternString(id) => Some(vm.interns.get_str(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
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
