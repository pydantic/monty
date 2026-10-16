//! `functools.lru_cache` / `functools.cache` — a callable that memoizes another
//! callable's results.
//!
//! Hits return directly from native lookup. Misses enter captured Python
//! bytecode which calls the wrapped callable and then a private store helper.
//! Pending keys and arguments are ordinary frame locals, including across
//! host suspension and snapshots; the VM needs no cache-specific return effects.

use std::{cell::Cell, fmt::Write, mem::take};

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::type_of_ref,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{
        Dict, LazyHeapSet, NamedTuple, PyTrait, Type,
        hash_table::{PyHashTable, Slot, TableEntry, TableProbe, TableSource},
        tuple::{TupleVec, allocate_tuple},
    },
    value::{EitherStr, Value},
};

/// A memoizing wrapper around another callable.
///
/// Owns its callable, keys, and results. Both heap cleanup and cycle tracing
/// must enumerate them: cached arguments and results can refer back to the wrapper.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LruCache {
    /// Owned callable, exposed as `__wrapped__`; `None` for a configured
    /// decorator awaiting the function to wrap.
    func: Option<Value>,
    /// Entry ceiling; `None` is unbounded (`cache`), `Some(0)` caches nothing.
    maxsize: Option<u64>,
    /// Whether arguments of different types are cached separately.
    typed: bool,
    /// Calls answered from the cache, as reported by `cache_info()`.
    hits: u64,
    /// Calls that reached the wrapped callable, including ones it then raised
    /// from and ones `maxsize=0` never stored.
    misses: u64,
    /// Call key (see [`make_key`]) to result.
    cache: PyHashTable<CacheEntry>,
    /// Source of stamps; monotonic, so the smallest stamp is the oldest use.
    clock: u64,
}

impl LruCache {
    /// Wraps `func` (taking ownership), or, with `None`, stands in as the
    /// decorator that will wrap the next callable it is given.
    fn new(func: Option<Value>, maxsize: Option<u64>, typed: bool) -> Self {
        Self {
            func,
            maxsize,
            typed,
            hits: 0,
            misses: 0,
            cache: PyHashTable::default(),
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
        for entry in self.cache.live() {
            if let Value::Ref(id) = entry.key {
                on_child(id);
            }
            if let Value::Ref(id) = entry.value {
                on_child(id);
            }
        }
    }

    /// Records a hit without changing the table's keys or slot layout.
    fn touch(&mut self, slot: &Slot) {
        if self.maxsize.is_some() {
            self.clock += 1;
            self.cache.entry(slot).stamp.set(self.clock);
        }
    }
}

/// Answers a hit natively, or transfers a miss to its frozen continuation.
pub(crate) fn call(cache_id: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let HeapReadOutput::LruCache(mut cache) = vm.heap.read(cache_id) else {
        unreachable!("cache dispatch requires an LruCache")
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

    let CallArgs { positional, keywords } = CallArgs::from_args(args, vm)?;
    defer_drop_mut!(positional, vm);
    let keywords = keywords.into_iter().collect::<Vec<_>>();
    defer_drop_mut!(keywords, vm);

    // Disabled caches must accept unhashable arguments and still use a frame,
    // rather than recursively dispatching nested wrappers on the Rust stack.
    let key = if maxsize == Some(0) {
        Value::None
    } else {
        make_key(positional, keywords, typed, vm)?
    };
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();

    // The shared probe revalidates candidates after a user `__eq__` mutates the cache.
    let found = if maxsize == Some(0) {
        None
    } else {
        let hash = key_hash(key, vm)?;
        cache.find_index(key, hash, vm)?
    };
    if let Some(slot) = found {
        let value = cache.get(vm.heap).cache.entry(&slot).value.clone_with_heap(vm);
        let this = cache.get_mut(vm.heap);
        this.hits += 1;
        this.touch(&slot);
        Ok(CallResult::Value(value))
    } else {
        cache.get_mut(vm.heap).misses += 1;
        let kwargs = Dict::from_pairs(take(keywords), vm)?;
        let kwargs = Value::Ref(vm.heap.allocate(HeapData::Dict(kwargs)));
        let args = allocate_tuple(TupleVec::from_vec(take(positional)), vm.heap);
        let store = Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Store));
        let function = func.clone_with_heap(vm);
        vm.heap.inc_ref(cache_id);
        let (key, vm) = key_guard.into_parts();
        vm.call_frozen_exact(
            FrozenFunction::CacheMiss,
            [store, Value::Ref(cache_id), key, function, args, kwargs],
        )
    }
}

/// Captures arbitrary call arguments without interpreting the wrapped signature.
#[derive(FromArgs)]
#[from_args(name = "_lru_cache_wrapper")]
struct CallArgs {
    #[from_args(varargs)]
    positional: Vec<Value>,
    #[from_args(varkwargs)]
    keywords: KwargsValues,
}

/// Argument shape for the decorator `lru_cache(maxsize=…, typed=…)` returns.
///
/// CPython's decorator is a closure over the two settings taking just the
/// function, so this shape is what its arity errors describe; Monty keeps the
/// settings on a function-less wrapper instead.
#[derive(FromArgs)]
#[from_args(name = "lru_cache.<locals>.decorating_function", style = def)]
struct DecoratingFunctionArgs {
    user_function: Value,
}

/// Applies the settings this decorator captured to the function it is given.
fn decorate(maxsize: Option<u64>, typed: bool, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let DecoratingFunctionArgs { user_function } = DecoratingFunctionArgs::from_args(args, vm)?;
    allocate(Some(user_function), maxsize, typed, vm)
}

/// Allocates a wrapper, taking ownership of `func` and rejecting one that
/// cannot be called. `None` allocates the decorator stage.
pub(crate) fn allocate(func: Option<Value>, maxsize: Option<u64>, typed: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    if func.as_ref().is_none_or(|func| func.is_callable(vm.heap)) {
        let cache = LruCache::new(func, maxsize, typed);
        Ok(Value::Ref(vm.heap.allocate(HeapData::LruCache(Box::new(cache)))))
    } else {
        func.drop_with(vm);
        Err(ExcType::partial_not_callable())
    }
}

/// The TypeError `lru_cache()` raises for a first argument that is neither a
/// size, a callable, nor `None`.
#[cold]
pub(crate) fn bad_maxsize_error() -> RunError {
    ExcType::type_error("Expected first argument to be an integer, a callable, or None")
}

/// Hashes a cache key before entering the mutation-aware table probe.
fn key_hash(key: &Value, vm: &mut VM<'_>) -> RunResult<u64> {
    key.py_hash(vm)?
        .map(HashValue::raw)
        .ok_or_else(|| ExcType::type_error_unhashable_dict_key(&key.py_type_name(vm)))
}

/// Stores a successful frozen call's result. Consumes the key; borrows the wrapper and value.
/// Disabled caches deliberately skip hashing and insertion.
pub(crate) fn store_result(cache: &Value, key: Value, value: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts_mut();
    let cache_id = match cache {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LruCache(_)) => *id,
        _ => return Err(ExcType::type_error("store() requires a cache wrapper")),
    };
    let HeapReadOutput::LruCache(mut cache) = vm.heap.read(cache_id) else {
        unreachable!("checked cache type above")
    };
    if cache.get(vm.heap).maxsize == Some(0) {
        return Ok(());
    }

    // A bounded cache preserves an entry inserted by a recursive call.
    // Unbounded caches instead overwrite it with the outer result.
    let hash = key_hash(key, vm)?;
    if let Some(slot) = cache.find_index(key, hash, vm)? {
        if cache.get(vm.heap).maxsize.is_none() {
            let old = cache.get(vm.heap).cache.entry(&slot);
            let entry = CacheEntry {
                key: old.key.clone_with_heap(vm),
                value: value.clone_with_heap(vm),
                hash: old.hash,
                stamp: Cell::new(old.stamp.get()),
            };
            cache.get_mut(vm.heap).cache.replace(slot, entry).drop_with(vm);
        }
        Ok(())
    } else {
        cache.get(vm.heap).cache.check_growth(&vm.heap.tracker)?;
        let (key, vm) = key_guard.into_parts();
        insert_new(&mut cache, key, hash, value, vm);
        Ok(())
    }
}

/// Inserts an owned key and a cloned result, making room in bounded caches.
fn insert_new<'h>(cache: &mut HeapObjectRead<'h, LruCache>, key: Value, hash: u64, value: &Value, vm: &mut VM<'h>) {
    let maxsize = cache.get(vm.heap).maxsize;
    if let Some(maxsize) = maxsize
        && cache.get(vm.heap).cache.len() as u64 >= maxsize
    {
        evict_one(cache, vm);
    }

    let value = value.clone_with_heap(vm);
    let this = cache.get_mut(vm.heap);
    this.clock += 1;
    this.cache.push(CacheEntry {
        key,
        value,
        hash,
        stamp: Cell::new(this.clock),
    });
}

/// Removes the least recently used entry by position without invoking user code.
/// A lookup by key could run `__eq__` and mutate both the cache and its stamps.
fn evict_one<'h>(cache: &mut HeapObjectRead<'h, LruCache>, vm: &mut VM<'h>) {
    let table = &cache.get(vm.heap).cache;
    if let Some(oldest) = table.live().min_by_key(|entry| entry.stamp.get()) {
        let slot = table
            .find(oldest.hash, |entry| entry.stamp == oldest.stamp)
            .expect("oldest entry is in the cache");
        cache.get_mut(vm.heap).cache.remove(slot).drop_with(vm);
    }
}

/// Builds and hashes a key, preserving keyword order and optional argument types.
/// A leading positional count separates positionals from keyword pairs without
/// CPython's private sentinel. Single untyped int/str arguments bypass the tuple.
fn make_key(positional: &[Value], keywords: &[(Value, Value)], typed: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    // This shortcut distinguishes f(1) from f(1.0), despite their equal arguments.
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
        for mut entry in self.cache.take_entries() {
            entry.key.py_dec_ref_ids(stack);
            entry.value.py_dec_ref_ids(stack);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, LruCache> {
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        call(self.id(), args, vm)
    }

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

    /// Fallback repr; `Value::py_repr_fmt` adds the identity through [`lru_cache_repr`].
    fn py_repr_fmt(&self, f: &mut impl Write, _: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(f.write_str("<functools._lru_cache_wrapper object>")?)
    }

    /// `__wrapped__` — the callable being cached.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        if attr.static_string(vm.interns) == Some(StaticStrings::DunderWrapped)
            && let Some(func) = self.get(vm.heap).func.as_ref()
        {
            let func = func.clone_with_heap(vm);
            Ok(Some(CallResult::Value(func)))
        } else {
            Ok(None)
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let value = match attr.static_string(vm.interns) {
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
            EitherStr::Interned(vm.interns.intern_static(StaticStrings::CacheInfoName)),
            vec![
                EitherStr::Interned(vm.interns.intern_static(StaticStrings::Hits)),
                EitherStr::Interned(vm.interns.intern_static(StaticStrings::Misses)),
                EitherStr::Interned(vm.interns.intern_static(StaticStrings::Maxsize)),
                EitherStr::Interned(vm.interns.intern_static(StaticStrings::Currsize)),
            ],
            items,
        );
        Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(info))))
    }

    /// Empties the cache and resets the hit/miss counters, as CPython does.
    fn cache_clear(&mut self, vm: &mut VM<'h>) {
        let mut entries = take(&mut self.get_mut(vm.heap).cache);
        entries.take_entries().drop_with(vm);
        let this = self.get_mut(vm.heap);
        this.hits = 0;
        this.misses = 0;
    }

    /// `{'maxsize': ..., 'typed': ...}`.
    fn cache_parameters(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let this = self.get(vm.heap);
        let pairs = vec![
            (
                Value::InternString(vm.interns.intern_static(StaticStrings::Maxsize)),
                maxsize_value(this.maxsize),
            ),
            (
                Value::InternString(vm.interns.intern_static(StaticStrings::Typed)),
                Value::Bool(this.typed),
            ),
        ];
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }
}

/// A cached result and its recency, kept together when the table compacts tombstones.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    /// Owned call key and result.
    key: Value,
    value: Value,
    /// Cached key hash; eviction never calls user code.
    hash: u64,
    /// Interior mutability updates recency without allowing table keys to change.
    stamp: Cell<u64>,
}

impl TableEntry for CacheEntry {
    fn probe_key(&self) -> &Value {
        &self.key
    }

    fn hash(&self) -> u64 {
        self.hash
    }

    fn tombstone() -> Self {
        Self {
            key: Value::Undefined,
            value: Value::Undefined,
            hash: 0,
            stamp: Cell::new(0),
        }
    }

    fn is_live(&self) -> bool {
        !matches!(self.key, Value::Undefined)
    }

    fn changed_size_error() -> RunError {
        ExcType::runtime_error_dict_changed_size()
    }
}

impl<C: ContainsHeap> DropWithContext<C> for CacheEntry {
    fn drop_with(self, context: &mut C) {
        self.key.drop_with(context);
        self.value.drop_with(context);
    }
}

impl<'h> TableSource<'h> for HeapObjectRead<'h, LruCache> {
    type Entry = CacheEntry;

    fn table<'r>(&self, vm: &'r VM<'h>) -> &'r PyHashTable<CacheEntry> {
        &self.get(vm.heap).cache
    }
}

/// Writes the wrapper's repr using the identity available at the `Value` level.
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
fn maxsize_value(maxsize: Option<u64>) -> Value {
    maxsize.map_or(Value::None, count_value)
}
