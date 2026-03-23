use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use ahash::AHashSet;
use hashbrown::HashTable;
use smallvec::{SmallVec, smallvec};

use super::{DictItemsView, DictKeysView, DictValuesView, MontyIter, PyTrait, allocate_tuple};
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapGuard, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::Type,
    value::{EitherStr, Value},
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
    pub fn from_pairs(pairs: Vec<(Value, Value)>, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let pairs_iter = pairs.into_iter();
        defer_drop_mut!(pairs_iter, vm);
        let dict = Self::with_capacity(pairs_iter.len());
        let mut dict_guard = HeapGuard::new(dict, vm);
        let (dict, vm) = dict_guard.as_parts_mut();
        for (key, value) in pairs_iter {
            if let Some(old_value) = dict.set(key, value, vm)? {
                old_value.drop_with_heap(vm);
            }
        }
        Ok(dict_guard.into_inner())
    }

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
    pub fn set(
        &mut self,
        key: Value,
        value: Value,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        // Track if we're adding a reference for GC optimization
        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.contains_refs = true;
        }

        // Handle hash computation errors explicitly so we can drop key/value properly
        let (opt_index, hash) = match self.find_index_hash(&key, vm) {
            Ok(result) => result,
            Err(e) => {
                // Drop the key and value before returning the error
                key.drop_with_heap(vm);
                value.drop_with_heap(vm);
                return Err(e);
            }
        };

        let entry = DictEntry { key, value, hash };
        if let Some(index) = opt_index {
            // Key exists, replace in place to preserve insertion order
            let old_entry = std::mem::replace(&mut self.entries[index], entry);

            // Decrement refcount for old key (we're discarding it)
            old_entry.key.drop_with_heap(vm);
            // Transfer ownership of the old value to caller (no clone needed)
            Ok(Some(old_entry.value))
        } else {
            // Key doesn't exist, add new pair to indices and entries
            let index = self.entries.len();
            self.entries.push(entry);
            self.indices
                .insert_unique(hash, index, |index| self.entries[*index].hash);
            Ok(None)
        }
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
    pub fn set(
        &mut self,
        key: Value,
        value: Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        // Track if we're adding a reference for GC optimization
        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.get_mut(vm.heap).contains_refs = true;
        }

        // Handle hash computation errors explicitly so we can drop key/value properly
        let (opt_index, hash) = match self.find_index_hash(&key, vm) {
            Ok(result) => result,
            Err(e) => {
                // Drop the key and value before returning the error
                key.drop_with_heap(vm);
                value.drop_with_heap(vm);
                return Err(e);
            }
        };

        let entry = DictEntry { key, value, hash };
        if let Some(index) = opt_index {
            // Key exists, replace in place to preserve insertion order
            let old_entry = std::mem::replace(&mut self.get_mut(vm.heap).entries[index], entry);

            // Decrement refcount for old key (we're discarding it)
            old_entry.key.drop_with_heap(vm);
            // Transfer ownership of the old value to caller (no clone needed)
            Ok(Some(old_entry.value))
        } else {
            // Key doesn't exist, add new pair to indices and entries
            let this = self.get_mut(vm.heap);
            let index = this.entries.len();
            this.entries.push(entry);
            this.indices
                .insert_unique(hash, index, |index| this.entries[*index].hash);
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
    pub fn iter(&self) -> DictIter<'_> {
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
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let dict = Self::new();
        let mut dict_guard = HeapGuard::new(dict, vm);

        {
            let (dict, vm) = dict_guard.as_parts_mut();
            let (pos_iter, kwargs) = args.into_parts();
            defer_drop_mut!(pos_iter, vm);
            let mut kwargs_guard = HeapGuard::new(kwargs, vm);

            if let Some(other_value) = pos_iter.next() {
                let other_value_guard = HeapGuard::new(other_value, kwargs_guard.heap());
                if pos_iter.len() != 0 {
                    return Err(ExcType::type_error_at_most("dict", 1, pos_iter.len() + 1));
                }
                let other_value = other_value_guard.into_inner();
                dict_merge_from_value(dict, other_value, kwargs_guard.heap())?;
            }

            let kwargs = kwargs_guard.into_inner();
            dict_merge_from_kwargs(dict, kwargs, vm)?;
        }

        let dict = dict_guard.into_inner();
        let heap_id = vm.heap.allocate(HeapData::Dict(dict))?;
        Ok(Value::Ref(heap_id))
    }

    fn find_index_hash(
        &self,
        key: &Value,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<(Option<usize>, u64)> {
        let hash = key
            .py_hash(vm.heap, vm.interns)?
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(vm)))?;

        // Dict keys are typically shallow (strings, ints, tuples of primitives),
        // so recursion errors are unlikely. If one occurs, treat it as "not equal" -
        // the key lookup fails but doesn't crash.
        let opt_index = self
            .indices
            .find(hash, |v| key.py_eq(&self.entries[*v].key, vm).unwrap_or(false))
            .copied();
        Ok((opt_index, hash))
    }

    /// Writes the Python `repr()` for this dict value.
    ///
    /// This is an inherent method so `HeapData` can call it on a bare `Dict`
    /// without requiring a `HeapRead` wrapper. Iterates entries in insertion order,
    /// producing `{key: value, ...}` output with recursion-depth and timeout checks.
    pub fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("{}");
        }

        let heap = &*vm.heap;
        // Check depth limit before recursing
        let Some(token) = heap.incr_recursion_depth_for_repr() else {
            return f.write_str("{...}");
        };
        crate::defer_drop_immutable_heap!(token, heap);

        f.write_char('{')?;
        let mut first = true;
        for entry in &self.entries {
            if !first {
                if heap.check_time().is_err() {
                    f.write_str(", ...[timeout]")?;
                    break;
                }
                f.write_str(", ")?;
            }
            first = false;
            entry.key.py_repr_fmt(f, vm, heap_ids)?;
            f.write_str(": ")?;
            entry.value.py_repr_fmt(f, vm, heap_ids)?;
        }
        f.write_char('}')?;

        Ok(())
    }
}

impl<'h> HeapRead<'h, Dict> {
    /// Checks whether the dict contains a given key.
    ///
    /// Uses `find_index_hash` internally so it handles the short-lived borrow
    /// pattern correctly — the dict is only accessed through temporary borrows,
    /// allowing `py_eq` calls on keys in between.
    pub(crate) fn contains_key(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<bool> {
        let (opt_index, _hash) = self.find_index_hash(key, vm)?;
        Ok(opt_index.is_some())
    }

    /// Looks up a key and returns a clone of the associated value.
    ///
    /// Returns `Ok(Some(value))` if the key exists (value is cloned with refcount
    /// increment), `Ok(None)` if the key is not found, or `Err` if the key is
    /// unhashable. The caller owns the returned value.
    pub(crate) fn get_cloned(
        &self,
        key: &Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        let (opt_index, _hash) = self.find_index_hash(key, vm)?;
        if let Some(index) = opt_index {
            // Clone the value using the two-phase pattern:
            // 1. Read the value discriminant (shared borrow)
            // 2. Increment refcount if Ref (mutable borrow)
            let ref_id = match &self.get(vm.heap).entries[index].value {
                Value::Ref(id) => Some(*id),
                _ => None,
            };
            if let Some(id) = ref_id {
                vm.heap.inc_ref(id);
                Ok(Some(Value::Ref(id)))
            } else {
                Ok(Some(self.get(vm.heap).entries[index].value.clone_immediate()))
            }
        } else {
            Ok(None)
        }
    }

    fn find_index_hash(
        &self,
        key: &Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<(Option<usize>, u64)> {
        let hash = key
            .py_hash(vm.heap, vm.interns)?
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(vm)))?;

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

    /// Two-phase clone of the key at a given entry index.
    fn clone_key_at(&self, index: usize, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Value {
        let ref_id = match self.get(vm.heap).key_at(index) {
            Some(Value::Ref(id)) => Some(*id),
            _ => None,
        };
        if let Some(id) = ref_id {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        } else {
            self.get(vm.heap).key_at(index).expect("index valid").clone_immediate()
        }
    }

    /// Two-phase clone of the value at a given entry index.
    fn clone_value_at(&self, index: usize, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Value {
        let ref_id = match self.get(vm.heap).value_at(index) {
            Some(Value::Ref(id)) => Some(*id),
            _ => None,
        };
        if let Some(id) = ref_id {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        } else {
            self.get(vm.heap)
                .value_at(index)
                .expect("index valid")
                .clone_immediate()
        }
    }

    /// Element-wise equality comparison using the short-lived borrow pattern.
    ///
    /// For each entry in self, looks up the key in other and compares values.
    /// Both keys and values are cloned temporarily for comparison.
    pub(crate) fn eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.get(vm.heap).len() != other.get(vm.heap).len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        let len = self.get(vm.heap).len();
        for i in 0..len {
            vm.heap.check_time()?;
            // Clone key from self to use as lookup key in other
            let key = self.get(vm.heap).key_at(i).expect("index valid").clone_with_heap(vm);
            defer_drop!(key, vm);
            // Swallow RunErrors from get_cloned (e.g. unhashable key) and treat as not-equal,
            // matching the pattern used in the original Dict::py_eq.
            if let Ok(Some(other_value)) = other.get_cloned(key, vm) {
                let self_value = self.clone_value_at(i, vm);
                let eq = self_value.py_eq(&other_value, vm);
                self_value.drop_with_heap(vm);
                other_value.drop_with_heap(vm);
                if !eq? {
                    return Ok(false);
                }
            } else {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Subscript access via HeapRead, returning a cloned value or KeyError.
    pub(crate) fn getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        match self.get_cloned(key, vm)? {
            Some(value) => Ok(value),
            None => Err(ExcType::key_error(key, vm)),
        }
    }

    /// Subscript assignment via HeapRead. Drops old value if key already exists.
    pub(crate) fn setitem(
        &mut self,
        key: Value,
        value: Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<()> {
        if let Some(old_value) = self.set(key, value, vm)? {
            old_value.drop_with_heap(vm);
        }
        Ok(())
    }
}

impl<'h> HeapRead<'h, Dict> {
    /// `dict.pop(key[, default])` via HeapRead.
    fn hr_pop(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let (key, default) = args.get_one_two_args("pop", vm.heap)?;
        defer_drop!(key, vm);
        let mut default_guard = HeapGuard::new(default, vm);

        // Find the key using the candidate-based lookup
        let (opt_index, _hash) = self.find_index_hash(key, default_guard.heap())?;

        if let Some(index) = opt_index {
            let vm = default_guard.heap();
            // Remove the entry
            let entry = self.get_mut(vm.heap).entries.remove(index);
            // Remove from index table and rebuild (same as dict_popitem)
            let this = self.get_mut(vm.heap);
            this.indices.clear();
            for (idx, e) in this.entries.iter().enumerate() {
                this.indices.insert_unique(e.hash, idx, |&i| this.entries[i].hash);
            }
            // Drop the old key, return the value
            entry.key.drop_with_heap(vm);
            Ok(entry.value)
        } else {
            let (default, vm) = default_guard.into_parts();
            if let Some(d) = default {
                Ok(d)
            } else {
                Err(ExcType::key_error(key, vm))
            }
        }
    }

    /// `dict.clear()` via HeapRead.
    fn hr_clear(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) {
        let entries: Vec<DictEntry> = self.get_mut(vm.heap).entries.drain(..).collect();
        self.get_mut(vm.heap).indices.clear();
        entries.drop_with_heap(vm);
    }

    /// `dict.copy()` via HeapRead — returns a shallow copy.
    fn hr_copy(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let len = self.get(vm.heap).entries.len();
        let mut pairs = Vec::with_capacity(len);
        for i in 0..len {
            let key = self.clone_key_at(i, vm);
            let value = self.clone_value_at(i, vm);
            pairs.push((key, value));
        }
        let new_dict = Dict::from_pairs(pairs, vm)?;
        let heap_id = vm.heap.allocate(HeapData::Dict(new_dict))?;
        Ok(Value::Ref(heap_id))
    }

    /// `dict.update([other], **kwargs)` via HeapRead.
    ///
    /// Because data stays in the heap, `d.update(d)` works correctly — the source
    /// dict is accessible through the heap while we iterate and copy its pairs.
    fn hr_update(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let (pos_iter, kwargs) = args.into_parts();
        defer_drop_mut!(pos_iter, vm);
        let mut kwargs_guard = HeapGuard::new(kwargs, vm);

        if let Some(other_value) = pos_iter.next() {
            let other_value_guard = HeapGuard::new(other_value, kwargs_guard.heap());
            if pos_iter.len() != 0 {
                return Err(ExcType::type_error_at_most("dict.update", 1, pos_iter.len() + 1));
            }
            let other_value = other_value_guard.into_inner();
            self.hr_merge_from_value(other_value, kwargs_guard.heap())?;
        }

        let kwargs = kwargs_guard.into_inner();
        self.hr_merge_from_kwargs(kwargs, vm)?;
        Ok(Value::None)
    }

    /// Merges key-value pairs from a dict or iterable-of-pairs into self via HeapRead.
    ///
    /// For dict sources, uses HeapReader::read() to access the source dict through
    /// the heap, enabling self-referential updates like `d.update(d)`.
    fn hr_merge_from_value(&mut self, other_value: Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<()> {
        let mut guard = HeapGuard::new(other_value, vm);
        let (other_value, vm) = guard.as_parts_mut();
        if let Value::Ref(id) = other_value {
            let src_id = *id;
            if let HeapReadOutput::Dict(src) = vm.heap.read(src_id) {
                let len = src.get(vm.heap).entries.len();
                for i in 0..len {
                    let entry = &src.get(vm.heap).entries[i];
                    let key = entry.key.clone_with_heap(vm);
                    let value = entry.value.clone_with_heap(vm);
                    let old_value = self.set(key, value, vm)?;
                    old_value.drop_with_heap(vm);
                }

                // guard drops other_value here
                return Ok(());
            }
        }

        // Non-dict values are interpreted as iterable-of-pairs
        let (other_value, vm) = guard.into_parts();
        self.hr_merge_from_iterable_pairs(other_value, vm)
    }

    /// Merges key-value pairs from an iterable of 2-item pairs.
    fn hr_merge_from_iterable_pairs(
        &mut self,
        iterable: Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<()> {
        let iter = MontyIter::new(iterable, vm)?;
        defer_drop_mut!(iter, vm);

        while let Some(item) = iter.for_next(vm)? {
            let pair_iter = MontyIter::new(item, vm)?;
            defer_drop_mut!(pair_iter, vm);

            let Some(key) = pair_iter.for_next(vm)? else {
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 0; 2 is required",
                ));
            };
            let mut key_guard = HeapGuard::new(key, vm);

            let Some(value) = pair_iter.for_next(key_guard.heap())? else {
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 1; 2 is required",
                ));
            };
            let mut value_guard = HeapGuard::new(value, key_guard.heap());

            if let Some(extra) = pair_iter.for_next(value_guard.heap())? {
                extra.drop_with_heap(value_guard.heap());
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length > 2; 2 is required",
                ));
            }

            let value = value_guard.into_inner();
            let key = key_guard.into_inner();

            if let Some(old_value) = self.set(key, value, vm)? {
                old_value.drop_with_heap(vm);
            }
        }

        Ok(())
    }

    /// Merges kwargs into self.
    fn hr_merge_from_kwargs(
        &mut self,
        kwargs: KwargsValues,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<()> {
        let kwargs_iter = kwargs.into_iter();
        defer_drop_mut!(kwargs_iter, vm);
        for (key, value) in kwargs_iter {
            let old_value = self.set(key, value, vm)?;
            old_value.drop_with_heap(vm);
        }
        Ok(())
    }

    /// `dict.setdefault(key[, default])` via HeapRead.
    fn hr_setdefault(&mut self, args: ArgValues, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let (key, default) = args.get_one_two_args("setdefault", vm.heap)?;
        let default = default.unwrap_or(Value::None);
        defer_drop!(key, vm);

        // Check if key exists
        if let Some(existing) = self.get_cloned(key, vm)? {
            default.drop_with_heap(vm);
            Ok(existing)
        } else {
            // Key doesn't exist - insert default and return a clone
            let return_value = default.clone_with_heap(vm);
            let key_clone = key.clone_with_heap(vm);
            if let Some(old_value) = self.set(key_clone, default, vm)? {
                old_value.drop_with_heap(vm);
            }
            Ok(return_value)
        }
    }

    /// `dict.popitem()` via HeapRead.
    fn hr_popitem(&mut self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        if self.get(vm.heap).is_empty() {
            return Err(ExcType::key_error_popitem_empty_dict());
        }

        let entry = self.get_mut(vm.heap).entries.pop().expect("dict is not empty");

        // Rebuild indices
        let this = self.get_mut(vm.heap);
        this.indices.clear();
        for (idx, e) in this.entries.iter().enumerate() {
            this.indices.insert_unique(e.hash, idx, |&i| this.entries[i].hash);
        }

        Ok(allocate_tuple(smallvec![entry.key, entry.value], vm.heap)?)
    }
}

/// Iterator over borrowed (key, value) pairs in a dict.
pub(crate) struct DictIter<'a>(std::slice::Iter<'a, DictEntry>);

impl<'a> Iterator for DictIter<'a> {
    type Item = (&'a Value, &'a Value);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|e| (&e.key, &e.value))
    }
}

impl<'a> IntoIterator for &'a Dict {
    type Item = (&'a Value, &'a Value);
    type IntoIter = DictIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        DictIter(self.entries.iter())
    }
}

/// Iterator over owned (key, value) pairs from a consumed dict.
pub(crate) struct DictIntoIter(std::vec::IntoIter<DictEntry>);

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

/// `PyTrait` implementation for `HeapRead<'h, Dict>`.
///
/// All methods access the dict data through short-lived borrows from the heap via
/// `self.get(vm.heap)`, and mutation methods use `self.get_mut(vm.heap)`. This avoids
/// taking the dict out of the heap, enabling self-referential operations like `d.update(d)`.
impl<'h> PyTrait<'h> for HeapRead<'h, Dict> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::Dict
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        self.eq(other, vm)
    }

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        self.get(vm.heap).py_repr_fmt(f, vm, heap_ids)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        self.getitem(key, vm)
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<()> {
        self.setitem(key, value, vm)
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(Type::Dict, attr.as_str(vm.interns)));
        };

        let value = match method {
            StaticStrings::Get => {
                let (key, default) = args.get_one_two_args("get", vm.heap)?;
                defer_drop!(key, vm);
                let default = default.unwrap_or(Value::None);
                let mut default_guard = HeapGuard::new(default, vm);
                let vm = default_guard.heap();
                match self.get_cloned(key, vm)? {
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
            StaticStrings::Pop => self.hr_pop(args, vm),
            StaticStrings::Clear => {
                args.check_zero_args("dict.clear", vm.heap)?;
                self.hr_clear(vm);
                Ok(Value::None)
            }
            StaticStrings::Copy => {
                args.check_zero_args("dict.copy", vm.heap)?;
                self.hr_copy(vm)
            }
            StaticStrings::Update => self.hr_update(args, vm),
            StaticStrings::Setdefault => self.hr_setdefault(args, vm),
            StaticStrings::Popitem => {
                args.check_zero_args("dict.popitem", vm.heap)?;
                self.hr_popitem(vm)
            }
            StaticStrings::Fromkeys => dict_fromkeys(args, vm),
            _ => {
                args.drop_with_heap(vm);
                return Err(ExcType::attribute_error(Type::Dict, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }
}

impl HeapItem for Dict {
    fn py_estimate_size(&self) -> usize {
        // Dict size: struct overhead + entries (2 Values per entry for key+value)
        std::mem::size_of::<Self>() + self.len() * 2 * std::mem::size_of::<Value>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - major GC optimization for dicts of primitives
        if !self.contains_refs {
            return;
        }
        for entry in &mut self.entries {
            if let Value::Ref(id) = &entry.key {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                entry.key.dec_ref_forget();
            }
            if let Value::Ref(id) = &entry.value {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                entry.value.dec_ref_forget();
            }
        }
    }
}

impl DropWithHeap for Dict {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.entries.drop_with_heap(heap);
    }
}

impl DropWithHeap for DictEntry {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.key.drop_with_heap(heap);
        self.value.drop_with_heap(heap);
    }
}

/// Merges key-value pairs from either a dict or an iterable of 2-item pairs.
///
/// This is shared between `dict()` construction and `dict.update()` so both
/// entry points follow identical positional-source semantics.
fn dict_merge_from_value(
    dict: &mut Dict,
    other_value: Value,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    let mut other_value_guard = HeapGuard::new(other_value, vm);
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
                old_value.drop_with_heap(vm);
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
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);

    while let Some(item) = iter.for_next(vm)? {
        // Each item should be a pair (iterable of 2 elements).
        let pair_iter = MontyIter::new(item, vm)?;
        defer_drop_mut!(pair_iter, vm);

        let Some(key) = pair_iter.for_next(vm)? else {
            return Err(ExcType::type_error(
                "dictionary update sequence element has length 0; 2 is required",
            ));
        };
        let mut key_guard = HeapGuard::new(key, vm);

        let Some(value) = pair_iter.for_next(key_guard.heap())? else {
            return Err(ExcType::type_error(
                "dictionary update sequence element has length 1; 2 is required",
            ));
        };
        let mut value_guard = HeapGuard::new(value, key_guard.heap());

        if let Some(extra) = pair_iter.for_next(value_guard.heap())? {
            extra.drop_with_heap(value_guard.heap());
            return Err(ExcType::type_error(
                "dictionary update sequence element has length > 2; 2 is required",
            ));
        }

        let value = value_guard.into_inner();
        let key = key_guard.into_inner();

        if let Some(old_value) = dict.set(key, value, vm)? {
            old_value.drop_with_heap(vm);
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
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);
    for (key, value) in kwargs_iter {
        let old_value = dict.set(key, value, vm)?;
        old_value.drop_with_heap(vm);
    }
    Ok(())
}

// Custom serde implementation for Dict.
// Serializes entries and contains_refs; rebuilds the indices hash table on deserialize.
impl serde::Serialize for Dict {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
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
pub fn dict_fromkeys(args: ArgValues, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let (iterable, default) = args.get_one_two_args("dict.fromkeys", vm.heap)?;
    let default = default.unwrap_or(Value::None);
    defer_drop!(default, vm);

    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);

    let dict = Dict::new();
    let mut dict_guard = HeapGuard::new(dict, vm);

    {
        let (dict, vm) = dict_guard.as_parts_mut();

        while let Some(key) = iter.for_next(vm)? {
            let old_value = dict.set(key, default.clone_with_heap(vm), vm)?;
            old_value.drop_with_heap(vm);
        }
    }

    let dict = dict_guard.into_inner();
    let heap_id = vm.heap.allocate(HeapData::Dict(dict))?;
    Ok(Value::Ref(heap_id))
}
