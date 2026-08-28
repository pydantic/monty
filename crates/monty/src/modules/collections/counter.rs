//! Runtime behaviour for `collections.Counter`.
//!
//! A Counter is a `dict` tagged with a `Counter` [`DictKind`](crate::types::DictKind);
//! operations receive the Counter's heap object handle but re-read its dictionary
//! through the VM whenever Python execution can intervene. This is why the logic
//! lives beside the module surface instead of in `types/dict.rs`. The dict method
//! surface is inherited as-is — only the
//! missing-key read (`0`, no insert), `most_common`/`elements`/`total`/`update`/
//! `subtract`, and the `+ - & |` algebra are added on top.

use std::{cmp::Ordering, mem};

use smallvec::smallvec;

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapObjectRead, HeapReadOutput},
    resource_checks::check_repeat_size,
    types::{Dict, List, PyTrait, allocate_tuple, iter::collect_owned_iterable, py_trait::CmpOrder},
    value::{VALUE_SIZE, Value},
};

/// Adds (or subtracts) `delta` from two counts, with CPython's operator wording
/// when the operand types have no numeric `+`/`-`.
///
/// A Counter is a plain dict, so its values are ordinary Python objects and
/// CPython never coerces them — floats and big ints must survive arithmetic
/// intact. Both operands are borrowed; the result is a fresh owned `Value`.
fn count_arith(lhs: &Value, rhs: &Value, subtract: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    // `py_add`/`py_sub` already raise the exact `binary_type_error(op, ...)` this
    // needs when the counts aren't addable, so delegate rather than re-derive it.
    if subtract {
        lhs.py_sub(rhs, vm)
    } else {
        lhs.py_add(rhs, vm)
    }
}

/// A Python ordering comparison used inside the `Counter` algebra.
///
/// CPython's Counter methods run different comparisons (`<=` for `<=`/`<`, `>=`
/// for `>=`/`>`, `<` when picking a min/max, `>` for the `> 0` keep-positive
/// filter), and the operator embedded in an unorderable-count `TypeError` is the
/// one that actually ran — so the caller picks the variant to keep CPython's
/// wording. It also decides how `NaN` (an *unordered* pair) resolves.
#[derive(Clone, Copy)]
enum CountCmp {
    Le,
    Lt,
    Ge,
    Gt,
}

impl CountCmp {
    /// The operator symbol CPython names in the `TypeError`.
    fn symbol(self) -> &'static str {
        match self {
            Self::Le => "<=",
            Self::Lt => "<",
            Self::Ge => ">=",
            Self::Gt => ">",
        }
    }

    /// Whether `lhs <op> rhs` holds given the three-way `ordering` of the pair.
    fn holds(self, ordering: Ordering) -> bool {
        match self {
            Self::Le => ordering != Ordering::Greater,
            Self::Lt => ordering == Ordering::Less,
            Self::Ge => ordering != Ordering::Less,
            Self::Gt => ordering == Ordering::Greater,
        }
    }
}

/// Evaluates `lhs <op> rhs` between two counts, raising CPython's operator-
/// specific `TypeError` when the pair has no ordering at all.
///
/// `NaN` makes a pair *unordered* rather than incomparable, and every IEEE
/// ordering comparison against `NaN` is false — so `nan <= nan` is `false`, which
/// is exactly what CPython's element-wise `Counter` comparisons observe.
fn count_holds(lhs: &Value, rhs: &Value, op: CountCmp, vm: &mut VM<'_>) -> RunResult<bool> {
    match lhs.py_cmp(rhs, vm)? {
        CmpOrder::Ordered(ordering) => Ok(op.holds(ordering)),
        CmpOrder::Unordered => Ok(false),
        CmpOrder::Incomparable => Err(ExcType::type_error_ordering(
            op.symbol(),
            &lhs.py_type_name(vm),
            &rhs.py_type_name(vm),
        )),
    }
}

/// Orders two counts for *sorting* (`most_common`, Counter repr), raising the
/// sort's `<` `TypeError` when they are incomparable.
///
/// Unlike the algebra's [`count_holds`], a `NaN` pair is treated as **equal**
/// here: Python's `sorted`/`heapq` never raise on `NaN`, they just leave it
/// wherever the (ill-defined for `NaN`) ordering happens to place it.
fn count_sort_cmp(lhs: &Value, rhs: &Value, vm: &mut VM<'_>) -> RunResult<Ordering> {
    match lhs.py_cmp(rhs, vm)? {
        CmpOrder::Ordered(ordering) => Ok(ordering),
        CmpOrder::Unordered => Ok(Ordering::Equal),
        CmpOrder::Incomparable => Err(ExcType::type_error_ordering(
            "<",
            &lhs.py_type_name(vm),
            &rhs.py_type_name(vm),
        )),
    }
}

/// Whether a count is strictly greater than zero (the `Counter` algebra keeps
/// only positive results). CPython's `_keep_positive` filters with `count > 0`,
/// so an unorderable count reports `>` and a `NaN` count is not positive.
fn count_is_positive(value: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    count_holds(value, &Value::Int(0), CountCmp::Gt, vm)
}

/// Converts a count to a repetition length for `elements()`.
///
/// CPython feeds the count to `itertools.repeat`, which requires a whole
/// number, so a float count raises rather than being truncated. Negative counts
/// yield `0` (the element is skipped).
///
/// A count outside `ssize_t` raises `OverflowError` *regardless of sign* —
/// CPython converts before it inspects the sign, so a huge negative count is
/// rejected rather than skipped. That boundary is exactly Monty's `Int`
/// (`i64`) / `LongInt` split, so the type of the count decides it.
fn count_repeat_len(value: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
    match value {
        Value::Int(i) => Ok(usize::try_from(*i).unwrap_or(0)),
        Value::Bool(b) => Ok(usize::from(*b)),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Err(ExcType::overflow_c_ssize_t()),
        Value::InternLongInt(_) => Err(ExcType::overflow_c_ssize_t()),
        other => Err(ExcType::type_error_not_an_integer(&other.py_type_name(vm))),
    }
}

/// Adds `delta` to the count for `key` in `counter` (creating the key at
/// `0 + delta` if absent). Takes ownership of `key`, which the dict keeps
/// on success; `delta` is only read, so the caller retains it.
///
/// `delta_first` selects the operand order for the addition: CPython's mapping
/// update computes `count + self.get(elem, 0)` while element counting computes
/// `self.get(elem, 0) + 1`. The two agree numerically, but the left operand is
/// the one named in a `TypeError`. Ignored when `subtract` is set, which is
/// always `self.get(elem, 0) - count`.
fn counter_bump<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    key: Value,
    delta: &Value,
    subtract: bool,
    delta_first: bool,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    // Either failure below — `dict_get` hashing an unhashable key (e.g.
    // `Counter([[1]])`), or the arithmetic on a non-numeric count — releases the
    // key through the guard; only the store at the end takes it out again.
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();
    let base = counter.dict_get(key, vm)?.unwrap_or(Value::Int(0));
    let total = if subtract || !delta_first {
        count_arith(&base, delta, subtract, vm)
    } else {
        count_arith(delta, &base, false, vm)
    };
    base.drop_with(vm);
    let total = total?;
    let (key, vm) = key_guard.into_parts();
    if let Some(old) = counter.set(key, total, vm)? {
        old.drop_with(vm);
    }
    Ok(())
}

/// Implements `Counter.update`/`subtract` and construction: folds counts from
/// `source` (a mapping adds its values; any other iterable counts occurrences)
/// and from `kwargs` into `counter`. `subtract` negates the deltas.
///
/// Consumes `source` and `kwargs`.
pub(crate) fn counter_update<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    source: Option<Value>,
    kwargs: KwargsValues,
    subtract: bool,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    // An explicit `None` source is a no-op (CPython skips it before dispatch), so
    // `Counter(None)` and `c.update(None)` only apply the keyword counts below.
    if let Some(source) = source
        && !matches!(source, Value::None)
    {
        let is_mapping = matches!(&source, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)));
        if is_mapping {
            let Value::Ref(src_id) = source else {
                unreachable!("mapping is a ref");
            };
            // Snapshot (key, delta) pairs first so `c.update(c)` is well-defined.
            let pairs: Vec<(Value, Value)> = {
                let Some(src) = vm.heap.read_as::<Dict>(src_id) else {
                    unreachable!("mapping is a dict");
                };
                let src = src.get(vm.heap);
                src.iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect()
            };
            source.drop_with(vm);
            counter_merge_mapping(counter, pairs, subtract, vm)?;
        } else {
            // `_count_elements`: `self[elem] = self_get(elem, 0) + 1`, so the
            // existing count is the left operand here.
            let items = collect_owned_iterable::<Vec<Value>>(source, vm)?.into_iter();
            // A bump can fail on an unhashable item; the guard releases the
            // items it never reached.
            defer_drop_mut!(items, vm);
            for item in items.by_ref() {
                counter_bump(counter, item, &Value::Int(1), subtract, false, vm)?;
            }
        }
    }

    // Keyword counts, e.g. `Counter(a=2)` / `c.update(a=5)`. CPython recurses
    // via `self.update(kwds)`, so these follow the mapping path — including the
    // empty-Counter fast path, re-evaluated after the iterable was folded in.
    let kwarg_pairs = counter_kwarg_deltas(kwargs);
    counter_merge_mapping(counter, kwarg_pairs, subtract, vm)?;
    Ok(())
}

/// Folds mapping `(key, count)` pairs into `counter`.
///
/// Mirrors `Counter.update`'s mapping branch: into a **non-empty** Counter each
/// count is added as `count + self.get(elem, 0)` (the incoming count is the left
/// operand, which is what CPython's error messages name), while into an **empty**
/// one CPython takes a `super().update()` fast path that stores values verbatim
/// — which is why `Counter({'a': True})` keeps `True` rather than `1`.
/// `subtract` has no fast path and always computes `self.get(elem, 0) - count`.
fn counter_merge_mapping<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    pairs: Vec<(Value, Value)>,
    subtract: bool,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    let is_empty = counter.get(vm.heap).is_empty();
    // A merge can fail on an unhashable key or a non-numeric count; the guard
    // releases the pairs it never reached.
    let pairs = pairs.into_iter();
    defer_drop_mut!(pairs, vm);
    for (key, count) in pairs.by_ref() {
        // The fast path stores `count` itself, while a bump only reads it — so
        // that branch owns the release.
        if is_empty && !subtract {
            if let Some(old) = counter.set(key, count, vm)? {
                old.drop_with(vm);
            }
        } else {
            let outcome = counter_bump(counter, key, &count, subtract, true, vm);
            count.drop_with(vm);
            outcome?;
        }
    }
    Ok(())
}

/// Converts kwargs into owned `(key_value, count_value)` delta pairs.
fn counter_kwarg_deltas(kwargs: KwargsValues) -> Vec<(Value, Value)> {
    match kwargs {
        KwargsValues::Empty => Vec::new(),
        KwargsValues::Inline(kvs) => kvs.into_iter().map(|(id, v)| (Value::InternString(id), v)).collect(),
        KwargsValues::Pairs(kvs) => kvs,
        KwargsValues::Dict(dict) => dict.into_iter().collect(),
    }
}

/// `Counter.update`/`subtract` method body: binds `iterable=None, /, **kwargs`
/// and folds the counts in. Returns `None`.
///
/// The binding is hand-rolled rather than `#[derive(FromArgs)]`: these are
/// pure-Python `def` methods, so CPython's over-arity error counts the implicit
/// `self` (`Counter.update() takes from 1 to 2 positional arguments but 3 were
/// given`), which the derive's `def` family — positional-only, no receiver —
/// cannot express. The `+ 1`s below re-introduce that `self`.
pub(crate) fn counter_update_method<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    args: ArgValues,
    subtract: bool,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let total = pos.len();
    let source = (total > 0).then(|| pos.next().expect("len checked"));
    // CPython names the specific method (with the `Counter.` qualifier) here.
    let name = if subtract { "Counter.subtract" } else { "Counter.update" };
    if total > 1 {
        source.drop_with(vm);
        pos.drop_with(vm);
        kwargs.drop_with(vm);
        return Err(ExcType::type_error_too_many_positional_range(name, 1, 2, total + 1, 0));
    }
    counter_update(counter, source, kwargs, subtract, vm)?;
    Ok(Value::None)
}

/// `Counter.total()` — the sum of all counts.
///
/// Sums with real numeric addition (CPython's `sum(self.values())`), so a
/// float or big-int count is preserved in the total rather than truncated.
pub(crate) fn counter_total<'h>(counter: &HeapObjectRead<'h, Dict>, vm: &mut VM<'h>) -> RunResult<Value> {
    let counts = counter
        .get(vm.heap)
        .iter()
        .map(|(_, count)| count.clone_with_heap(vm.heap))
        .collect::<Vec<_>>()
        .into_iter();
    // A count that cannot be added raises mid-fold, so both the counts not yet
    // folded in and the running total are held by guards that release them.
    defer_drop_mut!(counts, vm);
    let mut sum_guard = DropGuard::new(Value::Int(0), vm);
    for count in counts.by_ref() {
        let (sum, vm) = sum_guard.as_parts_mut();
        let next = count_arith(sum, &count, false, vm);
        count.drop_with(vm);
        // The previous total is replaced rather than dropped in place, so the
        // guard always owns exactly one value.
        mem::replace(sum, next?).drop_with(vm);
    }
    Ok(sum_guard.into_inner())
}

/// Orders entry indices for `most_common` / Counter repr: count descending,
/// ties in insertion order (the sort is stable).
///
/// Takes the counts by value because the comparison needs `&mut VM` (big ints
/// live on the heap), which callers cannot hold alongside a `Dict` borrow.
/// Consumes `counts`.
pub(crate) fn counter_order(counts: Vec<Value>, vm: &mut VM<'_>) -> RunResult<Vec<usize>> {
    let mut order: Vec<usize> = (0..counts.len()).collect();
    // `sort_by` needs an infallible comparator, so a comparison error is stashed
    // and reported once the sort finishes; the ordering it produces in that case
    // is discarded.
    let mut failure = None;
    order.sort_by(|&a, &b| {
        if failure.is_some() {
            return Ordering::Equal;
        }
        match count_sort_cmp(&counts[b], &counts[a], vm) {
            Ok(ordering) => ordering,
            Err(e) => {
                failure = Some(e);
                Ordering::Equal
            }
        }
    });
    counts.drop_with(vm);
    match failure {
        Some(e) => Err(e),
        None => Ok(order),
    }
}

/// `Counter.most_common([n])` — a list of `(element, count)` tuples ordered by
/// count descending (ties in insertion order). `n` omitted/`None` returns all;
/// a non-positive `n` returns an empty list.
pub(crate) fn counter_most_common<'h>(
    counter: &HeapObjectRead<'h, Dict>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let n_arg = args.get_zero_one_arg("most_common", vm.heap)?;
    // `n` goes through `__index__`, so a bool counts as `0`/`1` and a big int is
    // accepted (clamped): a huge positive returns every item, a negative none.
    // Only a genuinely non-integer `n` is rejected.
    let limit = match n_arg {
        None | Some(Value::None) => None,
        Some(Value::Int(n)) => Some(usize::try_from(n.max(0)).unwrap_or(usize::MAX)),
        Some(Value::Bool(b)) => Some(usize::from(b)),
        // `value @` binds the whole `Value` (so it is dropped, not plain-`Drop`ped
        // — the `Ref` owns a heap refcount), while `id` reads the entry.
        Some(value @ Value::Ref(id)) if matches!(vm.heap.get(id), HeapData::LongInt(_)) => {
            let negative = match vm.heap.get(id) {
                HeapData::LongInt(li) => li.is_negative(),
                _ => unreachable!("guarded by the matches! above"),
            };
            value.drop_with(vm);
            Some(if negative { 0 } else { usize::MAX })
        }
        Some(other) => {
            let ty = other.py_type_name(vm);
            other.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "'{ty}' object cannot be interpreted as an integer"
            )));
        }
    };

    let counts = counter
        .get(vm.heap)
        .iter()
        .map(|(_, count)| count.clone_with_heap(vm.heap))
        .collect();
    let order = counter_order(counts, vm)?;
    let take = limit.unwrap_or(order.len()).min(order.len());

    let mut items: Vec<Value> = Vec::with_capacity(take);
    for &i in &order[..take] {
        let dict = counter.get(vm.heap);
        let key = dict.key_at(i).expect("index in range").clone_with_heap(vm.heap);
        let count = dict.value_at(i).expect("index in range").clone_with_heap(vm.heap);
        let pair = allocate_tuple(smallvec![key, count], vm.heap);
        items.push(pair);
    }
    Ok(vm.heap.allocate_as(List::new(items)).into_value())
}

/// `Counter.elements()` — a list repeating each element by its count, skipping
/// zero and negative counts (Monty's eager iterator convention returns a list
/// rather than CPython's lazy iterator).
pub(crate) fn counter_elements<'h>(
    counter: &HeapObjectRead<'h, Dict>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    args.check_zero_args("elements", vm.heap)?;
    let n = counter.get(vm.heap).len();

    // Resolve every repetition length up front: a count that `itertools.repeat`
    // rejects — non-integer (TypeError) or outside `ssize_t` (OverflowError) —
    // must raise before anything is built.
    let mut lengths = Vec::with_capacity(n);
    for i in 0..n {
        let count = counter
            .get(vm.heap)
            .value_at(i)
            .expect("index in range")
            .clone_with_heap(vm.heap);
        let len = count_repeat_len(&count, vm);
        count.drop_with(vm);
        lengths.push(len?);
    }

    // Pre-check the total element count against the resource tracker: a single
    // large count (e.g. `c['a'] = 10**18`) would otherwise attempt to build a
    // multi-exabyte Rust-heap `Vec` before returning a graceful error.
    let total = lengths.iter().fold(0usize, |acc, len| acc.saturating_add(*len));
    check_repeat_size(VALUE_SIZE, total, &vm.heap.tracker)?;

    // `Vec::new()` (not `with_capacity(total)`): `total` is attacker-controlled,
    // and `check_repeat_size` above is the real guard (it fires before this loop
    // whenever a memory limit is set). Pre-reserving would itself panic.
    let mut items: Vec<Value> = Vec::new();
    for (i, &count) in lengths.iter().enumerate() {
        for _ in 0..count {
            let key = counter
                .get(vm.heap)
                .key_at(i)
                .expect("index in range")
                .clone_with_heap(vm.heap);
            items.push(key);
        }
    }
    Ok(vm.heap.allocate_as(List::new(items)).into_value())
}

/// The four binary `Counter` algebra operators.
#[derive(Clone, Copy)]
pub(crate) enum CounterOp {
    /// `+` — sum counts over the union of keys.
    Add,
    /// `-` — subtract counts over the union of keys.
    Sub,
    /// `&` — minimum count over keys present in both.
    And,
    /// `|` — maximum count over the union of keys.
    Or,
}

/// Computes a binary `Counter` operation, returning a new Counter that keeps
/// only positive counts (CPython's `Counter.__add__` etc.).
pub(crate) fn counter_binary_op<'h>(
    lhs: &HeapObjectRead<'h, Dict>,
    rhs: &HeapObjectRead<'h, Dict>,
    op: CounterOp,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let mut result = Dict::new();
    result.make_counter();
    let result = vm.heap.allocate_as(result);
    // Guard the freshly allocated result dict: a catchable error below (e.g.
    // arithmetic or comparison on non-numeric counts raising `TypeError`) must
    // free it, which the bare `?` early-returns would otherwise leak.
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();
    let result = result.read(vm.heap);

    // Each operand is snapshotted only when it is about to be consumed: taking
    // the right snapshot up front would leak it if folding the left one raised.
    match op {
        CounterOp::Add => {
            let l_pairs = lhs.clone_all_pairs(vm)?;
            counter_bump_all(result, l_pairs, false, vm)?;
            let r_pairs = rhs.clone_all_pairs(vm)?;
            counter_bump_all(result, r_pairs, false, vm)?;
        }
        CounterOp::Sub => {
            let l_pairs = lhs.clone_all_pairs(vm)?;
            counter_bump_all(result, l_pairs, false, vm)?;
            let r_pairs = rhs.clone_all_pairs(vm)?;
            counter_bump_all(result, r_pairs, true, vm)?;
        }
        // `|` keeps the larger count over the union of keys (see `counter_binary_extreme`).
        CounterOp::Or => counter_binary_extreme(result, lhs, rhs, ExtremeOp::Max, vm)?,
        // `&` keeps the smaller count over shared keys (see `counter_binary_extreme`).
        CounterOp::And => counter_binary_extreme(result, lhs, rhs, ExtremeOp::Min, vm)?,
    }
    counter_retain_positive(result, vm)?;
    Ok(result_guard.into_inner().into_value())
}

/// The extreme kept by the binary `&`/`|` algebra.
#[derive(Clone, Copy)]
enum ExtremeOp {
    /// `&` — the minimum count, over keys present in both.
    Min,
    /// `|` — the maximum count, over the union of keys.
    Max,
}

/// Folds a binary `&`/`|` into the fresh `result`.
///
/// Both walk the *left* operand's keys and pick the extreme of `count` and the
/// right's `other[elem]` (a missing right key reads as `0`), comparing exactly as
/// CPython does — `count < other_count`, left operand first, so an unorderable
/// pair reports `<` with the operands in that order. `|` additionally carries the
/// right-only keys across. The caller's final `retain_positive` performs the
/// `> 0` strip (and raises the `> 0` `TypeError` for an unorderable right-only
/// key, matching CPython's second-loop wording).
fn counter_binary_extreme<'h>(
    result: &mut HeapObjectRead<'h, Dict>,
    lhs: &HeapObjectRead<'h, Dict>,
    rhs: &HeapObjectRead<'h, Dict>,
    op: ExtremeOp,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    // Two guards cover every failure below, so each one is a plain `?`: the outer
    // releases the entries the loop never reached, the inner the one in flight.
    let l_pairs = lhs.clone_all_pairs(vm)?.into_iter();
    defer_drop_mut!(l_pairs, vm);
    for entry in l_pairs.by_ref() {
        let mut entry = DropGuard::new(entry, vm);
        let ((key, count), vm) = entry.as_parts_mut();
        let other = rhs.dict_get(key, vm)?.unwrap_or(Value::Int(0));
        let mut other = DropGuard::new(other, vm);
        let (other_count, vm) = other.as_parts_mut();
        // `count < other`: `|` keeps `count` when it is *not* smaller, `&` keeps
        // it when it *is* smaller. The loser is released.
        let count_is_smaller = count_holds(count, other_count, CountCmp::Lt, vm)?;
        let keep_count = match op {
            ExtremeOp::Max => !count_is_smaller,
            ExtremeOp::Min => count_is_smaller,
        };
        let other = other.into_inner();
        let ((key, count), vm) = entry.into_parts();
        let picked = if keep_count {
            other.drop_with(vm);
            count
        } else {
            count.drop_with(vm);
            other
        };
        if let Some(old) = result.set(key, picked, vm)? {
            old.drop_with(vm);
        }
    }

    if matches!(op, ExtremeOp::Max) {
        let r_pairs = rhs.clone_all_pairs(vm)?.into_iter();
        defer_drop_mut!(r_pairs, vm);
        for entry in r_pairs.by_ref() {
            let mut entry = DropGuard::new(entry, vm);
            let ((key, _), vm) = entry.as_parts_mut();
            // Shared keys were already resolved in the left pass, so the guard
            // releases this entry; only a right-only key reaches the result.
            if let Some(existing) = lhs.dict_get(key, vm)? {
                existing.drop_with(vm);
            } else {
                let ((key, count), vm) = entry.into_parts();
                if let Some(old) = result.set(key, count, vm)? {
                    old.drop_with(vm);
                }
            }
        }
    }
    Ok(())
}

/// The multiset comparisons CPython defines between two Counters.
#[derive(Clone, Copy)]
pub(crate) enum CounterCmp {
    /// `<=` — every count in the left is at most the matching right count.
    Le,
    /// `<` — `<=` and not equal.
    Lt,
    /// `>=` — every count in the left is at least the matching right count.
    Ge,
    /// `>` — `>=` and not equal.
    Gt,
}

/// Evaluates a multiset comparison between two Counters.
///
/// CPython walks the union of both key sets with a missing key reading as `0`
/// (`all(self[e] <= other[e] for c in (self, other) for e in c)`), so a key
/// present in only one operand still constrains the result. The strict forms
/// are defined in terms of the loose ones: `<` is `<= and !=`, `>` is
/// `>= and !=`.
pub(crate) fn counter_compare<'h>(
    lhs: &HeapObjectRead<'h, Dict>,
    rhs: &HeapObjectRead<'h, Dict>,
    cmp: CounterCmp,
    vm: &mut VM<'h>,
) -> RunResult<bool> {
    // `<` is `<= and !=` and `>` is `>= and !=`, so both strict forms run the
    // loose comparison — whose operator is what an unorderable pair reports — and
    // add the inequality via the separately tracked `equal`.
    let (op, strict) = match cmp {
        CounterCmp::Le => (CountCmp::Le, false),
        CounterCmp::Lt => (CountCmp::Le, true),
        CounterCmp::Ge => (CountCmp::Ge, false),
        CounterCmp::Gt => (CountCmp::Ge, true),
    };
    // `holds` is false as soon as any pair breaks the loose ordering; `equal`
    // tracks whether every compared pair matched, which the strict forms need.
    let mut holds = true;
    let mut equal = true;
    // `counter_union_counts` hands back owned pairs the caller must drop. The
    // guard covers every exit — a short-circuit (`!holds`), a failing comparison,
    // and normal exhaustion — releasing whatever the loop never compared.
    let pairs = counter_union_counts(lhs, rhs, vm)?.into_iter();
    defer_drop_mut!(pairs, vm);
    for pair in pairs.by_ref() {
        defer_drop!(pair, vm);
        let (l, r) = pair;
        // One `py_cmp` yields both the loose check and the equality the strict
        // forms need. A `NaN` pair is unordered: the loose comparison fails (as
        // in CPython, `nan <= x` is false) and the pair counts as not-equal.
        match l.py_cmp(r, vm)? {
            CmpOrder::Ordered(Ordering::Equal) => {}
            CmpOrder::Ordered(ordering) => {
                holds &= op.holds(ordering);
                equal = false;
            }
            CmpOrder::Unordered => {
                holds = false;
                equal = false;
            }
            CmpOrder::Incomparable => {
                return Err(ExcType::type_error_ordering(
                    op.symbol(),
                    &l.py_type_name(vm),
                    &r.py_type_name(vm),
                ));
            }
        }
        if !holds {
            break;
        }
    }
    Ok(holds && (!strict || !equal))
}

/// Pairs up the counts of two Counters over the union of their keys, with a
/// key missing from either side reading as `0`.
///
/// Returns owned clones so the caller can run comparisons that need `&mut VM`.
/// Every returned pair must be dropped by the caller.
fn counter_union_counts<'h>(
    lhs: &HeapObjectRead<'h, Dict>,
    rhs: &HeapObjectRead<'h, Dict>,
    vm: &mut VM<'h>,
) -> RunResult<Vec<(Value, Value)>> {
    // Three guards, so a failing lookup is a plain `?`: the pairs accumulated so
    // far, the entries not yet visited, and the entry in flight all get released.
    let mut pairs = DropGuard::new(Vec::new(), vm);
    // Left keys against their right counterparts, then the right-only keys.
    for (keys, other, flip) in [(lhs, rhs, false), (rhs, lhs, true)] {
        let (collected, vm) = pairs.as_parts_mut();
        let entries = keys.clone_all_pairs(vm)?.into_iter();
        defer_drop_mut!(entries, vm);
        for entry in entries.by_ref() {
            let mut entry = DropGuard::new(entry, vm);
            let ((key, _), vm) = entry.as_parts_mut();
            let other = other.dict_get(key, vm)?;
            // On the second pass only keys absent from the left are new; the
            // shared ones were already paired, so skip them (the guard releases
            // the entry); otherwise the count moves into `collected`.
            match (flip, other) {
                (true, Some(other)) => other.drop_with(vm),
                (true, None) => {
                    let ((key, count), vm) = entry.into_parts();
                    key.drop_with(vm);
                    collected.push((Value::Int(0), count));
                }
                (false, other) => {
                    let ((key, count), vm) = entry.into_parts();
                    key.drop_with(vm);
                    collected.push((count, other.unwrap_or(Value::Int(0))));
                }
            }
        }
    }
    Ok(pairs.into_inner())
}

/// Applies a `Counter` algebra operator **in place**, mutating `counter`.
///
/// CPython's `__iadd__`/`__isub__`/`__iand__`/`__ior__` mutate `self` and return
/// it, so `c += other` keeps the object identity and any alias observes the
/// update — unlike the binary operators, which build a fresh Counter. Each ends
/// with `_keep_positive()`, stripping entries that are not `> 0`.
///
/// The right operand `rhs` may be **any mapping** (`c += {'a': 2}`), not only a
/// Counter. `+=`/`-=`/`|=` iterate `rhs.items()`, so a non-mapping raises
/// `AttributeError`; `&=` subscripts `rhs[elem]`, so a plain dict raises
/// `KeyError` for a key absent from it (a Counter reads that as `0`).
///
/// Note the asymmetry CPython has between the directions: `+=` / `-=` walk
/// *other*'s keys (so new keys can appear), whereas `&=` walks *self*'s keys
/// (so no key is ever added), and `|=` walks *other*'s keys keeping the larger.
pub(crate) fn counter_inplace_op<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    rhs: &Value,
    op: CounterOp,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    match op {
        // `for elem, count in other.items(): self[elem] += count`
        CounterOp::Add => {
            let rhs = counter_require_mapping(rhs, vm)?;
            counter_bump_all(counter, rhs.clone_all_pairs(vm)?, false, vm)?;
        }
        CounterOp::Sub => {
            let rhs = counter_require_mapping(rhs, vm)?;
            counter_bump_all(counter, rhs.clone_all_pairs(vm)?, true, vm)?;
        }
        // `for elem, other_count in other.items(): if other_count > self[elem]:
        //     self[elem] = other_count` — walks *other*, so new keys can appear.
        CounterOp::Or => {
            let rhs = counter_require_mapping(rhs, vm)?;
            let r_pairs = rhs.clone_all_pairs(vm)?.into_iter();
            defer_drop_mut!(r_pairs, vm);
            for entry in r_pairs.by_ref() {
                let mut entry = DropGuard::new(entry, vm);
                let ((key, other_count), vm) = entry.as_parts_mut();
                // A key absent from `self` reads as 0 (`self[elem]`).
                let count = counter.dict_get(key, vm)?.unwrap_or(Value::Int(0));
                // `other_count > self[elem]`: other first, so the TypeError names it first.
                let bigger = count_holds(other_count, &count, CountCmp::Gt, vm);
                count.drop_with(vm);
                // A count that is not bigger leaves `self` alone, and the guard
                // releases the entry — as it does if the comparison raised.
                if bigger? {
                    let ((key, other_count), vm) = entry.into_parts();
                    if let Some(old) = counter.set(key, other_count, vm)? {
                        old.drop_with(vm);
                    }
                }
            }
        }
        // `for elem, count in self.items(): if other[elem] < count:
        //     self[elem] = other[elem]` — walks *self*, subscripting `other`.
        CounterOp::And => {
            let pairs = counter.clone_all_pairs(vm)?.into_iter();
            defer_drop_mut!(pairs, vm);
            for entry in pairs.by_ref() {
                let mut entry = DropGuard::new(entry, vm);
                let ((key, count), vm) = entry.as_parts_mut();
                // `other[elem]` is a real subscript: a Counter yields 0 for a
                // missing key, a plain dict raises `KeyError`, and a non-mapping
                // raises its subscript error — exactly CPython's `__iand__`. An
                // empty `self` never reaches here, so `c &= 5` is a no-op then.
                let other_count = rhs.py_getitem(key, vm)?;
                let mut other_count = DropGuard::new(other_count, vm);
                let (other, vm) = other_count.as_parts_mut();
                // `other[elem] < count`: other first, matching CPython's wording.
                let is_smaller = count_holds(other, count, CountCmp::Lt, vm)?;
                // Only the smaller of the two survives; the guards release the
                // loser, the whole entry when nothing changes, and both if the
                // comparison raised.
                if is_smaller {
                    let smaller = other_count.into_inner();
                    let ((key, count), vm) = entry.into_parts();
                    count.drop_with(vm);
                    if let Some(old) = counter.set(key, smaller, vm)? {
                        old.drop_with(vm);
                    }
                }
            }
        }
    }
    counter_retain_positive(counter, vm)
}

/// Computes a unary `Counter` operation. `+c` keeps the counts that are `> 0`;
/// `-c` keeps the *strictly negative* counts, storing their negation (CPython's
/// `{elem: -count for elem, count in self.items() if count < 0}`). Both leave
/// only positive counts, so the result is a valid Counter.
pub(crate) fn counter_unary_op<'h>(
    counter: &HeapObjectRead<'h, Dict>,
    negate: bool,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let mut result = Dict::new();
    result.make_counter();
    let result = vm.heap.allocate_as(result);
    // Guard the freshly allocated result dict: a catchable error below (e.g.
    // comparing a non-numeric count) must free it, which the bare `?`
    // early-returns would otherwise leak.
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();
    let result = result.read(vm.heap);
    // Scoped so the iterator guard releases its borrow of `result_guard` before
    // the result is handed back.
    {
        let pairs = counter.clone_all_pairs(vm)?.into_iter();
        defer_drop_mut!(pairs, vm);
        for entry in pairs.by_ref() {
            let mut entry = DropGuard::new(entry, vm);
            let ((_, count), vm) = entry.as_parts_mut();
            // `-c` filters on `count < 0` first (reporting `<` for an unorderable
            // count, dropping a NaN), then stores `0 - count`. `+c` copies the count
            // through and lets the trailing `retain_positive` apply the `> 0` strip.
            let negated = if negate {
                if !count_holds(count, &Value::Int(0), CountCmp::Lt, vm)? {
                    // Not negative, so nothing is stored; the guard releases the entry.
                    continue;
                }
                Some(count_arith(&Value::Int(0), count, true, vm)?)
            } else {
                None
            };
            let ((key, count), vm) = entry.into_parts();
            let stored = match negated {
                Some(negated) => {
                    count.drop_with(vm);
                    negated
                }
                None => count,
            };
            if let Some(old) = result.set(key, stored, vm)? {
                old.drop_with(vm);
            }
        }
    }
    counter_retain_positive(result, vm)?;
    Ok(result_guard.into_inner().into_value())
}

/// Folds a batch of `(key, delta)` pairs into `counter`, releasing any pairs
/// left unconsumed when a bump fails.
fn counter_bump_all<'h>(
    counter: &mut HeapObjectRead<'h, Dict>,
    pairs: Vec<(Value, Value)>,
    subtract: bool,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    let pairs = pairs.into_iter();
    defer_drop_mut!(pairs, vm);
    for (key, delta) in pairs.by_ref() {
        // `counter_bump` borrows the delta, so each one is released here.
        let outcome = counter_bump(counter, key, &delta, subtract, false, vm);
        delta.drop_with(vm);
        outcome?;
    }
    Ok(())
}

/// Returns the dictionary handle for an in-place right-hand operand, or raises
/// CPython's `AttributeError: 'X' object has no attribute 'items'` — the failure
/// `c += other` / `-=` / `|=` hit when `other.items()` has no `.items()`.
fn counter_require_mapping<'h>(rhs: &Value, vm: &VM<'h>) -> RunResult<HeapObjectRead<'h, Dict>> {
    match rhs.read_heap(vm) {
        Some(HeapReadOutput::Dict(dict)) => Ok(dict),
        _ => Err(ExcType::attribute_error(rhs.py_type_name(vm), "items")),
    }
}

/// Removes every entry whose count is not strictly positive from `counter`.
fn counter_retain_positive<'h>(counter: &mut HeapObjectRead<'h, Dict>, vm: &mut VM<'h>) -> RunResult<()> {
    // The keys to remove are collected first: the scan clones them out of the
    // dict, so popping while iterating is not possible. Both guards release what
    // they still hold if a count comparison — or a `pop` hashing a key — raises.
    let mut remove = DropGuard::new(Vec::new(), vm);
    {
        let (remove, vm) = remove.as_parts_mut();
        let entries = counter.clone_all_pairs(vm)?.into_iter();
        defer_drop_mut!(entries, vm);
        for (key, count) in entries {
            defer_drop_mut!(count, vm);
            let mut key_guard = DropGuard::new(key, vm);
            if !count_is_positive(count, key_guard.ctx())? {
                let key = key_guard.into_inner();
                remove.push(key);
            }
        }
    }
    let (remove, vm) = remove.into_parts();
    let remove = remove.into_iter();
    defer_drop_mut!(remove, vm);
    for key in remove.by_ref() {
        defer_drop!(key, vm);
        if let Some(old) = counter.pop(key, vm)? {
            old.drop_with(vm);
        }
    }
    Ok(())
}
