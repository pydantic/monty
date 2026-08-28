use std::{cmp::Ordering, collections::VecDeque, fmt::Write, mem};

use super::{CmpOrder, PyTrait, iter::collect_owned_iterable};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    resource_checks::{check_estimated_size, check_repeat_size},
    types::{
        LazyHeapSet, Type,
        list::repr_items_fmt,
        long_int::repeat_count,
        slice::{normalize_sequence_index, value_to_i64_bound},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// `deque([iterable[, maxlen]])` — both positional-or-keyword, both defaulted.
/// See [`Deque::init`] for why the over-arity case is pre-checked instead.
#[derive(FromArgs)]
#[from_args(name = "deque")]
struct DequeArgs {
    // `Option` rather than a `Value::None` default: an explicit `None` iterable is
    // a `TypeError` in CPython (`'NoneType' object is not iterable`), so "omitted"
    // has to be distinguishable from a real `None`.
    #[from_args(static_string = "IterableArg", default)]
    iterable: Option<Value>,
    #[from_args(default = Value::None)]
    maxlen: Value,
}

/// `deque.rotate([n])` — `PyArg_UnpackTuple`, so the arity error is
/// `rotate expected at most 1 argument, got 2` (no type name).
#[derive(FromArgs)]
#[from_args(name = "rotate", style = unpack)]
struct RotateArgs {
    #[from_args(pos_only, default = Value::Int(1))]
    n: Value,
}

/// `deque.insert(i, x)` — `PyArg_UnpackTuple` with a fixed arity, so the error is
/// `insert expected 2 arguments, got 1`.
#[derive(FromArgs)]
#[from_args(name = "insert", style = unpack)]
struct InsertArgs {
    #[from_args(pos_only)]
    index: Value,
    #[from_args(pos_only)]
    item: Value,
}

/// `deque.index(x[, start[, stop]])` — `PyArg_UnpackTuple` with `min < max`, so a
/// missing `x` reports `index expected at least 1 argument, got 0`.
#[derive(FromArgs)]
#[from_args(name = "index", style = unpack)]
struct IndexArgs {
    #[from_args(pos_only)]
    value: Value,
    // `Option` rather than a `Value::None` default: an explicit `None` bound is an
    // error in CPython, so "omitted" has to be distinguishable from a real `None`.
    #[from_args(pos_only, default)]
    start: Option<Value>,
    #[from_args(pos_only, default)]
    stop: Option<Value>,
}

/// Python's `collections.deque`: a double-ended queue backed by a [`VecDeque`].
///
/// The distinguishing feature over [`List`](super::List) is `maxlen`: a bounded
/// deque evicts from the *opposite* end on overflow (a ring buffer). A deque is
/// **not** equal to a list with the same items, and is unhashable.
///
/// Items are owned `Value`s: callers transfer an already-inc-ref'd reference in,
/// and anything evicted by `maxlen` is dropped here, so eviction must not leak.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Deque {
    items: VecDeque<Value>,
    /// Maximum length; `None` means unbounded. Read-only from Python.
    maxlen: Option<usize>,
    /// True if any item is a `Value::Ref` — lets the GC skip iteration.
    contains_refs: bool,
    /// Structural-mutation counter mirroring CPython's `dequeobject.state`, so a
    /// live iterator detects invalidation even from length-preserving mutations
    /// (`rotate()`, a paired `append()`/`popleft()`). See [`Deque::bump_state`].
    state: u64,
}

impl Deque {
    /// Creates a deque from a vector, truncating to `maxlen` from the *left*
    /// (CPython keeps the rightmost items: `deque([1, 2, 3], 2) == deque([2, 3])`).
    ///
    /// Note: does NOT adjust refcounts — the caller owns the values passed in.
    /// Any item dropped by truncation is returned so the caller can release it.
    #[must_use]
    pub fn new(items: Vec<Value>, maxlen: Option<usize>) -> (Self, Vec<Value>) {
        let mut items = VecDeque::from(items);
        let mut evicted = Vec::new();
        if let Some(max) = maxlen {
            while items.len() > max {
                if let Some(v) = items.pop_front() {
                    evicted.push(v);
                }
            }
        }
        let contains_refs = items.iter().any(|v| matches!(v, Value::Ref(_)));
        (
            Self {
                items,
                maxlen,
                contains_refs,
                state: 0,
            },
            evicted,
        )
    }

    /// Number of items currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The `maxlen` bound, or `None` if unbounded.
    #[must_use]
    pub fn maxlen(&self) -> Option<usize> {
        self.maxlen
    }

    /// Returns whether the deque holds any heap references.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// The current mutation counter, captured by iterators to detect invalidation.
    #[inline]
    #[must_use]
    pub fn state(&self) -> u64 {
        self.state
    }

    /// Records a structural mutation, invalidating any live iterator.
    ///
    /// Only mutations CPython counts belong here: adding, removing, or reordering
    /// items. `reverse()` and `d[i] = x` deliberately do NOT call this — CPython
    /// leaves `state` alone for both, so they are legal mid-iteration. Wraps rather
    /// than overflows; a collision needs 2^64 mutations between two `next()` calls.
    #[inline]
    fn bump_state(&mut self) {
        self.state = self.state.wrapping_add(1);
    }

    /// Borrows the item at `index`, which must be in range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.items.get(index)
    }

    /// Iterates the items in order, for the GC child walk.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.items.iter()
    }
}

impl Deque {
    /// Constructs a deque from the `collections.deque(...)` call.
    ///
    /// `deque([iterable[, maxlen]])` — both arguments are positional-or-keyword.
    /// The over-arity case is pre-checked because CPython's wording here
    /// (`deque() takes at most 2 arguments (N given)`) omits the word
    /// "positional" that every `FromArgs` style emits.
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        if let ArgValues::ArgsKargs { args: positional, .. } = &args
            && positional.len() > 2
        {
            let given = args.count();
            args.drop_with(vm);
            return Err(ExcType::type_error_deque_too_many_args(given));
        }

        let DequeArgs { iterable, maxlen } = DequeArgs::from_args(args, vm)?;

        // `maxlen` is validated before the iterable is consumed (CPython order), so
        // the guard releases the already-bound `iterable` on every failing branch.
        // An explicit `None` is unbounded; a big int out of range is `OverflowError`.
        let mut iterable = DropGuard::new(iterable, vm);
        let raw_maxlen = if let Value::None = maxlen {
            None
        } else {
            let vm = iterable.ctx();
            let parsed = read_ssize(&maxlen, vm, ExcType::overflow_c_ssize_t);
            maxlen.drop_with(vm);
            match parsed {
                Some(Ok(i)) => Some(i),
                Some(Err(e)) => return Err(e),
                None => return Err(ExcType::type_error_integer_required()),
            }
        };
        let maxlen = raw_maxlen.map(check_maxlen).transpose()?;

        // An omitted iterable is empty; an explicit `None` falls through to
        // `collect_owned_iterable`, raising `'NoneType' object is not iterable`.
        let (iterable, vm) = iterable.into_parts();
        let items = match iterable {
            None => Vec::new(),
            Some(v) => collect_owned_iterable(v, vm)?,
        };

        let (deque, evicted) = Self::new(items, maxlen);
        // Items dropped by the maxlen truncation still hold their refcounts.
        evicted.drop_with(vm);
        let heap_id = vm.heap.allocate(HeapData::Deque(deque));
        Ok(Value::Ref(heap_id))
    }
}

/// Rejects a negative `maxlen`, converting a validated one to `usize`.
///
/// The conversion is fallible on a 32-bit target (`wasm32-wasip1`), where an
/// `i64` `maxlen` above `usize::MAX` is a real input — `deque([], 2**40)`. It
/// reports the same `OverflowError` [`read_ssize`] gives a `maxlen` too large
/// for `i64`, so the two ways of overflowing an index-sized integer agree.
fn check_maxlen(n: i64) -> RunResult<usize> {
    if n < 0 {
        Err(ExcType::value_error_maxlen_negative())
    } else {
        usize::try_from(n).map_err(|_| ExcType::overflow_c_ssize_t())
    }
}

/// Reads a deque integer argument (`int`, `bool`, or big `int`) as an `i64`.
///
/// A big `int` is accepted (it is a real int); `None` means genuinely
/// non-integer (the caller raises its own type error), and `Some(Err)` means a
/// big int overflowed `i64` — CPython's `PyNumber_AsSsize_t` failure, whose
/// exception kind the caller supplies (`OverflowError` for maxlen/rotate/insert,
/// `IndexError` for subscript).
fn read_ssize(value: &Value, vm: &VM<'_>, overflow: fn() -> RunError) -> Option<RunResult<i64>> {
    match value {
        Value::Int(i) => Some(Ok(*i)),
        Value::Bool(b) => Some(Ok(i64::from(*b))),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => Some(li.to_i64().ok_or_else(overflow)),
        _ => None,
    }
}

impl<'h> HeapRead<'h, Deque> {
    /// Appends to the right, evicting from the left if `maxlen` is reached.
    ///
    /// Ownership of `item` transfers to the deque (refcount already handled by
    /// the caller); any evicted item is released here.
    pub fn append(&mut self, vm: &mut VM<'h>, item: Value) {
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        let this = self.get_mut(vm.heap);
        this.items.push_back(item);
        this.bump_state();
        let evicted = evict_front_if_full(this);
        if let Some(value) = evicted {
            value.drop_with(vm);
        }
    }

    /// Appends to the left, evicting from the right if `maxlen` is reached.
    pub fn appendleft(&mut self, vm: &mut VM<'h>, item: Value) {
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        let this = self.get_mut(vm.heap);
        this.items.push_front(item);
        this.bump_state();
        let evicted = evict_back_if_full(this);
        if let Some(value) = evicted {
            value.drop_with(vm);
        }
    }

    /// Clones every item, incrementing refcounts — used by `copy`, `+` and `*`.
    fn clone_all_items(&self, vm: &mut VM<'h>) -> RunResult<Vec<Value>> {
        let len = self.get(vm.heap).len();
        vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let item = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            out.push(item);
        }
        Ok(out)
    }

    /// Resolves a Python index (negative counts from the right) to a real one.
    fn resolve_index(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<usize> {
        // A slice is rejected with CPython's sequence wording (names the type).
        if let Value::Ref(id) = key
            && matches!(vm.heap.get(*id), HeapData::Slice(_))
        {
            return Err(ExcType::type_error_sequence_index("slice"));
        }
        // A big int is a valid index, raising `IndexError` if it can't fit.
        let index = if let Some(res) = read_ssize(key, vm, ExcType::index_error_int_too_large) {
            res?
        } else {
            let name = key.py_type_name(vm);
            return Err(ExcType::type_error_sequence_index(&name));
        };
        let len = i64::try_from(self.get(vm.heap).len()).expect("deque length exceeds i64::MAX");
        let normalized = if index < 0 { index + len } else { index };
        if normalized < 0 || normalized >= len {
            return Err(ExcType::index_error_deque_out_of_range());
        }
        Ok(usize::try_from(normalized).expect("index validated non-negative"))
    }
}

/// Drops the leftmost item if the deque now exceeds `maxlen`.
///
/// Returns the evicted value so the caller can release its refcount — eviction
/// happens on the hot `append` path, so this must never leak.
fn evict_front_if_full(deque: &mut Deque) -> Option<Value> {
    match deque.maxlen {
        Some(max) if deque.items.len() > max => deque.items.pop_front(),
        _ => None,
    }
}

/// Drops the rightmost item if the deque now exceeds `maxlen`.
fn evict_back_if_full(deque: &mut Deque) -> Option<Value> {
    match deque.maxlen {
        Some(max) if deque.items.len() > max => deque.items.pop_back(),
        _ => None,
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Deque> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// `in` walks the deque comparing each item by `==`, like `list`.
    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let this = self.get(vm.heap);
        let (len, start_state) = (this.len(), this.state());
        for i in 0..len {
            let el = self
                .get(vm.heap)
                .get(i)
                .expect("index in range")
                .clone_with_heap(vm.heap);
            let eq = item.py_eq(&el, vm);
            el.drop_with(vm);
            if eq? {
                return Ok(Some(true));
            }
            // A user `__eq__` mutating the deque invalidates the walk (and the
            // indices above); CPython raises, checking only after a false compare.
            if self.get(vm.heap).state() != start_state {
                return Err(ExcType::runtime_error_deque_mutated());
            }
        }
        Ok(Some(false))
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Deque
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        value.drop_with(vm);
        let type_name = self.py_type(vm).name(vm.heap, vm.interns);
        if name.static_string() == Some(StaticStrings::Maxlen) {
            Err(ExcType::attribute_error_not_writable("maxlen", &type_name))
        } else {
            Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)))
        }
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let deque_id = self.id();
        let iterator = vm
            .heap
            .allocate(HeapData::DequeIterator(DequeIterator::new(deque_id, vm)));
        vm.heap.inc_ref(deque_id);
        Ok(Value::Ref(iterator))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.get(vm.heap).len() > 0)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let idx = self.resolve_index(key, vm)?;
        Ok(self.get(vm.heap).items[idx].clone_with_heap(vm))
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop_mut!(value, vm);

        let idx = self.resolve_index(key, vm)?;
        if matches!(*value, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        // The guard drops whatever `value` holds after the swap — i.e. the old item.
        mem::swap(&mut self.get_mut(vm.heap).items[idx], value);
        Ok(())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // A deque only ever equals another deque — unlike NamedTuple/tuple, there
        // is no cross-type equality with list. `maxlen` is not part of equality.
        let Some(HeapReadOutput::Deque(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let len = self.get(vm.heap).len();
        if len != other.get(vm.heap).len() {
            return Ok(Some(false));
        }
        let start_states = states(self, &other, vm);
        // Charge a recursion level: two distinct cyclic deques (`a.append(a);
        // b.append(b); a == b`) re-enter here per level and would otherwise
        // overflow the host stack. A deque walks by index, so it charges directly.
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        for i in 0..len {
            let a = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(a, vm);
            let b = other.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(Some(false));
            }
            // A user `__eq__` mutating either deque leaves `len` and the indices
            // above stale. CPython walks both with iterators, which notice on the
            // step after the comparison — so check here, not before the compare.
            if states(self, &other, vm) != start_states {
                return Err(ExcType::runtime_error_deque_mutated());
            }
        }
        Ok(Some(true))
    }

    /// Lexicographic ordering, deque-vs-deque only.
    ///
    /// The trait takes `&Self`, so the dispatcher has already rejected other
    /// types with "'<' not supported between instances of ...". Charges a
    /// recursion level for the same reason [`py_eq_impl`](Self::py_eq_impl)
    /// does — nested deques recurse through here.
    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let self_len = self.get(vm.heap).len();
        let other_len = other.get(vm.heap).len();
        let start_states = states(self, other, vm);
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        for i in 0..self_len.min(other_len) {
            let a = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(a, vm);
            let b = other.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(b, vm);
            match a.py_cmp(b, vm)? {
                CmpOrder::Ordered(Ordering::Equal) => {}
                CmpOrder::Ordered(ord) => return Ok(CmpOrder::Ordered(ord)),
                // A `NaN` element is never `==`-equal, so it is the first
                // differing pair and the deque is unordered (yields `False`).
                CmpOrder::Unordered => return Ok(CmpOrder::Unordered),
                // CPython checks `__eq__` first and only orders non-equal pairs,
                // so equal-but-unorderable elements (e.g. `None == None`) don't
                // block the comparison — mirror list/tuple.
                CmpOrder::Incomparable => {
                    if !a.py_eq(b, vm)? {
                        return Ok(CmpOrder::Incomparable);
                    }
                }
            }
            // Either comparison above can run a user `__eq__`/`__lt__` that
            // resizes a deque, invalidating the indices — see `py_eq_impl`.
            if states(self, other, vm) != start_states {
                return Err(ExcType::runtime_error_deque_mutated());
            }
        }
        // All shared items equal — the shorter deque sorts first.
        Ok(CmpOrder::Ordered(self_len.cmp(&other_len)))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("deque(")?;
        if let Ok(mut guard) = vm.recursion_guard() {
            let vm = &mut *guard;
            // Format a snapshot of the items (taken only once the depth limit
            // allows a body at all): CPython's deque repr copies to a list
            // first, so a user `__repr__` mutating the deque mid-format
            // changes nothing (and can't invalidate indices here).
            let items = self.clone_all_items(vm)?;
            defer_drop!(items, vm);
            f.write_char('[')?;
            repr_items_fmt(items, f, vm, heap_ids)?;
            f.write_char(']')?;
        } else {
            // Depth limit reached — same elision `repr_sequence_fmt` emits.
            f.write_str("...")?;
        }
        // CPython only shows maxlen when the deque is bounded.
        if let Some(max) = self.get(vm.heap).maxlen() {
            write!(f, ", maxlen={max}")?;
        }
        f.write_char(')')?;
        Ok(())
    }

    /// `deque + deque` — concatenation, keeping the LEFT operand's `maxlen`
    /// (so the result can truncate). Any non-deque right operand returns `None`,
    /// yielding CPython's "can only concatenate deque" `TypeError`.
    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(HeapReadOutput::Deque(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let maxlen = self.get(vm.heap).maxlen();
        let mut items = self.clone_all_items(vm)?;
        items.extend(other.clone_all_items(vm)?);
        let (deque, evicted) = Deque::new(items, maxlen);
        evicted.drop_with(vm.heap);
        let id = vm.heap.allocate(HeapData::Deque(deque));
        Ok(Some(Value::Ref(id)))
    }

    /// `deque += <iterable>` — CPython's `deque.__iadd__` *is* `extend`, so any
    /// iterable works (`d += [1, 2]`, `d += 'ab'`) and a non-iterable raises the
    /// iterator protocol's `TypeError` rather than falling back to `+`'s
    /// concatenation error. The deque keeps its identity, so aliases see the
    /// update.
    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        // `deque_extend` consumes the iterable, so hand it an owned clone.
        let iterable = other.clone_with_heap(vm.heap);
        deque_extend(self, iterable, ExtendEnd::Right, vm)?;
        Ok(true)
    }

    /// `deque * int` — repetition that keeps the deque's `maxlen`, so a bounded
    /// deque builds only its surviving suffix rather than the full product.
    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = repeat_count(other, vm)? else {
            return Ok(None);
        };
        let maxlen = self.get(vm.heap).maxlen();
        // `count == 0` (and negative counts, clamped to 0) yields an empty deque,
        // so skip cloning the items. Otherwise snapshot the source items so the
        // build holds no heap borrow — a timeout mid-build can then release the
        // clones with `&mut vm`.
        let source: Vec<Value> = if count == 0 {
            Vec::new()
        } else {
            self.get(vm.heap).iter().map(|v| v.clone_with_heap(vm.heap)).collect()
        };
        let result = repeat_deque(source, maxlen, count, vm)?;
        Ok(Some(result))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        // `maxlen` is the deque's only data attribute (read-only in CPython).
        if attr.static_string() == Some(StaticStrings::Maxlen) {
            let value = match self.get(vm.heap).maxlen() {
                Some(max) => Value::Int(i64::try_from(max).expect("maxlen fits in i64")),
                None => Value::None,
            };
            return Ok(Some(CallResult::Value(value)));
        }
        Ok(None)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Deque, attr.as_str(vm.interns)));
        };
        call_deque_method(self, method, args, vm).map(CallResult::Value)
    }
}

impl HeapItem for Deque {
    /// Releases every heap reference the deque owns.
    ///
    /// MUST report exactly the same ids as `for_each_child_id` in `heap.rs` —
    /// too few decrements leaks, too many is a use-after-free.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                obj.dec_ref_forget();
            }
        }
    }
}

/// Reads both deques' mutation counters, for comparing against a captured pair.
///
/// The deque comparisons walk `self` and `other` by index, so a user `__eq__`
/// resizing *either* one invalidates the walk; both counters must be watched.
fn states<'h>(a: &HeapObjectRead<'h, Deque>, b: &HeapObjectRead<'h, Deque>, vm: &VM<'h>) -> (u64, u64) {
    (a.get(vm.heap).state(), b.get(vm.heap).state())
}

/// Iterates over a deque, raising if it is structurally mutated mid-iteration.
///
/// Mirrors [`ListIterator`](super::list::ListIterator) but honors the deque's
/// mutation counter rather than a length check: a `rotate()` or a paired
/// `append()`/`popleft()` keeps the length while still invalidating the
/// iterator, so the captured `state` is the correct sentinel (see
/// [`Deque::state`]).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DequeIterator {
    /// Owned reference to the deque under iteration.
    deque: HeapId,
    /// Index of the next item to yield.
    index: usize,
    /// The deque's mutation counter captured at creation; any change means the
    /// deque was structurally mutated and iteration must raise `RuntimeError`.
    state: u64,
}

impl DequeIterator {
    /// Creates an iterator which takes ownership of one reference to `deque`,
    /// capturing the deque's current mutation counter.
    pub(crate) fn new(deque: HeapId, vm: &VM<'_>) -> Self {
        let HeapData::Deque(d) = vm.heap.get(deque) else {
            unreachable!("deque iterator must reference a deque")
        };
        Self {
            deque,
            index: 0,
            state: d.state(),
        }
    }

    /// Returns the deque kept alive by this iterator.
    pub(crate) fn deque_id(&self) -> HeapId {
        self.deque
    }

    /// Returns the number of items remaining in the deque's current contents.
    pub(crate) fn size_hint(&self, heap: &Heap) -> usize {
        let HeapData::Deque(deque) = heap.get(self.deque) else {
            unreachable!("deque iterator must reference a deque")
        };
        deque.len().saturating_sub(self.index)
    }
}

impl HeapItem for DequeIterator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.deque);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DequeIterator> {
    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::DequeIterator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.clone_value(vm.heap))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let (deque_id, index, state) = {
            let iterator = self.get(vm.heap);
            (iterator.deque, iterator.index, iterator.state)
        };
        let item = {
            let HeapData::Deque(deque) = vm.heap.get(deque_id) else {
                unreachable!("deque iterator must reference a deque")
            };
            // A structural mutation invalidates the iterator, matching CPython's
            // `RuntimeError: deque mutated during iteration`. Checked BEFORE the
            // exhaustion test so a mutation on the final step still raises.
            if deque.state() != state {
                return Err(ExcType::runtime_error_deque_mutated());
            }
            deque.get(index).map(|item| item.clone_with_heap(vm.heap))
        };
        if item.is_some() {
            self.get_mut(vm.heap).index += 1;
        }
        Ok(item)
    }
}

/// Dispatches a method call on a deque.
///
/// The `FromArgs`-style arity wording differs per method because CPython's own C
/// implementation does: `append`/`count`/`copy` use `METH_O`/`METH_NOARGS` (which
/// name the type), while `index`/`insert`/`rotate` use `PyArg_UnpackTuple` (which
/// does not). The messages are reproduced verbatim.
fn call_deque_method<'h>(
    deque: &mut HeapObjectRead<'h, Deque>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    match method {
        StaticStrings::Append => {
            let item = args.get_one_arg("deque.append", vm.heap)?;
            deque.append(vm, item);
            Ok(Value::None)
        }
        StaticStrings::Appendleft => {
            let item = args.get_one_arg("deque.appendleft", vm.heap)?;
            deque.appendleft(vm, item);
            Ok(Value::None)
        }
        StaticStrings::Pop => {
            args.check_zero_args("deque.pop", vm.heap)?;
            let this = deque.get_mut(vm.heap);
            // An empty pop raises without mutating, so it must not bump the state.
            let item = this
                .items
                .pop_back()
                .ok_or_else(ExcType::index_error_pop_from_empty_deque)?;
            this.bump_state();
            Ok(item)
        }
        StaticStrings::Popleft => {
            args.check_zero_args("deque.popleft", vm.heap)?;
            let this = deque.get_mut(vm.heap);
            let item = this
                .items
                .pop_front()
                .ok_or_else(ExcType::index_error_pop_from_empty_deque)?;
            this.bump_state();
            Ok(item)
        }
        StaticStrings::Clear => {
            args.check_zero_args("deque.clear", vm.heap)?;
            let this = deque.get_mut(vm.heap);
            // CPython returns early for an already-empty deque, leaving state alone.
            if this.items.is_empty() {
                return Ok(Value::None);
            }
            this.bump_state();
            let items: Vec<Value> = this.items.drain(..).collect();
            items.drop_with(vm);
            Ok(Value::None)
        }
        StaticStrings::Copy => {
            args.check_zero_args("deque.copy", vm.heap)?;
            let maxlen = deque.get(vm.heap).maxlen();
            let items = deque.clone_all_items(vm)?;
            let (new_deque, evicted) = Deque::new(items, maxlen);
            evicted.drop_with(vm);
            let id = vm.heap.allocate(HeapData::Deque(new_deque));
            Ok(Value::Ref(id))
        }
        StaticStrings::Reverse => {
            args.check_zero_args("deque.reverse", vm.heap)?;
            deque.get_mut(vm.heap).items.make_contiguous().reverse();
            Ok(Value::None)
        }
        StaticStrings::Extend => {
            let iterable = args.get_one_arg("deque.extend", vm.heap)?;
            deque_extend(deque, iterable, ExtendEnd::Right, vm)?;
            Ok(Value::None)
        }
        StaticStrings::Extendleft => {
            let iterable = args.get_one_arg("deque.extendleft", vm.heap)?;
            // extendleft REVERSES the input: each item is pushed to the front in turn.
            deque_extend(deque, iterable, ExtendEnd::Left, vm)?;
            Ok(Value::None)
        }
        StaticStrings::Rotate => rotate(deque, args, vm),
        StaticStrings::Insert => insert(deque, args, vm),
        StaticStrings::Remove => remove(deque, args, vm),
        StaticStrings::Index => index(deque, args, vm),
        StaticStrings::Count => count(deque, args, vm),
        _ => {
            args.drop_with(vm);
            Err(ExcType::attribute_error(Type::Deque, method.into()))
        }
    }
}

/// `deque.rotate([n=1])` — rotates right by `n` (left if negative), wrapping.
fn rotate<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let RotateArgs { n } = RotateArgs::from_args(args, vm)?;
    defer_drop!(n, vm);
    // A big int is a valid rotation count; only one out of `i64` range is an
    // `OverflowError`, matching CPython's `PyNumber_AsSsize_t`.
    let n = match read_ssize(n, vm, ExcType::overflow_c_ssize_t) {
        Some(res) => res?,
        None => return Err(ExcType::type_error_not_an_integer(&n.py_type_name(vm))),
    };
    let this = deque.get_mut(vm.heap);
    let len = this.items.len();
    // CPython bails for len <= 1 without touching state (no iterator invalidation);
    // above that it bumps unconditionally — even `rotate(0)`, hence no `shift != 0`
    // guard.
    if len <= 1 {
        return Ok(Value::None);
    }
    this.bump_state();
    // Reduce modulo len so a huge n doesn't spin; rem_euclid keeps it non-negative.
    let shift = usize::try_from(n.rem_euclid(i64::try_from(len).expect("len fits in i64")))
        .expect("rem_euclid is non-negative");
    this.items.rotate_right(shift);
    Ok(Value::None)
}

/// `deque.insert(i, x)` — raises if the deque is already at `maxlen`.
fn insert<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let InsertArgs {
        index: index_value,
        item,
    } = InsertArgs::from_args(args, vm)?;
    defer_drop!(index_value, vm);
    // Every failing branch below releases `item` through the guard. The insert
    // at the end takes it back out.
    let mut item_guard = DropGuard::new(item, vm);
    let vm = item_guard.ctx();

    // CPython checks fullness before touching the index.
    let this = deque.get(vm.heap);
    if let Some(max) = this.maxlen()
        && this.len() >= max
    {
        return Err(ExcType::index_error_deque_full());
    }

    // A big int is a valid insert position; one out of `i64` range is an
    // `OverflowError` (CPython's `PyNumber_AsSsize_t`), not a type error.
    let raw = match read_ssize(index_value, vm, ExcType::overflow_c_ssize_t) {
        Some(Ok(i)) => i,
        Some(Err(e)) => return Err(e),
        None => return Err(ExcType::type_error_not_an_integer(&index_value.py_type_name(vm))),
    };

    // insert() clamps rather than raising, like list.insert.
    let len = i64::try_from(deque.get(vm.heap).len()).expect("len fits in i64");
    let normalized = if raw < 0 { (raw + len).max(0) } else { raw.min(len) };
    let idx = usize::try_from(normalized).expect("index clamped non-negative");

    let (item, vm) = item_guard.into_parts();
    if matches!(item, Value::Ref(_)) {
        deque.get_mut(vm.heap).contains_refs = true;
    }
    let this = deque.get_mut(vm.heap);
    this.items.insert(idx, item);
    this.bump_state();
    Ok(Value::None)
}

/// `deque.remove(x)` — removes the first item equal to `x`.
fn remove<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let target = args.get_one_arg("deque.remove", vm.heap)?;
    defer_drop!(target, vm);

    let (len, start_state) = {
        let this = deque.get(vm.heap);
        (this.len(), this.state())
    };
    for i in 0..len {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        let eq = item.py_eq(target, vm)?;
        // CPython checks for mutation by the user `__eq__` before acting on the
        // comparison — even a matching one — and quirkily raises IndexError here.
        if deque.get(vm.heap).state() != start_state {
            return Err(ExcType::index_error_deque_mutated());
        }
        if eq {
            let this = deque.get_mut(vm.heap);
            let removed = this.items.remove(i).expect("index in range");
            // Only a successful removal bumps: a `remove()` that raises ValueError
            // leaves CPython's iterators valid (verified against CPython 3.14).
            this.bump_state();
            removed.drop_with(vm);
            return Ok(Value::None);
        }
    }
    Err(ExcType::value_error_deque_remove())
}

/// `deque.index(x[, start[, stop]])` — index of the first item equal to `x`.
fn index<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let IndexArgs {
        value: target,
        start,
        stop,
    } = IndexArgs::from_args(args, vm)?;
    defer_drop!(target, vm);
    defer_drop!(start, vm);
    defer_drop!(stop, vm);

    // Both bounds are coerced before the length is read, because coercion can run a
    // user `__index__` that mutates this deque. Reading the length first would
    // resolve a negative bound against a stale size and let the walk below index
    // past the end of a shortened deque, which panics.
    let start_arg = coerce_bound(start.as_ref(), vm)?;
    let stop_arg = coerce_bound(stop.as_ref(), vm)?;

    let len = deque.get(vm.heap).len();
    let start = start_arg.map_or(0, |i| normalize_sequence_index(i, len));
    let stop = stop_arg.map_or(len, |i| normalize_sequence_index(i, len));

    let start_state = deque.get(vm.heap).state();
    for i in start..stop {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if item.py_eq(target, vm)? {
            return Ok(Value::Int(i64::try_from(i).expect("index fits in i64")));
        }
        // A user `__eq__` mutating the deque invalidates the walk; CPython
        // raises, checking only after a false compare.
        if deque.get(vm.heap).state() != start_state {
            return Err(ExcType::runtime_error_deque_mutated());
        }
    }
    Err(ExcType::value_error_deque_index())
}

/// `deque.count(x)` — number of items equal to `x`.
fn count<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let target = args.get_one_arg("deque.count", vm.heap)?;
    defer_drop!(target, vm);

    let (len, start_state) = {
        let this = deque.get(vm.heap);
        (this.len(), this.state())
    };
    let mut total: i64 = 0;
    for i in 0..len {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if item.py_eq(target, vm)? {
            total += 1;
        }
        // A user `__eq__` mutating the deque invalidates the walk; CPython
        // checks after counting each compare.
        if deque.get(vm.heap).state() != start_state {
            return Err(ExcType::runtime_error_deque_mutated());
        }
    }
    Ok(Value::Int(total))
}

/// Coerces an optional `index` bound to an `i64`, leaving normalization to the caller.
///
/// `None` means "not supplied"; an explicit `Value::None` is a *bad argument*, matching
/// CPython (`index()` bounds go through `_PyEval_SliceIndexNotNone`, unlike real slicing
/// which accepts `None`). Normalization is deliberately not done here — see [`index`].
fn coerce_bound(value: Option<&Value>, vm: &mut VM<'_>) -> RunResult<Option<i64>> {
    match value {
        Some(value) => Ok(Some(value_to_i64_bound(value, vm)?)),
        None => Ok(None),
    }
}

/// Which end [`deque_extend`] appends each item to.
#[derive(Clone, Copy)]
pub(crate) enum ExtendEnd {
    /// `extend` / `+=` — append to the right, in source order.
    Right,
    /// `extendleft` — push each item to the front in turn, which reverses the input.
    Left,
}

/// Extends `deque` in place by every item of `iterable` — `deque.extend`
/// and `extendleft`, and CPython's `deque.__iadd__` (`+=` *is* `extend`).
///
/// Each item is appended as the source yields it, so an iterator that raises
/// part-way leaves the earlier items in the deque, and a bounded deque consumes
/// a long source without ever holding more than it keeps.
///
/// Extending a deque *by itself* is the one case that cannot append while it
/// iterates, or it would chase its own tail; it snapshots the original items
/// first, as CPython does.
pub(crate) fn deque_extend<'h>(
    deque: &mut HeapObjectRead<'h, Deque>,
    iterable: Value,
    end: ExtendEnd,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    if iterable.ref_id() == Some(deque.id()) {
        let items = deque
            .get(vm.heap)
            .iter()
            .map(|item| item.clone_with_heap(vm.heap))
            .collect::<Vec<_>>()
            .into_iter();
        iterable.drop_with(vm);
        defer_drop_mut!(items, vm);
        for item in items.by_ref() {
            deque_push(deque, item, end, vm);
        }
        Ok(())
    } else {
        let iter = iterable.into_py_iter(vm)?;
        defer_drop!(iter, vm);
        let mut iter = iter.read(vm);
        // One-shot preflight from the size hint: exact-hint iterators (e.g.
        // `range`) reject oversized extends with a graceful `MemoryError`,
        // matching `collect_python_iterator`; hint-less iterators fall back
        // to VM checkpoints and the allocator's hard limit. A bounded deque
        // retains at most `maxlen` items however long the iterator, so cap
        // the estimate at what it can actually keep.
        let hint = iter.iter_size_hint(vm);
        let retained = deque.get(vm.heap).maxlen().map_or(hint, |maxlen| hint.min(maxlen));
        check_estimated_size(retained.saturating_mul(VALUE_SIZE), &vm.heap.tracker)?;
        while let Some(item) = iter.py_next(vm)? {
            deque_push(deque, item, end, vm);
        }
        Ok(())
    }
}

/// Appends one item to whichever end the extension targets.
fn deque_push<'h>(deque: &mut HeapRead<'h, Deque>, item: Value, end: ExtendEnd, vm: &mut VM<'h>) {
    match end {
        ExtendEnd::Right => deque.append(vm, item),
        ExtendEnd::Left => deque.appendleft(vm, item),
    }
}

/// Builds `deque * count`, honoring the deque's `maxlen`.
///
/// A bounded deque keeps only its rightmost `maxlen` items, so we build just the
/// surviving suffix rather than the full product — `deque([1,2], maxlen=2) *
/// 10**9` must not materialize two billion `Value`s before truncating. The
/// pattern is periodic with period `len`, so the kept window of `L = min(len*
/// count, maxlen)` starts at `(len - L % len) % len`. An unbounded deque has no
/// shortcut and materializes the full product (may hit resource limits, like
/// CPython's `MemoryError`).
fn repeat_deque(source: Vec<Value>, maxlen: Option<usize>, count: usize, vm: &mut VM<'_>) -> RunResult<Value> {
    let len = source.len();
    // `source` is an owned copy of the deque's items; release it on every exit.
    defer_drop!(source, vm);
    let result = if let Some(max) = maxlen {
        // `kept` is attacker-controlled (a huge `maxlen`), so pre-check that many
        // `Value` slots against the tracker and poll the time limit while building
        // — else the suffix could allocate/spin before the final `allocate` checks.
        let kept = len.saturating_mul(count).min(max);
        check_repeat_size(mem::size_of::<Value>(), kept, &vm.heap.tracker)?;
        // `Vec::new()` (not `with_capacity(kept)`): the check above is the real
        // guard, and reserving an attacker-sized capacity would itself abort.
        // The guard releases the clones built so far if the time poll trips.
        let mut result = DropGuard::new(Vec::new(), vm);
        if kept > 0 {
            let start = (len - kept % len) % len;
            for i in 0..kept {
                let (items, vm) = result.as_parts_mut();
                items.push(source[(start + i % len) % len].clone_with_heap(vm.heap));
                vm.heap.tracker.check_time_every(i)?;
            }
        }
        result.into_inner()
    } else {
        check_repeat_size(len.saturating_mul(mem::size_of::<Value>()), count, &vm.heap.tracker)?;
        let mut result = DropGuard::new(Vec::with_capacity(len * count), vm);
        for rep in 0..count {
            let (items, vm) = result.as_parts_mut();
            items.extend(source.iter().map(|v| v.clone_with_heap(vm.heap)));
            vm.heap.tracker.check_time_every(rep)?;
        }
        result.into_inner()
    };
    // We already trimmed to at most `maxlen`, so `Deque::new` evicts nothing and
    // no refcounts need releasing — `debug_assert` guards that invariant.
    let (new_deque, evicted) = Deque::new(result, maxlen);
    debug_assert!(evicted.is_empty(), "repeat_deque built more than maxlen items");
    Ok(Value::Ref(vm.heap.allocate(HeapData::Deque(new_deque))))
}
