use std::{cmp::Ordering, collections::VecDeque, fmt::Write, mem};

use super::{CmpOrder, PyTrait, iter::collect_owned_iterable};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    resource_checks::check_repeat_size,
    types::{LazyHeapSet, Type, list::repr_sequence_fmt, long_int::repeat_count},
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
/// deque silently evicts from the *opposite* end when it overflows, which is what
/// makes it useful as a ring buffer / sliding window. Everything else is
/// list-like, but note a deque is **not** equal to a list with the same items,
/// and it is unhashable.
///
/// # Reference counting
/// Items are stored as owned `Value`s. Callers transfer ownership in (having
/// already inc-ref'd); anything evicted by `maxlen` is dropped here, so eviction
/// paths must not leak. See [`HeapItem::py_dec_ref_ids`] for the teardown side.
///
/// # GC optimization
/// `contains_refs` mirrors `List`: when no item is a `Value::Ref`, the child walk
/// and refcount teardown can skip iteration entirely.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Deque {
    items: VecDeque<Value>,
    /// Maximum length; `None` means unbounded. Read-only from Python.
    maxlen: Option<usize>,
    /// True if any item is a `Value::Ref` — lets the GC skip iteration.
    contains_refs: bool,
    /// Bumped by every *structural* mutation, so a live iterator can tell it has
    /// been invalidated. Mirrors CPython's `dequeobject.state`: a length check is
    /// not enough, since `rotate()` and a paired `append()`/`popleft()` leave the
    /// length untouched. See [`Deque::bump_state`] for what does and doesn't count.
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

        // `maxlen` is validated before the iterable is consumed, matching CPython.
        // Both failing branches must release `iterable`, which is already bound:
        // a caught error would otherwise pin it for the rest of the run. An
        // explicit `None` means unbounded; a big int is accepted but overflows to
        // CPython's `OverflowError` rather than a type error.
        let raw_maxlen = if let Value::None = maxlen {
            None
        } else {
            let parsed = read_ssize(&maxlen, vm, ExcType::overflow_c_ssize_t);
            maxlen.drop_with(vm);
            match parsed {
                Some(Ok(i)) => Some(i),
                Some(Err(e)) => {
                    iterable.drop_with(vm);
                    return Err(e);
                }
                None => {
                    iterable.drop_with(vm);
                    return Err(ExcType::type_error_integer_required());
                }
            }
        };
        let maxlen = match raw_maxlen.map(check_maxlen).transpose() {
            Ok(maxlen) => maxlen,
            Err(e) => {
                iterable.drop_with(vm);
                return Err(e);
            }
        };

        // An omitted iterable builds an empty deque; an explicit `None` (or any
        // other non-iterable) falls through to `collect_owned_iterable`, which
        // raises CPython's `'NoneType' object is not iterable`.
        let items = match iterable {
            None => Vec::new(),
            Some(v) => collect_owned_iterable(v, vm)?,
        };

        let (deque, evicted) = Self::new(items, maxlen);
        // Items dropped by the maxlen truncation still hold their refcounts.
        for value in evicted {
            value.drop_with(vm);
        }
        let heap_id = vm.heap.allocate(HeapData::Deque(deque))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Rejects a negative `maxlen`, converting a validated one to `usize`.
fn check_maxlen(n: i64) -> RunResult<usize> {
    if n < 0 {
        Err(ExcType::value_error_maxlen_negative())
    } else {
        Ok(usize::try_from(n).expect("maxlen validated non-negative"))
    }
}

/// Reads a deque integer argument (`int`, `bool`, or big `int`) as an `i64`.
///
/// A big int (heap `LongInt`) is a real `int`, so it is accepted rather than
/// rejected as a non-integer; only `InternLongInt` is skipped since it is always
/// materialised to a heap `LongInt` before reaching a runtime operation. Returns
/// `None` for a genuinely non-integer value (the caller raises its own type
/// error), and `Some(Err)` when a big int overflows `i64` — CPython's
/// `PyNumber_AsSsize_t` failure, whose exception kind the caller supplies
/// (`OverflowError` for `maxlen`/`rotate`/`insert`, `IndexError` for subscript).
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
    pub fn append(&mut self, vm: &mut VM<'h>, item: Value) -> RunResult<()> {
        vm.heap.track_growth(VALUE_SIZE)?;
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        let this = self.get_mut(vm.heap);
        this.items.push_back(item);
        this.bump_state();
        let evicted = evict_front_if_full(this);
        if let Some(value) = evicted {
            // The deque is at `maxlen`, so this append grew it by nothing —
            // give back the slot charged above, or a bounded deque would
            // exhaust the memory limit after enough appends.
            vm.heap.track_shrink(VALUE_SIZE);
            value.drop_with(vm);
        }
        Ok(())
    }

    /// Appends to the left, evicting from the right if `maxlen` is reached.
    pub fn appendleft(&mut self, vm: &mut VM<'h>, item: Value) -> RunResult<()> {
        vm.heap.track_growth(VALUE_SIZE)?;
        if matches!(item, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }
        let this = self.get_mut(vm.heap);
        this.items.push_front(item);
        this.bump_state();
        let evicted = evict_back_if_full(this);
        if let Some(value) = evicted {
            // Net-zero growth — see the note in `append`.
            vm.heap.track_shrink(VALUE_SIZE);
            value.drop_with(vm);
        }
        Ok(())
    }

    /// Clones every item, incrementing refcounts — used by `copy`, `+` and `*`.
    fn clone_all_items(&self, vm: &mut VM<'h>) -> Vec<Value> {
        let len = self.get(vm.heap).len();
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let item = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            out.push(item);
        }
        out
    }

    /// Resolves a Python index (negative counts from the right) to a real one.
    fn resolve_index(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<usize> {
        // A slice (or any non-integer) is rejected with CPython's sequence wording,
        // which names the offending type rather than the container.
        if let Value::Ref(id) = key
            && matches!(vm.heap.get(*id), HeapData::Slice(_))
        {
            return Err(ExcType::type_error_sequence_index("slice"));
        }
        // A big int is still a valid index: accept it, raising CPython's
        // index-sized `IndexError` when it can't fit a real index.
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

impl<'h> PyTrait<'h> for HeapRead<'h, Deque> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// `in` walks the deque comparing each item by `==`, like `list`.
    fn py_contains_impl(&self, _self_id: HeapId, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let len = self.get(vm.heap).len();
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
        }
        Ok(Some(false))
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Deque
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let deque_id = self_id.expect("heap values have an id");
        let iterator = vm
            .heap
            .allocate(HeapData::DequeIterator(DequeIterator::new(deque_id, vm)))?;
        vm.heap.inc_ref(deque_id);
        Ok(Value::Ref(iterator))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> bool {
        self.get(vm.heap).len() > 0
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
        // Charge a recursion level before descending into the items: two
        // *distinct* cyclic deques (`a.append(a); b.append(b); a == b`) re-enter
        // here once per level, and would otherwise recurse until the host stack
        // overflows. `List`/`Tuple` take the same bound from the token their
        // iterators hold; a deque walks by index, so it charges directly.
        let mut depth = vm.recursion_guard()?;
        let vm = &mut *depth;
        for i in 0..len {
            let a = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(a, vm);
            let b = other.get(vm.heap).items[i].clone_with_heap(vm.heap);
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(Some(false));
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
        let mut depth = vm.recursion_guard()?;
        let vm = &mut *depth;
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
        }
        // All shared items equal — the shorter deque sorts first.
        Ok(CmpOrder::Ordered(self_len.cmp(&other_len)))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let len = self.get(vm.heap).len();
        f.write_str("deque(")?;
        repr_sequence_fmt('[', ']', len, |heap, i| &self.get(heap).items[i], f, vm, heap_ids)?;
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
        let mut items = self.clone_all_items(vm);
        items.extend(other.clone_all_items(vm));
        let (deque, evicted) = Deque::new(items, maxlen);
        for value in evicted {
            value.drop_with(vm.heap);
        }
        let id = vm.heap.allocate(HeapData::Deque(deque))?;
        Ok(Some(Value::Ref(id)))
    }

    /// `deque * int` — repetition that keeps the deque's `maxlen`, so a bounded
    /// deque builds only its surviving suffix rather than the full product.
    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = repeat_count(other, vm)? else {
            return Ok(None);
        };
        let result = repeat_deque(self.get(vm.heap), count, vm)?;
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

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Deque, attr.as_str(vm.interns)));
        };
        call_deque_method(self, method, args, vm).map(CallResult::Value)
    }
}

impl HeapItem for Deque {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.items.len() * VALUE_SIZE
    }

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
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.deque);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, DequeIterator> {
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

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let self_id = self_id.expect("heap values have an id");
        vm.heap.inc_ref(self_id);
        Ok(Value::Ref(self_id))
    }

    fn py_next(&mut self, _: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
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
    deque: &mut HeapRead<'h, Deque>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    match method {
        StaticStrings::Append => {
            let item = args.get_one_arg("deque.append", vm.heap)?;
            deque.append(vm, item)?;
            Ok(Value::None)
        }
        StaticStrings::Appendleft => {
            let item = args.get_one_arg("deque.appendleft", vm.heap)?;
            deque.appendleft(vm, item)?;
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
            for value in items {
                value.drop_with(vm);
            }
            Ok(Value::None)
        }
        StaticStrings::Copy => {
            args.check_zero_args("deque.copy", vm.heap)?;
            let maxlen = deque.get(vm.heap).maxlen();
            let items = deque.clone_all_items(vm);
            let (new_deque, evicted) = Deque::new(items, maxlen);
            for value in evicted {
                value.drop_with(vm);
            }
            let id = vm.heap.allocate(HeapData::Deque(new_deque))?;
            Ok(Value::Ref(id))
        }
        StaticStrings::Reverse => {
            args.check_zero_args("deque.reverse", vm.heap)?;
            deque.get_mut(vm.heap).items.make_contiguous().reverse();
            Ok(Value::None)
        }
        StaticStrings::Extend => {
            let iterable = args.get_one_arg("deque.extend", vm.heap)?;
            let items: Vec<Value> = collect_owned_iterable(iterable, vm)?;
            for item in items {
                deque.append(vm, item)?;
            }
            Ok(Value::None)
        }
        StaticStrings::Extendleft => {
            let iterable = args.get_one_arg("deque.extendleft", vm.heap)?;
            let items: Vec<Value> = collect_owned_iterable(iterable, vm)?;
            // extendleft REVERSES the input: each item is pushed to the front in turn.
            for item in items {
                deque.appendleft(vm, item)?;
            }
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
    // A big int is a valid rotation count; only one out of `i64` range is an
    // `OverflowError`, matching CPython's `PyNumber_AsSsize_t`.
    let parsed = read_ssize(&n, vm, ExcType::overflow_c_ssize_t);
    let n = if let Some(res) = parsed {
        n.drop_with(vm);
        res?
    } else {
        let name = n.py_type_name(vm);
        n.drop_with(vm);
        return Err(ExcType::type_error_not_an_integer(&name));
    };
    let this = deque.get_mut(vm.heap);
    let len = this.items.len();
    // CPython bails out for len <= 1 (rotating is a no-op) WITHOUT touching state, so
    // neither an empty rotate nor a single-item one invalidates an iterator. Above
    // that it bumps unconditionally — even `rotate(0)`, which is why this is not
    // guarded on `shift != 0`.
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

    // CPython checks fullness before touching the index.
    let this = deque.get(vm.heap);
    if let Some(max) = this.maxlen()
        && this.len() >= max
    {
        item.drop_with(vm);
        return Err(ExcType::index_error_deque_full());
    }

    // A big int is a valid insert position; one out of `i64` range is an
    // `OverflowError` (CPython's `PyNumber_AsSsize_t`), not a type error.
    let raw = match read_ssize(index_value, vm, ExcType::overflow_c_ssize_t) {
        Some(Ok(i)) => i,
        Some(Err(e)) => {
            item.drop_with(vm);
            return Err(e);
        }
        None => {
            let name = index_value.py_type_name(vm);
            item.drop_with(vm);
            return Err(ExcType::type_error_not_an_integer(&name));
        }
    };

    // insert() clamps rather than raising, like list.insert.
    let len = i64::try_from(deque.get(vm.heap).len()).expect("len fits in i64");
    let normalized = if raw < 0 { (raw + len).max(0) } else { raw.min(len) };
    let idx = usize::try_from(normalized).expect("index clamped non-negative");

    vm.heap.track_growth(VALUE_SIZE)?;
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

    let len = deque.get(vm.heap).len();
    for i in 0..len {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if item.py_eq(target, vm)? {
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

    let len = deque.get(vm.heap).len();
    // `stop` is already bound, so a failure resolving `start` has to release it
    // before propagating — `bound_arg` only owns the value it was handed.
    let start = match bound_arg(start, 0, len, vm) {
        Ok(start) => start,
        Err(e) => {
            if let Some(stop) = stop {
                stop.drop_with(vm);
            }
            return Err(e);
        }
    };
    let stop = bound_arg(stop, len, len, vm)?;

    for i in start..stop.min(len) {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if item.py_eq(target, vm)? {
            return Ok(Value::Int(i64::try_from(i).expect("index fits in i64")));
        }
    }
    Err(ExcType::value_error_deque_index())
}

/// `deque.count(x)` — number of items equal to `x`.
fn count<'h>(deque: &mut HeapRead<'h, Deque>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let target = args.get_one_arg("deque.count", vm.heap)?;
    defer_drop!(target, vm);

    let len = deque.get(vm.heap).len();
    let mut total: i64 = 0;
    for i in 0..len {
        let item = deque.get(vm.heap).items[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if item.py_eq(target, vm)? {
            total += 1;
        }
    }
    Ok(Value::Int(total))
}

/// Normalizes an optional `start`/`stop` bound for `index`, clamping to `[0, len]`.
///
/// `None` means "not supplied" and falls back to `default`; an explicit
/// `Value::None` is a *bad argument*, matching CPython (`index()` bounds go through
/// `_PyEval_SliceIndexNotNone`, unlike real slicing which accepts `None`). Big ints
/// clamp by sign rather than erroring, since CPython's `__index__` path accepts any
/// int and then clamps.
fn bound_arg(value: Option<Value>, default: usize, len: usize, vm: &mut VM<'_>) -> RunResult<usize> {
    let len_i64 = i64::try_from(len).expect("len fits in i64");
    let Some(value) = value else { return Ok(default) };
    // Match by reference so there is exactly one `drop_with` for the bound, on
    // every path — the accepted ones as well as the rejection below.
    let raw = match &value {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(i64::from(*b)),
        // Out of `i64` range entirely — saturate to the end the sign points at.
        Value::Ref(heap_id) if let HeapData::LongInt(li) = vm.heap.get(*heap_id) => {
            Some(li.to_i64().unwrap_or(if li.is_negative() { 0 } else { len_i64 }))
        }
        _ => None,
    };
    value.drop_with(vm);
    let raw = raw.ok_or_else(ExcType::type_error_slice_indices_no_none)?;
    let normalized = if raw < 0 {
        (raw + len_i64).max(0)
    } else {
        raw.min(len_i64)
    };
    Ok(usize::try_from(normalized).expect("bound clamped non-negative"))
}

/// Extends `deque_id` in place by every item of `iterable`, as CPython's
/// `deque.__iadd__` (`+=` *is* `extend`).
///
/// Any iterable works and a non-iterable raises `TypeError` from the iterator
/// protocol — the reason deque `+=` is driven from the VM rather than the
/// `py_iadd_impl` trait method (which can only surface a `ResourceError`).
/// Items are collected *before* appending so `d += d` extends by the original
/// items and appending never invalidates the iterator draining the source.
pub(crate) fn deque_extend(deque_id: HeapId, iterable: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let items: Vec<Value> = collect_owned_iterable(iterable, vm)?;
    let HeapReadOutput::Deque(mut deque) = vm.heap.read(deque_id) else {
        unreachable!("deque id must reference a deque");
    };
    for item in items {
        deque.append(vm, item)?;
    }
    Ok(())
}

/// Builds `deque * count`, honoring the deque's `maxlen`.
///
/// A bounded deque keeps only its rightmost `maxlen` items, so `deque([1, 2],
/// maxlen=2) * 10**9` is `deque([1, 2], maxlen=2)` — CPython never materializes
/// the full product, and neither must we (two billion `Value`s would OOM the
/// host well before the maxlen truncation ran). We build only the surviving
/// suffix: the repeated sequence is periodic with period `len`, so the kept
/// window of `L = min(len*count, maxlen)` items starts at `(len - L % len) % len`
/// within the pattern and wraps. An unbounded deque has no such shortcut and
/// materializes the full product (which may exhaust resource limits, matching
/// CPython's `MemoryError` on an impossibly large repeat).
fn repeat_deque(deque: &Deque, count: usize, vm: &VM<'_>) -> RunResult<Value> {
    let len = deque.len();
    let result = if let Some(max) = deque.maxlen() {
        // Bounded: keep only the last `min(len*count, maxlen)` items. `kept` is
        // still attacker-controlled (a huge `maxlen` like `deque(maxlen=10**9)`),
        // so pre-check that many `Value` slots against the memory tracker and
        // poll the time limit while building — otherwise the suffix could
        // allocate/spin before the final `allocate` ever consults a limit.
        let kept = len.saturating_mul(count).min(max);
        check_repeat_size(mem::size_of::<Value>(), kept, vm.heap.tracker())?;
        // `Vec::new()` (not `with_capacity(kept)`): the check above is the real
        // guard, and reserving an attacker-sized capacity would itself abort.
        let mut result = Vec::new();
        if kept > 0 {
            // `len*count` is a multiple of `len`, so dropping the leading
            // items down to `kept` survivors starts at this offset.
            let start = (len - kept % len) % len;
            for i in 0..kept {
                let v = deque.get((start + i % len) % len).expect("index within deque");
                result.push(v.clone_with_heap(vm.heap));
                // Poll once per notional copy of the deque, matching the
                // unbounded branch's cadence.
                if i % len == 0 {
                    vm.heap.check_time()?;
                }
            }
        }
        result
    } else {
        // Unbounded: materialize the full product.
        check_repeat_size(len.saturating_mul(mem::size_of::<Value>()), count, vm.heap.tracker())?;
        let mut result = Vec::with_capacity(len * count);
        for _ in 0..count {
            result.extend(deque.iter().map(|v| v.clone_with_heap(vm.heap)));
            vm.heap.check_time()?;
        }
        result
    };
    // We already trimmed to at most `maxlen`, so `Deque::new` evicts nothing and
    // no refcounts need releasing — `debug_assert` guards that invariant.
    let (new_deque, evicted) = Deque::new(result, deque.maxlen());
    debug_assert!(evicted.is_empty(), "repeat_deque built more than maxlen items");
    Ok(Value::Ref(vm.heap.allocate(HeapData::Deque(new_deque))?))
}
