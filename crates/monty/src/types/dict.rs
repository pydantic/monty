use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem, slice, vec,
};

use hashbrown::HashTable;
use serde::ser::SerializeStruct;
use smallvec::{SmallVec, smallvec};

use super::{DictItemsView, DictKeysView, DictValuesView, LazyHeapSet, PyTrait, allocate_tuple};
use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    resource::ResourceTracker,
    types::Type,
    value::{EitherStr, VALUE_SIZE, Value},
};

/// Python dict type preserving insertion order.
///
/// This type provides Python dict semantics including dynamic key-value namespaces,
/// reference counting for heap values, and standard dict methods.
///
/// # Implemented Methods
/// - `get(key[, default])` - Get value or default
/// - `keys()` - Return view of keys
/// - `values()` - Return view of values
/// - `items()` - Return view of (key, value) pairs
/// - `pop(key[, default])` - Remove and return value
/// - `clear()` - Remove all items
/// - `copy()` - Shallow copy
/// - `update(other)` - Update from dict or iterable of pairs
/// - `setdefault(key[, default])` - Get or set default value
/// - `popitem()` - Remove and return last (key, value) pair
/// - `fromkeys(iterable[, value])` - Create dict from keys (classmethod)
///
/// All dict methods from Python's builtins are implemented.
///
/// # Storage Strategy
/// Uses a `HashTable<usize>` for hash lookups combined with a dense `Vec<DictEntry>`
/// to preserve insertion order (matching Python 3.7+ behavior). The hash table maps
/// key hashes to indices in the entries vector. This design provides O(1) lookups
/// while maintaining insertion order for iteration.
///
/// # Reference Counting
/// When values are added via `set()`, their reference counts are incremented.
/// When using `from_pairs()`, ownership is transferred without incrementing refcounts
/// (caller must ensure values' refcounts account for the dict's reference).
///
/// # GC Optimization
/// The `contains_refs` flag tracks whether the dict contains any `Value::Ref` items.
/// This allows `collect_child_ids` and `py_dec_ref_ids` to skip iteration when the
/// dict contains only primitive values (ints, bools, None, etc.), significantly
/// improving GC performance for dicts of primitives.
#[derive(Debug, Default)]
pub(crate) struct Dict {
    /// indices mapping from the entry hash to its index.
    indices: HashTable<usize>,
    /// entries is a dense vec maintaining entry order.
    entries: Vec<DictEntry>,
    /// True if any key or value in the dict is a `Value::Ref`. Used to skip iteration
    /// in `collect_child_ids` and `py_dec_ref_ids` when no refs are present.
    /// Only transitions from false to true (never back) since tracking removals would be O(n).
    contains_refs: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DictEntry {
    key: Value,
    value: Value,
    /// the hash is needed here for correct use of insert_unique
    hash: u64,
}

impl Dict {
    /// Creates a new empty dict.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            indices: HashTable::with_capacity(capacity),
            entries: Vec::with_capacity(capacity),
            contains_refs: false,
        }
    }

    /// Returns whether this dict contains any heap references (`Value::Ref`).
    ///
    /// Used during allocation to determine if this container could create cycles,
    /// and in `collect_child_ids` and `py_dec_ref_ids` to skip iteration when no refs
    /// are present.
    ///
    /// Note: This flag only transitions from false to true (never back). When a ref is
    /// removed via `pop()`, we do NOT recompute the flag because that would be O(n).
    /// This is conservative - we may iterate unnecessarily if all refs were removed,
    /// but we'll never skip iteration when refs exist.
    #[inline]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        self.contains_refs
    }

    /// Creates a dict from a vector of (key, value) pairs.
    ///
    /// Assumes the caller is transferring ownership of all keys and values in the pairs.
    /// Does NOT increment reference counts since ownership is being transferred.
    /// Returns Err if any key is unhashable (e.g., list, dict).
    pub fn from_pairs(pairs: Vec<(Value, Value)>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let pairs_iter = pairs.into_iter();
        defer_drop_mut!(pairs_iter, vm);
        let dict = Self::with_capacity(pairs_iter.len());
        let mut dict_guard = DropGuard::new(dict, vm);
        let (dict, vm) = dict_guard.as_parts_mut();
        for (key, value) in pairs_iter {
            if let Some(old_value) = dict.set(key, value, vm)? {
                old_value.drop_with(vm);
            }
        }
        Ok(dict_guard.into_inner())
    }

    /// Inserts a JSON object entry whose key is guaranteed to be a string.
    ///
    /// This specialized path avoids the generic `py_eq`/candidate-cloning lookup
    /// used by ordinary dict insertion. JSON object keys are always strings, so
    /// we can compare keys directly by their string contents while preserving the
    /// same duplicate-key semantics as CPython (`{"a": 1, "a": 2}` keeps the
    /// last value and retains the first insertion position).
    pub fn set_json_string_key(
        &mut self,
        key: Value,
        value: Value,
        vm: &mut VM<'_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        debug_assert!(json_key_string_slice(&key, vm.heap, vm.interns).is_some());

        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.contains_refs = true;
        }

        let hash = key
            .py_hash(vm)?
            .expect("json object keys are always hashable strings")
            .raw();
        let opt_index = self.find_json_string_key_index(hash, &key, vm.heap, vm.interns);

        let entry = DictEntry { key, value, hash };
        if let Some(index) = opt_index {
            let old_entry = mem::replace(&mut self.entries[index], entry);
            old_entry.key.drop_with(vm);
            Ok(Some(old_entry.value))
        } else {
            vm.heap.track_growth(2 * VALUE_SIZE)?;
            let index = self.entries.len();
            self.entries.push(entry);
            self.indices.insert_unique(hash, index, |&i| self.entries[i].hash);
            Ok(None)
        }
    }

    /// Finds the existing entry index for a JSON string key.
    ///
    /// The `hash` must match the Python string hash for `key`. Only string keys
    /// participate; any non-string entry is treated as non-equal.
    fn find_json_string_key_index(
        &self,
        hash: u64,
        key: &Value,
        heap: &Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Option<usize> {
        let key_str = json_key_string_slice(key, heap, interns).expect("json object keys are always string values");
        self.indices
            .find(hash, |&idx| {
                let entry = &self.entries[idx];
                entry.hash == hash && json_key_equals_str(&entry.key, key_str, heap, interns)
            })
            .copied()
    }
}

/// Returns the underlying string slice for a JSON object key value.
///
/// JSON object parsing only inserts string keys, but the helper remains
/// defensive and returns `None` for any non-string value.
fn json_key_string_slice<'a>(
    key: &'a Value,
    heap: &'a Heap<impl ResourceTracker>,
    interns: &'a Interns,
) -> Option<&'a str> {
    match key {
        Value::InternString(id) => Some(interns.get_str(*id)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(string) => Some(string.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Returns whether `key` is a string equal to `expected`.
///
/// This bypasses Python's full equality machinery because JSON object keys are
/// always strings, so content comparison is sufficient and much cheaper.
fn json_key_equals_str(key: &Value, expected: &str, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
    match key {
        Value::InternString(id) => interns.get_str(*id) == expected,
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(string) => string.as_str() == expected,
            _ => false,
        },
        _ => false,
    }
}

impl<'h> HeapRead<'h, Dict> {
    /// Element-wise equality against another dict (matching keys and values).
    ///
    /// Shared by `Dict::py_eq_impl` and `Dataclass::py_eq_impl` (which compares
    /// the dataclasses' attribute dicts).
    pub(crate) fn eq_dict(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<bool> {
        if self.get(vm.heap).len() != other.get(vm.heap).len() {
            return Ok(false);
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((key, value)) = iter.next(vm)? {
            let Some(other_value) = other.dict_get(key, vm)? else {
                return Ok(false);
            };
            defer_drop!(other_value, vm);
            if !value.py_eq(other_value, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Gets a value from the dict by key.
    ///
    /// Returns Ok(Some(value)) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    pub(crate) fn dict_get<'a>(
        &'a self,
        key: &Value,
        vm: &'a mut VM<'h, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        let (opt_index, _hash) = self.find_index_hash(key, vm)?;
        if let Some(index) = opt_index {
            Ok(Some(self.get(vm.heap).entries[index].value.clone_with_heap(vm.heap)))
        } else {
            Ok(None)
        }
    }
}

impl Dict {
    /// Gets a value from the dict by string key name (immutable lookup).
    ///
    /// This is an O(1) lookup that doesn't require mutable heap access.
    /// Only works for string keys - returns None if the key is not found.
    pub fn get_by_str(&self, key_str: &str, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<&Value> {
        // Compute hash for the string key
        let mut hasher = DefaultHasher::new();
        key_str.hash(&mut hasher);
        let hash = hasher.finish();

        // Find entry with matching hash and key
        self.indices
            .find(hash, |&idx| {
                let entry_key = &self.entries[idx].key;
                match entry_key {
                    Value::InternString(id) => interns.get_str(*id) == key_str,
                    Value::Ref(id) => {
                        if let HeapData::Str(s) = heap.get(*id) {
                            s.as_str() == key_str
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            })
            .map(|&idx| &self.entries[idx].value)
    }

    /// Sets a key-value pair in the dict.
    ///
    /// The caller transfers ownership of `key` and `value` to the dict. Their refcounts
    /// are NOT incremented here - the caller is responsible for ensuring the refcounts
    /// were already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// If the key already exists, replaces the old value and returns it (caller now
    /// owns the old value and is responsible for its refcount).
    /// Returns Err if key is unhashable.
    pub fn set(&mut self, key: Value, value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        vm.heap.protect_mut(self).set(key, value, vm)
    }
}

impl<'h> HeapRead<'h, Dict> {
    /// Sets a key-value pair in the dict.
    ///
    /// The caller transfers ownership of `key` and `value` to the dict. Their refcounts
    /// are NOT incremented here - the caller is responsible for ensuring the refcounts
    /// were already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// If the key already exists, replaces the old value and returns it (caller now
    /// owns the old value and is responsible for its refcount).
    /// Returns Err if key is unhashable.
    pub fn set(&mut self, key: Value, value: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Value>> {
        // Track if we're adding a reference for GC optimization
        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }

        // Handle hash computation errors explicitly so we can drop key/value properly
        let (opt_index, hash) = match self.find_index_hash(&key, vm) {
            Ok(result) => result,
            Err(e) => {
                // Drop the key and value before returning the error
                key.drop_with(vm);
                value.drop_with(vm);
                return Err(e);
            }
        };

        let entry = DictEntry { key, value, hash };
        if let Some(index) = opt_index {
            // Key exists, replace in place to preserve insertion order
            let old_entry = mem::replace(&mut self.get_mut(vm.heap).entries[index], entry);

            // Decrement refcount for old key (we're discarding it)
            old_entry.key.drop_with(vm);
            // Transfer ownership of the old value to caller (no clone needed)
            Ok(Some(old_entry.value))
        } else {
            // Key doesn't exist — track memory growth before adding the new entry.
            // Growth unit is 2 * size_of::<Value>() to match Dict::py_estimate_size.
            vm.heap.track_growth(2 * VALUE_SIZE)?;
            let this = self.get_mut(vm.heap);
            let index = this.entries.len();
            this.entries.push(entry);
            this.indices
                .insert_unique(hash, index, |index| this.entries[*index].hash);
            Ok(None)
        }
    }

    /// Removes and returns a key-value pair from the dict.
    ///
    /// Returns Ok(Some((key, value))) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    ///
    /// Reference counting: does not decrement refcounts for removed key and value;
    /// caller assumes ownership and is responsible for managing their refcounts.
    pub fn pop(&mut self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<(Value, Value)>> {
        // Find the key using the candidate-based lookup
        let (opt_index, _hash) = self.find_index_hash(key, vm)?;

        if let Some(index) = opt_index {
            // Remove the entry
            let entry = self.get_mut(vm.heap).entries.remove(index);
            // Remove from index table and rebuild (same as dict_popitem)
            let this = self.get_mut(vm.heap);
            this.indices.clear();
            for (idx, e) in this.entries.iter().enumerate() {
                this.indices.insert_unique(e.hash, idx, |&i| this.entries[i].hash);
            }
            Ok(Some((entry.key, entry.value)))
        } else {
            Ok(None)
        }
    }
}

impl Dict {
    /// Returns the number of key-value pairs in the dict.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the dict is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over references to (key, value) pairs.
    pub fn iter(&self) -> DictEntriesIter<'_> {
        self.into_iter()
    }

    /// Returns the key at the given iteration index, or None if out of bounds.
    ///
    /// Used for index-based iteration in for loops. Returns a reference to
    /// the key at the given position in insertion order.
    pub fn key_at(&self, index: usize) -> Option<&Value> {
        self.entries.get(index).map(|e| &e.key)
    }

    /// Returns the value at the given iteration index, or None if out of bounds.
    ///
    /// Dictionary views use this to produce live `dict_values` iteration directly
    /// from the underlying storage without copying the dictionary.
    pub fn value_at(&self, index: usize) -> Option<&Value> {
        self.entries.get(index).map(|e| &e.value)
    }

    /// Returns the key-value pair at the given iteration index, or None if out of bounds.
    ///
    /// This accessor keeps dict-view iteration logic out of the storage internals
    /// while still allowing `dict_items` to produce tuples on demand.
    pub fn item_at(&self, index: usize) -> Option<(&Value, &Value)> {
        self.entries.get(index).map(|entry| (&entry.key, &entry.value))
    }

    /// Creates a dict from the `dict([mapping_or_pairs], **kwargs)` constructor call.
    ///
    /// Supported forms:
    /// - `dict()` returns an empty dict.
    /// - `dict(existing_dict)` returns a shallow copy of the dict.
    /// - `dict(iterable_of_pairs)` consumes `(key, value)` pairs from the iterable.
    /// - `dict(**kwargs)` inserts keyword arguments as string keys.
    ///
    /// Keyword arguments are applied after the optional positional source, matching
    /// CPython precedence (`dict([('a', 1)], a=2)` yields `{'a': 2}`).
    ///
    /// For now, only real `dict` values use mapping-copy semantics; other values
    /// are interpreted as iterables of pairs.
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let DictInitArgs { source, extras } = DictInitArgs::from_args(args, vm)?;
        let dict = Self::new();
        let mut dict_guard = DropGuard::new(dict, vm);

        {
            let (dict, vm) = dict_guard.as_parts_mut();
            let mut kwargs_guard = DropGuard::new(extras, vm);

            if let Some(other_value) = source {
                let other_value_guard = DropGuard::new(other_value, kwargs_guard.ctx());
                let other_value = other_value_guard.into_inner();
                dict_merge_from_value(dict, other_value, kwargs_guard.ctx())?;
            }

            let kwargs = kwargs_guard.into_inner();
            dict_merge_from_kwargs(dict, kwargs, vm)?;
        }

        let dict = dict_guard.into_inner();
        let heap_id = vm.heap.allocate(HeapData::Dict(dict))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Argument shape for `dict([source], **kwargs)`.
///
/// `source` is an optional positional (mapping or iterable of pairs).
/// `extras` collects every additional keyword argument so they can be merged
/// into the dict after the source.
#[derive(FromArgs)]
#[from_args(name = "dict")]
struct DictInitArgs {
    #[from_args(pos_only, default)]
    source: Option<Value>,
    #[from_args(varkwargs)]
    extras: KwargsValues,
}

impl<'h> HeapRead<'h, Dict> {
    fn find_index_hash(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<(Option<usize>, u64)> {
        let hash = key
            .py_hash(vm)?
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(&key.py_type_name(vm)))?
            .raw();

        // Dict keys are typically shallow (strings, ints, tuples of primitives),
        // so recursion errors are unlikely. If one occurs, treat it as "not equal" -
        // the key lookup fails but doesn't crash.
        //
        // Collect candidate indices during the lookup to avoid borrow tracker issues
        let mut candidates: SmallVec<[usize; 2]> = SmallVec::new();
        let this = self.get(vm.heap);
        this.indices.find(hash, |v| {
            if this.entries[*v].hash == hash {
                candidates.push(*v);
            }
            false
        });

        for candidate_index in candidates {
            let candidate_key = self.get(vm.heap).entries[candidate_index].key.clone_with_heap(vm);
            defer_drop!(candidate_key, vm);
            if key.py_eq(candidate_key, vm)? {
                return Ok((Some(candidate_index), hash));
            }
        }

        Ok((None, hash))
    }

    /// Checks whether the dict contains a given key.
    pub(crate) fn contains_key(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<bool> {
        let (opt_index, _hash) = self.find_index_hash(key, vm)?;
        Ok(opt_index.is_some())
    }

    /// Returns a stack-borrowed lending iterator over the dict's
    /// `(key, value)` entries in insertion order, holding a recursion-depth
    /// token for its lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`DictIter`]) because that's the obvious entry point for "iterate
    /// this container".
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter<R: ResourceTracker>(&self, vm: &mut VM<'h, R>) -> RunResult<DictIter<'_, 'h>> {
        DictIter::new(self, vm)
    }

    /// Merges key-value pairs from a dict or iterable-of-pairs into self via HeapRead.
    ///
    /// For dict sources, uses HeapReader::read() to access the source dict through
    /// the heap, enabling self-referential updates like `d.update(d)`.
    fn merge_from_value(&mut self, other_value: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        let mut guard = DropGuard::new(other_value, vm);
        let (other_value, vm) = guard.as_parts_mut();
        if let Value::Ref(id) = other_value {
            let src_id = *id;
            if let HeapReadOutput::Dict(src) = vm.heap.read(src_id) {
                let iter = src.iter(vm)?;
                defer_drop_mut!(iter, vm);
                while let Some((key, value)) = iter.next_owned(vm)? {
                    let old_value = self.set(key, value, vm)?;
                    old_value.drop_with(vm);
                }

                // guard drops other_value here
                return Ok(());
            }
        }

        // Non-dict values are interpreted as iterable-of-pairs
        let (other_value, vm) = guard.into_parts();
        self.merge_from_iterable_pairs(other_value, vm)
    }

    /// Merges key-value pairs from an iterable of 2-item pairs.
    fn merge_from_iterable_pairs(&mut self, iterable: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        let iter = iterable.into_py_iter(vm)?;
        defer_drop!(iter, vm);
        let mut iter = iter.read(vm);

        while let Some(item) = iter.py_next(vm)? {
            let pair_iter = item.into_py_iter(vm)?;
            defer_drop!(pair_iter, vm);
            let mut pair_iter = pair_iter.read(vm);

            let Some(key) = pair_iter.py_next(vm)? else {
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 0; 2 is required",
                ));
            };
            let mut key_guard = DropGuard::new(key, vm);

            let Some(value) = pair_iter.py_next(key_guard.ctx())? else {
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 1; 2 is required",
                ));
            };
            let mut value_guard = DropGuard::new(value, key_guard.ctx());

            if let Some(extra) = pair_iter.py_next(value_guard.ctx())? {
                extra.drop_with(value_guard.ctx());
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length > 2; 2 is required",
                ));
            }

            let value = value_guard.into_inner();
            let key = key_guard.into_inner();

            if let Some(old_value) = self.set(key, value, vm)? {
                old_value.drop_with(vm);
            }
        }

        Ok(())
    }

    /// Merges kwargs into self.
    fn merge_from_kwargs(&mut self, kwargs: KwargsValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        let kwargs_iter = kwargs.into_iter();
        defer_drop_mut!(kwargs_iter, vm);
        for (key, value) in kwargs_iter {
            let old_value = self.set(key, value, vm)?;
            old_value.drop_with(vm);
        }
        Ok(())
    }
}

/// Iterator over borrowed (key, value) pairs in a dict.
pub(crate) struct DictEntriesIter<'a>(slice::Iter<'a, DictEntry>);

impl<'a> Iterator for DictEntriesIter<'a> {
    type Item = (&'a Value, &'a Value);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|e| (&e.key, &e.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }

    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, Self::Item) -> B,
    {
        self.0.fold(init, |acc, e| f(acc, (&e.key, &e.value)))
    }
}

impl<'a> IntoIterator for &'a Dict {
    type Item = (&'a Value, &'a Value);
    type IntoIter = DictEntriesIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        DictEntriesIter(self.entries.iter())
    }
}

/// Iterator over owned (key, value) pairs from a consumed dict.
pub(crate) struct DictIntoIter(vec::IntoIter<DictEntry>);

impl Iterator for DictIntoIter {
    type Item = (Value, Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|e| (e.key, e.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for DictIntoIter {}

impl IntoIterator for Dict {
    type Item = (Value, Value);
    type IntoIter = DictIntoIter;
    fn into_iter(self) -> Self::IntoIter {
        DictIntoIter(self.entries.into_iter())
    }
}

/// Stack-borrowed lending iterator over a heap-allocated [`Dict`]'s
/// `(key, value)` entries in insertion order.
///
/// Borrows a [`HeapRead`] for its lifetime, so the heap entry is pinned by
/// the reader count for the duration of iteration.
///
/// **Yield modes.** Pick the variant that matches the caller's natural
/// pattern to avoid redundant `clone_with_heap` / `drop_with` work:
///
/// - [`next`](Self::next) returns `Option<(&Value, &Value)>`. The iterator
///   owns the most-recently-yielded pair (using [`Value::Undefined`] as the
///   empty sentinel) and drops the previous pair at the start of each call,
///   so "use and discard" call sites do **not** need a per-item
///   `defer_drop!`.
/// - [`next_value`](Self::next_value) returns only a borrowed value, avoiding
///   key refcount churn for value-only operations.
/// - [`next_owned`](Self::next_owned) returns `Option<(Value, Value)>` and
///   clones straight into the return value, leaving the internal slot
///   `Undefined`. Prefer this when feeding pairs into a sink that takes
///   ownership (e.g. [`HeapRead::set`], `Set::add`) — going through `next`
///   forces a second `clone_with_heap` per element.
///
/// Mixing the yield modes is supported: every step drops whatever the slot
/// held before doing its work.
///
/// **Recursion guard.** Acquires a [`RecursionToken`] at construction and
/// releases it via [`DropWithContext`]. The iterator MUST be wrapped in
/// [`defer_drop_mut!`] so the token (and any in-flight pair) is released on
/// every exit path — dict iteration almost always calls back into
/// `py_eq` / `py_hash` (membership lookups, comparison) which recurse on
/// cyclic structures.
///
/// **Mutation policy.** The initial length is captured at construction. If
/// the dict's size changes between steps, the next step returns
/// `RuntimeError: dictionary changed size during iteration` (matching
/// CPython and Monty's dict-iterator behavior). Same-size updates (replacing
/// a value at an existing key) are allowed and observable.
pub(crate) struct DictIter<'a, 'h> {
    dict: &'a HeapRead<'h, Dict>,
    index: usize,
    expected_len: usize,
    token: RecursionToken,
    /// Most-recently-yielded pair. Both fields are `Value::Undefined` when
    /// nothing is held — drops on that variant are no-ops, so `next` can
    /// unconditionally release the previous slot before fetching the next.
    current_key: Value,
    current_value: Value,
}

impl<'a, 'h> DictIter<'a, 'h> {
    fn new<R: ResourceTracker>(dict: &'a HeapRead<'h, Dict>, vm: &mut VM<'h, R>) -> RunResult<Self> {
        let expected_len = dict.get(vm.heap).entries.len();
        let token = vm.recursion_token()?;
        Ok(Self {
            dict,
            index: 0,
            expected_len,
            token,
            current_key: Value::Undefined,
            current_value: Value::Undefined,
        })
    }

    /// Advances the iterator and returns borrows of the next `(key, value)`
    /// pair, or `Ok(None)` on exhaustion. The returned references are valid
    /// until the next call to `next` (or until the iterator is dropped).
    ///
    /// Returns `Err(RuntimeError)` if the dict's size has changed since
    /// construction.
    pub(crate) fn next<'i, R: ResourceTracker>(
        &'i mut self,
        vm: &mut VM<'h, R>,
    ) -> RunResult<Option<(&'i Value, &'i Value)>> {
        let Some(entry_index) = self.advance(vm)? else {
            return Ok(None);
        };
        let entry = &self.dict.get(vm.heap).entries[entry_index];
        self.current_key = entry.key.clone_with_heap(vm.heap);
        self.current_value = entry.value.clone_with_heap(vm.heap);
        Ok(Some((&self.current_key, &self.current_value)))
    }

    /// Advances the iterator and returns a borrow of the next value.
    ///
    /// Prefer this for value-only operations so dictionary keys are not cloned.
    /// The returned reference is valid until the next call that advances the
    /// iterator, or until the iterator is dropped.
    pub(crate) fn next_value<'i, R: ResourceTracker>(&'i mut self, vm: &mut VM<'h, R>) -> RunResult<Option<&'i Value>> {
        let Some(entry_index) = self.advance(vm)? else {
            return Ok(None);
        };
        let entry = &self.dict.get(vm.heap).entries[entry_index];
        self.current_value = entry.value.clone_with_heap(vm.heap);
        Ok(Some(&self.current_value))
    }

    /// Advances the iterator and returns the next `(key, value)` pair as
    /// owned values, transferring ownership to the caller.
    ///
    /// Prefer this over [`next`](Self::next) when the call site immediately
    /// needs owned values — e.g. to feed into a function that consumes a
    /// `Value` like `Set::add` or `Dict::set`. Going through `next` instead
    /// would clone the pair into the iterator's internal slot, then force the
    /// caller to re-`clone_with_heap` it, doubling the refcount churn.
    ///
    /// The iterator's internal slot is left `Undefined` after this call, so
    /// callers can freely mix `next` and `next_owned` on the same iterator.
    pub(crate) fn next_owned<R: ResourceTracker>(&mut self, vm: &mut VM<'h, R>) -> RunResult<Option<(Value, Value)>> {
        let Some(entry_index) = self.advance(vm)? else {
            return Ok(None);
        };
        let entry = &self.dict.get(vm.heap).entries[entry_index];
        let pair = (entry.key.clone_with_heap(vm.heap), entry.value.clone_with_heap(vm.heap));
        Ok(Some(pair))
    }

    /// Shared step for the iterator's borrowed and owned yield modes.
    ///
    /// Releases the previously-yielded slot (no-op when each slot is
    /// `Undefined`), runs the per-step time check and the dict mutation
    /// guard, then returns the entry index to read at — or `Ok(None)` when
    /// the iterator is exhausted. Bumps `self.index` on success.
    fn advance<R: ResourceTracker>(&mut self, vm: &mut VM<'h, R>) -> RunResult<Option<usize>> {
        mem::replace(&mut self.current_key, Value::Undefined).drop_with(vm.heap);
        mem::replace(&mut self.current_value, Value::Undefined).drop_with(vm.heap);
        vm.heap.check_time()?;
        let current = self.dict.get(vm.heap);
        if current.entries.len() != self.expected_len {
            return Err(ExcType::runtime_error_dict_changed_size());
        }
        if self.index >= self.expected_len {
            return Ok(None);
        }
        let entry_index = self.index;
        self.index += 1;
        Ok(Some(entry_index))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for DictIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current_key.drop_with(container);
        self.current_value.drop_with(container);
        self.token.drop_with(container);
    }
}

/// `PyTrait` implementation for `HeapRead<'h, Dict>`.
///
/// All methods access the dict data through short-lived borrows from the heap via
/// `self.get(vm.heap)`, and mutation methods use `self.get_mut(vm.heap)`. This avoids
/// taking the dict out of the heap, enabling self-referential operations like `d.update(d)`.
impl<'h> PyTrait<'h> for HeapRead<'h, Dict> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Dict
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::Dict(other)) => Ok(Some(self.eq_dict(&other, vm)?)),
            _ => Ok(None),
        }
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        if self.get(vm.heap).is_empty() {
            return Ok(f.write_str("{}")?);
        }

        // Check depth limit before recursing
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("{...}")?);
        };
        let vm = &mut *guard;

        f.write_char('{')?;
        let len = self.get(vm.heap).len();
        for i in 0..len {
            if i > 0 {
                if vm.heap.check_time().is_err() {
                    f.write_str(", ...[timeout]")?;
                    break;
                }
                f.write_str(", ")?;
            }
            let key = self
                .get(vm.heap)
                .key_at(i)
                .expect("index in range")
                .clone_with_heap(vm.heap);
            defer_drop!(key, vm);
            key.py_repr_fmt(f, vm, heap_ids)?;
            f.write_str(": ")?;
            let value = self
                .get(vm.heap)
                .value_at(i)
                .expect("index in range")
                .clone_with_heap(vm.heap);
            defer_drop!(value, vm);
            value.py_repr_fmt(f, vm, heap_ids)?;
        }
        f.write_char('}')?;

        Ok(())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        match self.dict_get(key, vm)? {
            Some(value) => Ok(value),
            None => Err(ExcType::key_error(key, vm)),
        }
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        // Drop the old value if one was replaced
        if let Some(old_value) = self.set(key, value, vm)? {
            old_value.drop_with(vm);
        }
        Ok(())
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Dict, attr.as_str(vm.interns)));
        };

        let value = match method {
            StaticStrings::Get => {
                // dict.get() accepts 1 or 2 arguments
                let (key, default) = args.get_one_two_args("get", vm.heap)?;
                defer_drop!(key, vm);
                let default = default.unwrap_or(Value::None);
                let mut default_guard = DropGuard::new(default, vm);
                let vm = default_guard.ctx();
                // Handle the lookup - may fail for unhashable keys
                match self.dict_get(key, vm)? {
                    Some(v) => Ok(v),
                    None => Ok(default_guard.into_inner()),
                }
            }
            StaticStrings::Keys => {
                args.check_zero_args("dict.keys", vm.heap)?;
                let view_id = vm.heap.allocate(HeapData::DictKeysView(DictKeysView::new(self_id)))?;
                vm.heap.inc_ref(self_id);
                Ok(Value::Ref(view_id))
            }
            StaticStrings::Values => {
                args.check_zero_args("dict.values", vm.heap)?;
                let view_id = vm
                    .heap
                    .allocate(HeapData::DictValuesView(DictValuesView::new(self_id)))?;
                vm.heap.inc_ref(self_id);
                Ok(Value::Ref(view_id))
            }
            StaticStrings::Items => {
                args.check_zero_args("dict.items", vm.heap)?;
                let view_id = vm.heap.allocate(HeapData::DictItemsView(DictItemsView::new(self_id)))?;
                vm.heap.inc_ref(self_id);
                Ok(Value::Ref(view_id))
            }
            StaticStrings::Pop => {
                // dict.pop() accepts 1 or 2 arguments (key, optional default)
                let (key, default) = args.get_one_two_args("pop", vm.heap)?;
                defer_drop!(key, vm);
                let mut default_guard = DropGuard::new(default, vm);
                let vm = default_guard.ctx();
                if let Some((old_key, value)) = self.pop(key, vm)? {
                    // Drop the old key - we don't need it
                    old_key.drop_with(vm);
                    Ok(value)
                } else {
                    let (default, vm) = default_guard.into_parts();
                    // No matching key - return default if provided, else KeyError
                    if let Some(d) = default {
                        Ok(d)
                    } else {
                        Err(ExcType::key_error(key, vm))
                    }
                }
            }
            StaticStrings::Clear => {
                args.check_zero_args("dict.clear", vm.heap)?;
                dict_clear(self, vm);
                Ok(Value::None)
            }
            StaticStrings::Copy => {
                args.check_zero_args("dict.copy", vm.heap)?;
                dict_copy(self, vm)
            }
            StaticStrings::Update => dict_update(self, args, vm),
            StaticStrings::Setdefault => dict_setdefault(self, args, vm),
            StaticStrings::Popitem => {
                args.check_zero_args("dict.popitem", vm.heap)?;
                dict_popitem(self, vm)
            }
            // fromkeys is a classmethod but also accessible on instances
            StaticStrings::Fromkeys => dict_fromkeys(args, vm),
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::Dict, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }
}

impl HeapItem for Dict {
    fn py_estimate_size(&self) -> usize {
        // Dict size: struct overhead + entries (2 Values per entry for key+value)
        mem::size_of::<Self>() + self.len() * 2 * VALUE_SIZE
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - major GC optimization for dicts of primitives
        if !self.contains_refs {
            return;
        }
        for entry in &mut self.entries {
            if let Value::Ref(id) = &entry.key {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                entry.key.dec_ref_forget();
            }
            if let Value::Ref(id) = &entry.value {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                entry.value.dec_ref_forget();
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Dict {
    fn drop_with(self, heap: &mut C) {
        self.entries.drop_with(heap);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for DictEntry {
    fn drop_with(self, heap: &mut C) {
        self.key.drop_with(heap);
        self.value.drop_with(heap);
    }
}

/// Implements Python's `dict.clear()` method.
///
/// Removes all items from the dict.
fn dict_clear<'h>(dict: &mut HeapRead<'h, Dict>, vm: &mut VM<'h, impl ResourceTracker>) {
    dict.get_mut(vm.heap).indices.clear();
    mem::take(&mut dict.get_mut(vm.heap).entries).drop_with(vm.heap);
    // Note: contains_refs stays true even if all refs removed, per conservative GC strategy
}

/// Implements Python's `dict.copy()` method.
///
/// Returns a shallow copy of the dict.
fn dict_copy<'h>(dict: &mut HeapRead<'h, Dict>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    // Copy all key-value pairs (incrementing refcounts)
    let pairs: Vec<(Value, Value)> = dict
        .get(vm.heap)
        .iter()
        .map(|(k, v)| (k.clone_with_heap(vm), v.clone_with_heap(vm)))
        .collect();

    let new_dict = Dict::from_pairs(pairs, vm)?;
    let heap_id = vm.heap.allocate(HeapData::Dict(new_dict))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `dict.update([other], **kwargs)` method.
///
/// Updates the dict with key-value pairs from `other` and/or `kwargs`.
/// If `other` is a dict, copies its key-value pairs.
/// If `other` is an iterable, expects pairs of (key, value).
/// Keyword arguments are also added to the dict.
fn dict_update<'h>(
    dict: &mut HeapRead<'h, Dict>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let DictUpdateArgs { source, extras } = DictUpdateArgs::from_args(args, vm)?;
    let mut kwargs_guard = DropGuard::new(extras, vm);

    if let Some(other_value) = source {
        let other_value_guard = DropGuard::new(other_value, kwargs_guard.ctx());
        let other_value = other_value_guard.into_inner();
        dict.merge_from_value(other_value, kwargs_guard.ctx())?;
    }

    let kwargs = kwargs_guard.into_inner();
    dict.merge_from_kwargs(kwargs, vm)?;
    Ok(Value::None)
}

/// Argument shape for `dict.update([other], **kwargs)`.
///
/// Mirrors [`DictInitArgs`] — an optional positional source plus arbitrary
/// kwargs that are merged into the dict after the source.
#[derive(FromArgs)]
#[from_args(name = "update")]
struct DictUpdateArgs {
    #[from_args(pos_only, default)]
    source: Option<Value>,
    #[from_args(varkwargs)]
    extras: KwargsValues,
}

/// Merges key-value pairs from either a dict or an iterable of 2-item pairs.
///
/// This is shared between `dict()` construction and `dict.update()` so both
/// entry points follow identical positional-source semantics.
fn dict_merge_from_value(dict: &mut Dict, other_value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<()> {
    let mut other_value_guard = DropGuard::new(other_value, vm);
    {
        let (other_value, vm) = other_value_guard.as_parts();
        if let Value::Ref(id) = other_value
            && let HeapData::Dict(src_dict) = vm.heap.get(*id)
        {
            // Clone key-value pairs from the source dict.
            let pairs: Vec<(Value, Value)> = src_dict
                .iter()
                .map(|(k, v)| (k.clone_with_heap(vm), v.clone_with_heap(vm)))
                .collect();

            // Apply pairs into the target dict.
            for (key, value) in pairs {
                let old_value = dict.set(key, value, vm)?;
                old_value.drop_with(vm);
            }
            return Ok(());
        }
    }

    // Non-dict values are interpreted as iterable-of-pairs.
    let other_value = other_value_guard.into_inner();
    dict_merge_from_iterable_pairs(dict, other_value, vm)
}

/// Merges key-value pairs from an iterable of 2-item iterables.
///
/// Each item from `iterable` is treated as `(key, value)`. Items with length 0, 1,
/// or greater than 2 raise the same TypeError messages used by `dict.update()`.
fn dict_merge_from_iterable_pairs(
    dict: &mut Dict,
    iterable: Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    let iter = iterable.into_py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    while let Some(item) = iter.py_next(vm)? {
        // Each item should be a pair (iterable of 2 elements).
        let pair_iter = item.into_py_iter(vm)?;
        defer_drop!(pair_iter, vm);
        let mut pair_iter = pair_iter.read(vm);

        let Some(key) = pair_iter.py_next(vm)? else {
            return Err(ExcType::type_error(
                "dictionary update sequence element has length 0; 2 is required",
            ));
        };
        let mut key_guard = DropGuard::new(key, vm);

        let Some(value) = pair_iter.py_next(key_guard.ctx())? else {
            return Err(ExcType::type_error(
                "dictionary update sequence element has length 1; 2 is required",
            ));
        };
        let mut value_guard = DropGuard::new(value, key_guard.ctx());

        if let Some(extra) = pair_iter.py_next(value_guard.ctx())? {
            extra.drop_with(value_guard.ctx());
            return Err(ExcType::type_error(
                "dictionary update sequence element has length > 2; 2 is required",
            ));
        }

        let value = value_guard.into_inner();
        let key = key_guard.into_inner();

        if let Some(old_value) = dict.set(key, value, vm)? {
            old_value.drop_with(vm);
        }
    }

    Ok(())
}

/// Merges keyword arguments into a dict.
///
/// This helper drains `kwargs` safely on error so all values are dropped
/// correctly, then inserts each key-value pair into `dict`.
fn dict_merge_from_kwargs(
    dict: &mut Dict,
    kwargs: KwargsValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);
    for (key, value) in kwargs_iter {
        let old_value = dict.set(key, value, vm)?;
        old_value.drop_with(vm);
    }
    Ok(())
}

/// Implements Python's `dict.setdefault(key[, default])` method.
///
/// If key is in the dict, return its value.
/// If not, insert key with a value of default (or None) and return default.
fn dict_setdefault<'h>(
    dict: &mut HeapRead<'h, Dict>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (key, default) = args.get_one_two_args("setdefault", vm.heap)?;
    let default = default.unwrap_or(Value::None);
    let mut key_guard = DropGuard::new(key, vm);
    let (key, vm) = key_guard.as_parts();

    if let Some(existing) = dict.dict_get(key, vm)? {
        default.drop_with(vm);
        Ok(existing)
    } else {
        // Key doesn't exist - insert default and return it (cloned before insertion)
        let return_value = default.clone_with_heap(vm);
        let (key, vm) = key_guard.into_parts();
        if let Some(old_value) = dict.set(key, default, vm)? {
            // This shouldn't happen since we checked, but handle it anyway
            old_value.drop_with(vm);
        }
        Ok(return_value)
    }
}

/// Implements Python's `dict.popitem()` method.
///
/// Removes and returns the last inserted key-value pair as a tuple.
/// Raises KeyError if the dict is empty.
fn dict_popitem<'h>(dict: &mut HeapRead<'h, Dict>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let this = dict.get_mut(vm.heap);
    if this.is_empty() {
        return Err(ExcType::key_error_popitem_empty_dict());
    }

    // Remove the last entry (LIFO order)
    let entry = this.entries.pop().expect("dict is not empty");

    // Remove from indices - need to find the entry with this index
    // Since we removed the last entry, we need to clear and rebuild indices
    // (This is simpler than trying to find and remove the specific hash entry)
    // TODO: This O(n) rebuild could be optimized by finding and removing the
    // specific hash entry directly from the hashbrown table.
    this.indices.clear();
    for (idx, e) in this.entries.iter().enumerate() {
        this.indices.insert_unique(e.hash, idx, |&i| this.entries[i].hash);
    }

    // Create tuple (key, value)
    Ok(allocate_tuple(smallvec![entry.key, entry.value], vm.heap)?)
}

// Custom serde implementation for Dict.
// Serializes entries and contains_refs; rebuilds the indices hash table on deserialize.
impl serde::Serialize for Dict {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Dict", 2)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("contains_refs", &self.contains_refs)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dict {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct DictFields {
            entries: Vec<DictEntry>,
            contains_refs: bool,
        }
        let fields = DictFields::deserialize(deserializer)?;
        // Rebuild the indices hash table from the entries
        let mut indices = HashTable::with_capacity(fields.entries.len());
        for (idx, entry) in fields.entries.iter().enumerate() {
            indices.insert_unique(entry.hash, idx, |&i| fields.entries[i].hash);
        }
        Ok(Self {
            indices,
            entries: fields.entries,
            contains_refs: fields.contains_refs,
        })
    }
}

/// Implements Python's `dict.fromkeys(iterable[, value])` classmethod.
///
/// Creates a new dictionary with keys from `iterable` and all values set to `value`
/// (default: None).
///
/// This is a classmethod that can be called directly on the dict type:
/// ```python
/// dict.fromkeys(['a', 'b', 'c'])  # {'a': None, 'b': None, 'c': None}
/// dict.fromkeys(['a', 'b'], 0)    # {'a': 0, 'b': 0}
/// ```
pub fn dict_fromkeys(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let (iterable, default) = args.get_one_two_args("dict.fromkeys", vm.heap)?;
    let default = default.unwrap_or(Value::None);
    defer_drop!(default, vm);

    let iter = iterable.into_py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let dict = Dict::new();
    let mut dict_guard = DropGuard::new(dict, vm);

    {
        let (dict, vm) = dict_guard.as_parts_mut();

        while let Some(key) = iter.py_next(vm)? {
            let old_value = dict.set(key, default.clone_with_heap(vm), vm)?;
            old_value.drop_with(vm);
        }
    }

    let dict = dict_guard.into_inner();
    let heap_id = vm.heap.allocate(HeapData::Dict(dict))?;
    Ok(Value::Ref(heap_id))
}
