//! `functools.lru_cache` / `functools.cache` — a callable that memoizes another
//! callable's results.
//!
//! Calling one is dispatched by `VM::call_heap_callable` to [`call_lru_cache`],
//! which either answers from the stored results or runs the wrapped callable as
//! an ordinary frame, tagged with a [`CacheStore`] so its return value is
//! stored on the way out. Nothing here calls Python, so a cached function may
//! still suspend to the host mid-call.

use std::{
    fmt::Write,
    mem::{replace, take},
};

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::type_of_ref,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead,
        HeapReadOutput, heap_read_ref_as_field_mut,
    },
    intern::StaticStrings,
    types::{
        Dict, LazyHeapSet, NamedTuple, PyTrait, Type,
        tuple::{TupleVec, allocate_tuple},
    },
    value::{EitherStr, Value},
};

/// A memoizing wrapper around another callable.
///
/// `func`, the cached keys and the cached values are OWNED refs, so
/// `py_dec_ref_ids` and `for_each_child_id` must enumerate all of them — a
/// cached value routinely closes a cycle back to the wrapper (`@cache` on a
/// method, whose keys hold the instance).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LruCache {
    /// The wrapped callable, exposed as `__wrapped__`.
    ///
    /// `None` while this is still the decorator `lru_cache(maxsize=…)` handed
    /// back, waiting for the function to wrap: calling it with that function
    /// produces the real cache. CPython uses a closure for that stage, so its
    /// decorator is a `function` rather than a wrapper (see
    /// `limitations/functools.md`).
    func: Option<Value>,
    /// Entry ceiling; `None` is unbounded (`cache`), `Some(0)` caches nothing.
    maxsize: Option<u32>,
    /// Whether arguments of different types are cached separately.
    typed: bool,
    /// Calls answered from the cache, as reported by `cache_info()`.
    hits: u64,
    /// Calls that reached the wrapped callable, including ones it then raised
    /// from and ones `maxsize=0` never stored.
    misses: u64,
    /// Call key (see [`make_key`]) to result.
    cache: Dict,
    /// One use stamp per `cache` entry, in the same order, so the least
    /// recently used entry can be found when the cache is full.
    ///
    /// Left empty while `maxsize` is `None`: nothing is ever evicted then, so
    /// recency is dead weight. Every `cache` mutation must mirror the entry's
    /// position here — see [`LruCache::touch`] and [`evict_one`].
    stamps: Vec<u64>,
    /// Source of stamps; monotonic, so the smallest stamp is the oldest use.
    clock: u64,
}

impl LruCache {
    /// Wraps `func` (taking ownership), or, with `None`, stands in as the
    /// decorator that will wrap the next callable it is given.
    fn new(func: Option<Value>, maxsize: Option<u32>, typed: bool) -> Self {
        Self {
            func,
            maxsize,
            typed,
            hits: 0,
            misses: 0,
            cache: Dict::new(),
            stamps: Vec::new(),
            clock: 0,
        }
    }

    /// Invokes `on_child` for each heap id this wrapper owns (GC trace hook).
    ///
    /// MUST report exactly the ids [`HeapItem::py_dec_ref_ids`] releases.
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(Value::Ref(id)) = self.func {
            on_child(id);
        }
        if !self.cache.has_refs() {
            return;
        }
        for (key, value) in &self.cache {
            if let Value::Ref(id) = key {
                on_child(*id);
            }
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Records a use of the entry at `index`; a no-op while unbounded.
    ///
    /// Ignores an index with no stamp: a user `__eq__` running during the key
    /// lookup can clear or shrink the cache before the index is used.
    fn touch(&mut self, index: usize) {
        if self.maxsize.is_some() {
            self.clock += 1;
            if let Some(stamp) = self.stamps.get_mut(index) {
                *stamp = self.clock;
            }
        }
    }

    /// Index of the least recently used entry.
    ///
    /// A linear scan rather than CPython's O(1) intrusive linked list: it runs
    /// only when a full cache takes a new entry, which has just paid for a
    /// whole call of the wrapped function.
    fn lru_index(&self) -> Option<usize> {
        self.stamps
            .iter()
            .enumerate()
            .min_by_key(|(_, stamp)| **stamp)
            .map(|(index, _)| index)
    }
}

/// Calls the cached function `cache_id` with `args`.
///
/// A hit answers straight from the stored results. A miss calls the wrapped
/// callable and tags the pushed frame with a [`CacheStore`], so the result is
/// stored when — and only when — the call returns normally; a call that raises
/// leaves the cache untouched, as CPython's does.
pub(crate) fn call_lru_cache(cache_id: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let HeapReadOutput::LruCache(mut cache) = vm.heap.read(cache_id) else {
        unreachable!("call_lru_cache is only reached with an LruCache")
    };
    let (func, maxsize, typed) = {
        let this = cache.get(vm.heap);
        (
            this.func.as_ref().map(|func| func.clone_with_heap(vm)),
            this.maxsize,
            this.typed,
        )
    };
    // Still a decorator: this call supplies the function to wrap.
    let Some(func) = func else {
        return decorate(maxsize, typed, args, vm).map(CallResult::Value);
    };
    defer_drop!(func, vm);

    let (positional, keywords) = args.into_parts();
    let positional = positional.collect::<Vec<_>>();
    defer_drop_mut!(positional, vm);
    let keywords = keywords.into_iter().collect::<Vec<_>>();
    defer_drop_mut!(keywords, vm);

    // `maxsize=0` stores nothing, so skip building a key it would only throw
    // away; the wrapper then just counts calls.
    if maxsize == Some(0) {
        cache.get_mut(vm.heap).misses += 1;
        let args = take_args(positional, keywords);
        return call_wrapped(func, args, None, vm);
    }

    let key = make_key(positional, keywords, typed, vm)?;
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();

    // Comparing keys can run a user `__eq__`, which is free to call
    // `cache_clear()` on this very wrapper, so the index is read back rather
    // than trusted; a vanished entry counts as a miss.
    let hit = match cache_mut(&mut cache).find_entry_index(key, vm)? {
        Some(index) => cache
            .get(vm.heap)
            .cache
            .value_at(index)
            .map(|value| (index, value.clone_with_heap(vm))),
        None => None,
    };
    if let Some((index, value)) = hit {
        let this = cache.get_mut(vm.heap);
        this.hits += 1;
        this.touch(index);
        Ok(CallResult::Value(value))
    } else {
        cache.get_mut(vm.heap).misses += 1;
        let (key, vm) = key_guard.into_parts();
        vm.heap.inc_ref(cache_id);
        let store = CacheStore { cache: cache_id, key };
        let args = take_args(positional, keywords);
        call_wrapped(func, args, Some(store), vm)
    }
}

/// Argument shape for the decorator `lru_cache(maxsize=…, typed=…)` returns.
///
/// CPython's decorator is a closure over the two settings taking just the
/// function, so this shape is what its arity errors describe; Monty keeps the
/// settings on a function-less wrapper instead.
#[derive(FromArgs)]
#[from_args(name = "lru_cache.<locals>.decorating_function", style = def)]
struct DecoratingFunctionArgs {
    #[from_args(pos_only)]
    user_function: Value,
}

/// Applies the settings this decorator captured to the function it is given.
fn decorate(maxsize: Option<u32>, typed: bool, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let DecoratingFunctionArgs { user_function } = DecoratingFunctionArgs::from_args(args, vm)?;
    allocate(Some(user_function), maxsize, typed, vm)
}

/// Allocates a wrapper, taking ownership of `func` and rejecting one that
/// cannot be called. `None` allocates the decorator stage.
pub(crate) fn allocate(func: Option<Value>, maxsize: Option<u32>, typed: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    if func.as_ref().is_none_or(|func| func.is_callable(vm.heap)) {
        let cache = LruCache::new(func, maxsize, typed);
        Ok(Value::Ref(vm.heap.allocate(HeapData::LruCache(Box::new(cache)))))
    } else {
        func.drop_with(vm);
        Err(ExcType::partial_not_callable())
    }
}

/// The cached results as a dict handle, for the `HeapRead<Dict>` methods.
fn cache_mut<'r, 'h>(cache: &'r mut HeapObjectRead<'h, LruCache>) -> BorrowedHeapReadMut<'r, 'h, Dict> {
    heap_read_ref_as_field_mut!(cache, LruCache, cache)
}

/// Reassembles the call arguments taken apart to build the key.
fn take_args(positional: &mut Vec<Value>, keywords: &mut Vec<(Value, Value)>) -> ArgValues {
    ArgValues::ArgsKargs {
        args: take(positional),
        kwargs: if keywords.is_empty() {
            KwargsValues::Empty
        } else {
            KwargsValues::Pairs(take(keywords))
        },
    }
}

/// Runs the wrapped callable, arranging for `store` to receive its result.
///
/// A plain function call gets the store hung off its frame; anything answering
/// immediately (a builtin, a class, an `async def` handing back its coroutine)
/// is stored right here. A call that suspends to the host instead — only
/// reachable when the *wrapped callable itself* is external — passes through
/// uncached, since its result comes back through neither path.
fn call_wrapped(func: &Value, args: ArgValues, store: Option<CacheStore>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let result = match vm.call_function(func, args) {
        Ok(result) => result,
        Err(error) => {
            store.drop_with(vm);
            return Err(error);
        }
    };
    let Some(store) = store else { return Ok(result) };
    match result {
        CallResult::FramePushed => {
            vm.push_frame_cache_store(store);
            Ok(CallResult::FramePushed)
        }
        CallResult::Value(value) => {
            store_result(store, &value, vm)?;
            Ok(CallResult::Value(value))
        }
        other => {
            store.drop_with(vm);
            Ok(other)
        }
    }
}

/// Stores `value` in each pending cache, in the order the wrappers were
/// entered. Consumes every store; on the first failure the rest are released
/// unstored, since the value never reaches their callers either.
pub(crate) fn store_results(stores: Vec<CacheStore>, value: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let mut stores = stores.into_iter();
    let result = stores.try_for_each(|store| store_result(store, value, vm));
    stores.drop_with(vm);
    result
}

/// Stores `value` under the store's key, evicting the least recently used entry
/// first if the cache is full. Consumes `store`, releasing both its references.
fn store_result(store: CacheStore, value: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let CacheStore { cache: cache_id, key } = store;
    let result = store_into(cache_id, key, value, vm);
    // The store's own reference goes last: releasing it while the read handle
    // below is still alive would try to free an entry that has a live reader.
    vm.heap.dec_ref(cache_id);
    result
}

/// The body of [`store_result`], scoped so its read handle is released before
/// the caller drops the reference that kept the cache alive.
fn store_into(cache_id: HeapId, key: Value, value: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapReadOutput::LruCache(mut cache) = vm.heap.read(cache_id) else {
        unreachable!("a CacheStore only ever names an LruCache")
    };
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();

    // The wrapped call may have stored this key itself — through a recursive
    // call, or a re-entrant `__eq__` reaching the cache — so replace in place
    // rather than assuming the entry is still absent.
    let existing = match cache_mut(&mut cache).find_entry_index(key, vm)? {
        Some(index) if index < cache.get(vm.heap).cache.len() => Some(index),
        _ => None,
    };
    if let Some(index) = existing {
        let value = value.clone_with_heap(vm);
        let old = cache_mut(&mut cache).replace_value_at(index, value, vm);
        old.drop_with(vm);
        cache.get_mut(vm.heap).touch(index);
        Ok(())
    } else {
        insert_new(&mut cache, &mut key_guard, value)
    }
}

/// The miss half of [`store_result`]: makes room if the cache is full, then
/// takes the key out of its guard and inserts it.
fn insert_new<'h>(
    cache: &mut HeapObjectRead<'h, LruCache>,
    key_guard: &mut DropGuard<'_, VM<'h>, Value>,
    value: &Value,
) -> RunResult<()> {
    let (_, vm) = key_guard.as_parts_mut();
    let maxsize = cache.get(vm.heap).maxsize;
    if let Some(maxsize) = maxsize
        && cache.get(vm.heap).cache.len() >= maxsize as usize
    {
        evict_one(cache, vm)?;
    }

    let key = replace(key_guard.as_parts_mut().0, Value::None);
    let (_, vm) = key_guard.as_parts_mut();
    let value = value.clone_with_heap(vm);
    // A re-entrant `__eq__` between the probe above and this insertion can have
    // stored the key already, in which case `set` hands back the old value and
    // its refcount with it.
    if let Some(old) = cache_mut(cache).set(key, value, vm)? {
        old.drop_with(vm);
    }

    let this = cache.get_mut(vm.heap);
    if this.maxsize.is_some() {
        this.clock += 1;
        let clock = this.clock;
        // Ordinarily just appends the new entry's stamp. A user `__eq__` that
        // mutated the cache while this call ran can leave the two out of step,
        // and resizing restores the one-stamp-per-entry invariant that `touch`
        // and `evict_one` rely on.
        this.stamps.resize(this.cache.len(), clock);
    }
    Ok(())
}

/// Drops the least recently used entry to make room for a new one.
fn evict_one<'h>(cache: &mut HeapObjectRead<'h, LruCache>, vm: &mut VM<'h>) -> RunResult<()> {
    let Some(index) = cache.get(vm.heap).lru_index() else {
        return Ok(());
    };
    let Some(key) = cache
        .get(vm.heap)
        .cache
        .key_at(index)
        .map(|key| key.clone_with_heap(vm))
    else {
        return Ok(());
    };
    defer_drop!(key, vm);
    // `Dict::pop` removes the entry from the dense entry vec, shifting
    // everything after it down — `stamps` must shift in step.
    if let Some((key, value)) = cache_mut(cache).pop(key, vm)? {
        key.drop_with(vm);
        value.drop_with(vm);
        let this = cache.get_mut(vm.heap);
        if index < this.stamps.len() {
            this.stamps.remove(index);
        }
    }
    Ok(())
}

/// The pending "store this frame's return value" note a cached call leaves on
/// the frame it pushed.
///
/// Owns both halves: the wrapper (which the caller may drop mid-call) and the
/// key. Released by whichever of the return path, the unwind path or frame
/// teardown reaches the frame first, so it is dropped exactly once.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CacheStore {
    /// The [`LruCache`] to store into.
    pub cache: HeapId,
    /// The key the call was looked up under.
    pub key: Value,
}

impl<C: ContainsHeap> DropWithContext<C> for CacheStore {
    fn drop_with(self, heap: &mut C) {
        heap.heap_mut().dec_ref(self.cache);
        self.key.drop_with(heap);
    }
}

/// Builds the key for one call.
///
/// CPython separates the positional arguments from the keyword ones with a
/// private sentinel object; a leading positional count does the same job
/// without needing a value user code can never produce — two calls share a key
/// only if they split their arguments the same way. With `typed`, each
/// argument's type is appended, so `f(1)` and `f(1.0)` stay apart.
///
/// The key is a tuple, so an unhashable argument raises `TypeError` at the
/// first lookup, exactly as hashing CPython's key tuple does.
fn make_key(positional: &[Value], keywords: &[(Value, Value)], typed: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    // CPython's `_make_key` hands back a lone `int` or `str` argument as the
    // key itself rather than wrapping it, and only calls that take the same
    // shortcut can share a key — which is why `f(1)` and `f(1.0)` are separate
    // entries despite `1 == 1.0`.
    if !typed
        && keywords.is_empty()
        && let [only] = positional
        && matches!(only.py_type(vm), Type::Int | Type::Str)
    {
        return Ok(only.clone_with_heap(vm));
    }

    let mut items = TupleVec::new();
    items.push(Value::Int(i64::try_from(positional.len()).unwrap_or(i64::MAX)));
    for arg in positional {
        items.push(arg.clone_with_heap(vm));
    }
    for (name, value) in keywords {
        items.push(name.clone_with_heap(vm));
        items.push(value.clone_with_heap(vm));
    }
    if typed {
        for value in positional.iter().chain(keywords.iter().map(|(_, value)| value)) {
            items.push(type_of_ref(vm, value));
        }
    }

    // CPython hashes the key eagerly (that is what `_HashedSeq` is for), so an
    // unhashable argument is reported as itself; leaving it to the lookup would
    // blame the key tuple built around it instead.
    let key = allocate_tuple(items, vm.heap);
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();
    if key.py_hash(vm)?.is_none() {
        return Err(unhashable_argument_error(positional, keywords, vm));
    }
    Ok(key_guard.into_inner())
}

/// Names the argument that made the key unhashable.
///
/// Falls back to the whole key's type if every argument hashes on its own,
/// which nothing currently produces.
#[cold]
fn unhashable_argument_error(positional: &[Value], keywords: &[(Value, Value)], vm: &mut VM<'_>) -> RunError {
    for value in positional.iter().chain(keywords.iter().map(|(_, value)| value)) {
        if matches!(value.py_hash(vm), Ok(None)) {
            return ExcType::type_error_unhashable(&value.py_type_name(vm));
        }
    }
    ExcType::type_error_unhashable("tuple")
}

impl HeapItem for LruCache {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(func) = &mut self.func {
            func.py_dec_ref_ids(stack);
        }
        self.cache.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, LruCache> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::LruCacheWrapper
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Cached functions hash by identity, as CPython's do — the wrapper defines
    /// neither `__eq__` nor `__hash__`.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// The address-less floor under the real repr: `Value::py_repr_fmt` routes
    /// every wrapper through [`lru_cache_repr`], which has the heap id to show.
    fn py_repr_fmt(&self, f: &mut impl Write, _: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(f.write_str("<functools._lru_cache_wrapper object>")?)
    }

    /// `__wrapped__` — the callable being cached.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        if attr.static_string() == Some(StaticStrings::DunderWrapped)
            && let Some(func) = self.get(vm.heap).func.as_ref()
        {
            let func = func.clone_with_heap(vm);
            Ok(Some(CallResult::Value(func)))
        } else {
            Ok(None)
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let value = match attr.static_string() {
            Some(StaticStrings::CacheInfo) => {
                args.check_zero_args("_lru_cache_wrapper.cache_info", vm.heap)?;
                self.cache_info(vm)
            }
            Some(StaticStrings::CacheClear) => {
                args.check_zero_args("_lru_cache_wrapper.cache_clear", vm.heap)?;
                self.cache_clear(vm);
                Value::None
            }
            Some(StaticStrings::CacheParameters) => {
                args.check_zero_args("_lru_cache_wrapper.cache_parameters", vm.heap)?;
                self.cache_parameters(vm)?
            }
            // `f.__wrapped__(...)` is an ordinary call of the attribute's
            // value, not a method of the wrapper, so hand it straight on —
            // uncached, which is the point of reaching for `__wrapped__`.
            Some(StaticStrings::DunderWrapped) if let Some(func) = self.get(vm.heap).func.as_ref() => {
                let func = func.clone_with_heap(vm);
                defer_drop!(func, vm);
                return vm.call_function(func, args);
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::LruCacheWrapper, attr.as_str(vm.interns)));
            }
        };
        Ok(CallResult::Value(value))
    }
}

impl<'h> HeapObjectRead<'h, LruCache> {
    /// `CacheInfo(hits=..., misses=..., maxsize=..., currsize=...)`.
    fn cache_info(&self, vm: &mut VM<'h>) -> Value {
        let this = self.get(vm.heap);
        let items = vec![
            count_value(this.hits),
            count_value(this.misses),
            maxsize_value(this.maxsize),
            count_value(this.cache.len() as u64),
        ];
        let info = NamedTuple::new(
            StaticStrings::CacheInfoName,
            vec![
                StaticStrings::Hits.into(),
                StaticStrings::Misses.into(),
                StaticStrings::Maxsize.into(),
                StaticStrings::Currsize.into(),
            ],
            items,
        );
        Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(info))))
    }

    /// Empties the cache and resets the hit/miss counters, as CPython does.
    fn cache_clear(&mut self, vm: &mut VM<'h>) {
        let entries = take(&mut self.get_mut(vm.heap).cache);
        entries.drop_with(vm);
        let this = self.get_mut(vm.heap);
        this.stamps.clear();
        this.hits = 0;
        this.misses = 0;
    }

    /// `{'maxsize': ..., 'typed': ...}`.
    fn cache_parameters(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let this = self.get(vm.heap);
        let pairs = vec![
            (
                Value::InternString(StaticStrings::Maxsize.into()),
                maxsize_value(this.maxsize),
            ),
            (
                Value::InternString(StaticStrings::Typed.into()),
                Value::Bool(this.typed),
            ),
        ];
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }
}

/// Writes `<functools._lru_cache_wrapper object at 0x…>`.
///
/// Lives here rather than in `py_repr_fmt` because, like an instance's default
/// repr, it needs the heap id — which only the `Value` level still has.
pub(crate) fn lru_cache_repr(self_id: HeapId, f: &mut impl Write) -> RunResult<()> {
    Ok(write!(
        f,
        "<functools._lru_cache_wrapper object at 0x{:x}>",
        self_id.index()
    )?)
}

/// A counter as a Python `int`, saturating rather than wrapping negative.
fn count_value(count: u64) -> Value {
    Value::Int(i64::try_from(count).unwrap_or(i64::MAX))
}

/// `maxsize` as Python sees it: an `int`, or `None` when unbounded.
fn maxsize_value(maxsize: Option<u32>) -> Value {
    maxsize.map_or(Value::None, |maxsize| Value::Int(i64::from(maxsize)))
}
