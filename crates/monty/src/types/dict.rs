use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use ahash::AHashSet;
use hashbrown::{HashTable, hash_table::Entry};
use smallvec::smallvec;

use super::{List, MontyIter, PyTrait, allocate_tuple};
use crate::{
    args::{ArgValues, KwargsValues},
    defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{DepthGuard, ResourceError, ResourceTracker},
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
    pub fn from_pairs(
        pairs: Vec<(Value, Value)>,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        let mut dict = Self::with_capacity(pairs.len());
        let mut pairs_iter = pairs.into_iter();
        for (key, value) in pairs_iter.by_ref() {
            if let Err(err) = dict.set_transfer_ownership(key, value, heap, interns) {
                for (k, v) in pairs_iter {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
                dict.drop_all_entries(heap);
                return Err(err);
            }
        }
        Ok(dict)
    }

    /// Internal method to set a key-value pair without incrementing refcounts.
    ///
    /// Used when ownership is being transferred (e.g., from_pairs) rather than shared.
    /// The caller must ensure the values' refcounts already account for this dict's reference.
    fn set_transfer_ownership(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        // Track if we're adding a reference for GC optimization
        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.contains_refs = true;
        }

        let (opt_index, hash) = match self.find_index_hash(&key, heap, interns) {
            Ok((index, hash)) => (index, hash),
            Err(err) => {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(err);
            }
        };

        // Check if key already exists in bucket
        if let Some(index) = opt_index {
            // Key exists, replace in place to preserve insertion order.
            // The new duplicate key must be dropped since we keep the existing key.
            // The old value must also be dropped since we're replacing it.
            let existing_bucket = &mut self.entries[index];
            let old_value = std::mem::replace(&mut existing_bucket.value, value);
            old_value.drop_with_heap(heap);
            key.drop_with_heap(heap);
        } else {
            // Key doesn't exist, add new pair to indices and entries
            let index = self.entries.len();
            self.entries.push(DictEntry { key, value, hash });
            self.indices
                .insert_unique(hash, index, |index| self.entries[*index].hash);
        }
        Ok(())
    }

    fn drop_all_entries(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        for entry in self.entries.drain(..) {
            entry.key.drop_with_heap(heap);
            entry.value.drop_with_heap(heap);
        }
        self.indices.clear();
    }

    /// Gets a value from the dict by key.
    ///
    /// Returns Ok(Some(value)) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    pub fn get(
        &self,
        key: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<&Value>> {
        if let Some(index) = self.find_index_hash(key, heap, interns)?.0 {
            Ok(Some(&self.entries[index].value))
        } else {
            Ok(None)
        }
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
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        // Track if we're adding a reference for GC optimization
        if matches!(key, Value::Ref(_)) || matches!(value, Value::Ref(_)) {
            self.contains_refs = true;
        }

        // Handle hash computation errors explicitly so we can drop key/value properly
        let (opt_index, hash) = match self.find_index_hash(&key, heap, interns) {
            Ok(result) => result,
            Err(e) => {
                // Drop the key and value before returning the error
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(e);
            }
        };

        let entry = DictEntry { key, value, hash };
        if let Some(index) = opt_index {
            // Key exists, replace in place to preserve insertion order
            let old_entry = std::mem::replace(&mut self.entries[index], entry);

            // Decrement refcount for old key (we're discarding it)
            old_entry.key.drop_with_heap(heap);
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

    /// Removes and returns a key-value pair from the dict.
    ///
    /// Returns Ok(Some((key, value))) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    ///
    /// Reference counting: does not decrement refcounts for removed key and value;
    /// caller assumes ownership and is responsible for managing their refcounts.
    pub fn pop(
        &mut self,
        key: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<(Value, Value)>> {
        let hash = key
            .py_hash(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(heap)))?;

        // Create a guard for key equality comparisons.
        let mut guard = DepthGuard::default();
        let entry = self.indices.entry(
            hash,
            |v| {
                key.py_eq(&self.entries[*v].key, heap, &mut guard, interns)
                    .unwrap_or(false)
            },
            |index| self.entries[*index].hash,
        );

        if let Entry::Occupied(occ_entry) = entry {
            let entry = self.entries.remove(*occ_entry.get());
            occ_entry.remove();
            // Don't decrement refcounts - caller now owns the values
            Ok(Some((entry.key, entry.value)))
        } else {
            Ok(None)
        }
    }

    /// Returns a vector of all keys in the dict with proper reference counting.
    ///
    /// Each key's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn keys(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<Value> {
        self.entries
            .iter()
            .map(|entry| entry.key.clone_with_heap(heap))
            .collect()
    }

    /// Returns a vector of all values in the dict with proper reference counting.
    ///
    /// Each value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn values(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<Value> {
        self.entries
            .iter()
            .map(|entry| entry.value.clone_with_heap(heap))
            .collect()
    }

    /// Returns a vector of all (key, value) pairs in the dict with proper reference counting.
    ///
    /// Each key and value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn items(&self, heap: &mut Heap<impl ResourceTracker>) -> Vec<(Value, Value)> {
        self.entries
            .iter()
            .map(|entry| (entry.key.clone_with_heap(heap), entry.value.clone_with_heap(heap)))
            .collect()
    }

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

    /// Creates a dict from the `dict()` constructor call.
    ///
    /// - `dict()` with no args returns an empty dict
    /// - `dict(dict)` returns a shallow copy of the dict
    ///
    /// Note: Full Python semantics also support dict(iterable) where iterable
    /// yields (key, value) pairs, and dict(**kwargs) for keyword arguments.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("dict", heap)?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Dict(Self::new()))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let Value::Ref(id) = &v else {
                    let err = ExcType::type_error_not_iterable(v.py_type(heap));
                    v.drop_with_heap(heap);
                    return Err(err);
                };
                let id = *id;

                // Check if it's a dict and get key-value pairs
                let HeapData::Dict(dict) = heap.get(id) else {
                    let err = ExcType::type_error_not_iterable(v.py_type(heap));
                    v.drop_with_heap(heap);
                    return Err(err);
                };

                // Copy all key-value pairs first (without incrementing refcounts)
                let pairs: Vec<(Value, Value)> = dict
                    .iter()
                    .map(|(k, v)| (k.copy_for_extend(), v.copy_for_extend()))
                    .collect();

                // Now we can drop the borrow and increment refcounts
                for (k, v) in &pairs {
                    if let Value::Ref(key_id) = k {
                        heap.inc_ref(*key_id);
                    }
                    if let Value::Ref(val_id) = v {
                        heap.inc_ref(*val_id);
                    }
                }
                v.drop_with_heap(heap);

                let new_dict = Self::from_pairs(pairs, heap, interns)?;
                let result = heap.allocate(HeapData::Dict(new_dict))?;
                Ok(Value::Ref(result))
            }
        }
    }

    fn find_index_hash(
        &self,
        key: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<(Option<usize>, u64)> {
        let hash = key
            .py_hash(heap, interns)
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(heap)))?;

        // Create a guard for key equality comparisons. Dict keys are typically
        // shallow (strings, ints, tuples of primitives), so recursion errors
        // are unlikely. If one occurs, treat it as "not equal" - the key lookup
        // fails but doesn't crash.
        let mut guard = DepthGuard::default();
        let opt_index = self
            .indices
            .find(hash, |v| {
                key.py_eq(&self.entries[*v].key, heap, &mut guard, interns)
                    .unwrap_or(false)
            })
            .copied();
        Ok((opt_index, hash))
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

impl PyTrait for Dict {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Dict
    }

    fn py_estimate_size(&self) -> usize {
        // Dict size: struct overhead + entries (2 Values per entry for key+value)
        std::mem::size_of::<Self>() + self.len() * 2 * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        if self.len() != other.len() {
            return Ok(false);
        }

        guard.increase_err()?;
        // Check that all keys in self exist in other with equal values
        for entry in &self.entries {
            if let Ok(Some(other_v)) = other.get(&entry.key, heap, interns) {
                if !entry.value.py_eq(other_v, heap, guard, interns)? {
                    guard.decrease();
                    return Ok(false);
                }
            } else {
                guard.decrease();
                return Ok(false);
            }
        }
        guard.decrease();
        Ok(true)
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

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("{}");
        }

        // Check depth limit before recursing
        if !guard.increase() {
            return f.write_str("{...}");
        }

        f.write_char('{')?;
        let mut first = true;
        for entry in &self.entries {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            entry.key.py_repr_fmt(f, heap, heap_ids, guard, interns)?;
            f.write_str(": ")?;
            entry.value.py_repr_fmt(f, heap, heap_ids, guard, interns)?;
        }
        f.write_char('}')?;

        guard.decrease();
        Ok(())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        // Use copy_for_extend to avoid borrow conflict, then increment refcount
        let result = self.get(key, heap, interns)?.map(Value::copy_for_extend);
        match result {
            Some(value) => {
                if let Value::Ref(id) = &value {
                    heap.inc_ref(*id);
                }
                Ok(value)
            }
            None => Err(ExcType::key_error(key, heap, interns)),
        }
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        // Drop the old value if one was replaced
        if let Some(old_value) = self.set(key, value, heap, interns)? {
            old_value.drop_with_heap(heap);
        }
        Ok(())
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(method) = attr.static_string() else {
            return Err(ExcType::attribute_error(Type::Dict, attr.as_str(interns)));
        };

        match method {
            StaticStrings::Get => {
                // dict.get() accepts 1 or 2 arguments
                let (key, default) = args.get_one_two_args("get", heap)?;
                let default = default.unwrap_or(Value::None);
                // Handle the lookup - may fail for unhashable keys
                let result = match self.get(&key, heap, interns) {
                    Ok(r) => r,
                    Err(e) => {
                        // Drop key and default before returning error
                        key.drop_with_heap(heap);
                        default.drop_with_heap(heap);
                        return Err(e);
                    }
                };
                let value = match result {
                    Some(v) => v.clone_with_heap(heap),
                    None => default.clone_with_heap(heap),
                };
                // Drop the key and default arguments
                key.drop_with_heap(heap);
                default.drop_with_heap(heap);
                Ok(value)
            }
            StaticStrings::Keys => {
                args.check_zero_args("dict.keys", heap)?;
                let keys = self.keys(heap);
                let list_id = heap.allocate(HeapData::List(List::new(keys)))?;
                Ok(Value::Ref(list_id))
            }
            StaticStrings::Values => {
                args.check_zero_args("dict.values", heap)?;
                let values = self.values(heap);
                let list_id = heap.allocate(HeapData::List(List::new(values)))?;
                Ok(Value::Ref(list_id))
            }
            StaticStrings::Items => {
                args.check_zero_args("dict.items", heap)?;
                // Return list of tuples
                let items = self.items(heap);
                let mut tuples: Vec<Value> = Vec::with_capacity(items.len());
                for (k, v) in items {
                    let tuple_val = allocate_tuple(smallvec![k, v], heap)?;
                    tuples.push(tuple_val);
                }
                let list_id = heap.allocate(HeapData::List(List::new(tuples)))?;
                Ok(Value::Ref(list_id))
            }
            StaticStrings::Pop => {
                // dict.pop() accepts 1 or 2 arguments (key, optional default)
                let (key, default) = args.get_one_two_args("pop", heap)?;
                let result = match self.pop(&key, heap, interns) {
                    Ok(r) => r,
                    Err(e) => {
                        // Clean up key and default before returning error
                        key.drop_with_heap(heap);
                        if let Some(d) = default {
                            d.drop_with_heap(heap);
                        }
                        return Err(e);
                    }
                };
                if let Some((old_key, value)) = result {
                    // Drop the old key - we don't need it
                    old_key.drop_with_heap(heap);
                    // Drop the lookup key and default arguments
                    key.drop_with_heap(heap);
                    if let Some(d) = default {
                        d.drop_with_heap(heap);
                    }
                    Ok(value)
                } else {
                    // No matching key - return default if provided, else KeyError
                    if let Some(d) = default {
                        key.drop_with_heap(heap);
                        Ok(d)
                    } else {
                        let err = ExcType::key_error(&key, heap, interns);
                        key.drop_with_heap(heap);
                        Err(err)
                    }
                }
            }
            StaticStrings::Clear => {
                args.check_zero_args("dict.clear", heap)?;
                dict_clear(self, heap);
                Ok(Value::None)
            }
            StaticStrings::Copy => {
                args.check_zero_args("dict.copy", heap)?;
                dict_copy(self, heap, interns)
            }
            StaticStrings::Update => dict_update(self, args, heap, interns),
            StaticStrings::Setdefault => dict_setdefault(self, args, heap, interns),
            StaticStrings::Popitem => {
                args.check_zero_args("dict.popitem", heap)?;
                dict_popitem(self, heap)
            }
            // fromkeys is a classmethod but also accessible on instances
            StaticStrings::Fromkeys => dict_fromkeys(args, heap, interns),
            _ => Err(ExcType::attribute_error(Type::Dict, attr.as_str(interns))),
        }
    }
}

/// Implements Python's `dict.clear()` method.
///
/// Removes all items from the dict.
fn dict_clear(dict: &mut Dict, heap: &mut Heap<impl ResourceTracker>) {
    for entry in dict.entries.drain(..) {
        entry.key.drop_with_heap(heap);
        entry.value.drop_with_heap(heap);
    }
    dict.indices.clear();
    // Note: contains_refs stays true even if all refs removed, per conservative GC strategy
}

/// Implements Python's `dict.copy()` method.
///
/// Returns a shallow copy of the dict.
fn dict_copy(dict: &Dict, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    // Copy all key-value pairs (incrementing refcounts)
    let pairs: Vec<(Value, Value)> = dict
        .iter()
        .map(|(k, v)| (k.clone_with_heap(heap), v.clone_with_heap(heap)))
        .collect();

    let new_dict = Dict::from_pairs(pairs, heap, interns)?;
    let heap_id = heap.allocate(HeapData::Dict(new_dict))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `dict.update([other], **kwargs)` method.
///
/// Updates the dict with key-value pairs from `other` and/or `kwargs`.
/// If `other` is a dict, copies its key-value pairs.
/// If `other` is an iterable, expects pairs of (key, value).
/// Keyword arguments are also added to the dict.
fn dict_update(
    dict: &mut Dict,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (pos_iter, kwargs) = args.into_parts();
    defer_drop_mut!(pos_iter, heap);

    let Some(other_value) = pos_iter.next() else {
        // No positional argument - just process kwargs
        return dict_update_from_kwargs(dict, kwargs, heap, interns);
    };

    // Check no extra positional arguments
    if pos_iter.len() != 0 {
        other_value.drop_with_heap(heap);
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_most("dict.update", 1, 2));
    }

    // Check if it's a dict first
    if let Value::Ref(id) = &other_value {
        let is_dict = matches!(heap.get(*id), HeapData::Dict(_));
        if is_dict {
            // Get key-value pairs from the source dict
            let pairs: Vec<(Value, Value)> = {
                let HeapData::Dict(src_dict) = heap.get(*id) else {
                    unreachable!()
                };
                src_dict
                    .iter()
                    .map(|(k, v)| (k.copy_for_extend(), v.copy_for_extend()))
                    .collect()
            };

            // Increment refcounts after releasing the borrow
            for (k, v) in &pairs {
                if let Value::Ref(key_id) = k {
                    heap.inc_ref(*key_id);
                }
                if let Value::Ref(val_id) = v {
                    heap.inc_ref(*val_id);
                }
            }

            // Now set each pair
            for (key, value) in pairs {
                if let Some(old_value) = dict.set(key, value, heap, interns)? {
                    old_value.drop_with_heap(heap);
                }
            }

            other_value.drop_with_heap(heap);
            // Process kwargs after the dict update
            return dict_update_from_kwargs(dict, kwargs, heap, interns);
        }
    }

    // Try as an iterable of pairs
    // Drop kwargs before propagating error to avoid refcount leak
    let mut iter = match MontyIter::new(other_value, heap, interns) {
        Ok(i) => i,
        Err(e) => {
            kwargs.drop_with_heap(heap);
            return Err(e);
        }
    };

    loop {
        // Drop iter and kwargs before propagating error to avoid refcount leak
        let item = match iter.for_next(heap, interns) {
            Ok(Some(i)) => i,
            Ok(None) => break,
            Err(e) => {
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        };

        // Each item should be a pair (iterable of 2 elements)
        // Drop iter and kwargs before propagating error to avoid refcount leak
        let mut pair_iter = match MontyIter::new(item, heap, interns) {
            Ok(pi) => pi,
            Err(e) => {
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        };

        // Drop pair_iter, iter, and kwargs before propagating error to avoid refcount leak
        let key = match pair_iter.for_next(heap, interns) {
            Ok(Some(k)) => k,
            Ok(None) => {
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 0; 2 is required",
                ));
            }
            Err(e) => {
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        };

        // Drop key, pair_iter, iter, and kwargs before propagating error to avoid refcount leak
        let value = match pair_iter.for_next(heap, interns) {
            Ok(Some(v)) => v,
            Ok(None) => {
                key.drop_with_heap(heap);
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length 1; 2 is required",
                ));
            }
            Err(e) => {
                key.drop_with_heap(heap);
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        };

        // Check for extra elements - must drop the first extra element too!
        match pair_iter.for_next(heap, interns) {
            Ok(Some(first_extra)) => {
                first_extra.drop_with_heap(heap);
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                // Drain remaining elements
                loop {
                    match pair_iter.for_next(heap, interns) {
                        Ok(Some(extra)) => extra.drop_with_heap(heap),
                        Ok(None) => break,
                        Err(_) => break, // Error while draining - just stop
                    }
                }
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "dictionary update sequence element has length > 2; 2 is required",
                ));
            }
            Ok(None) => {}
            Err(e) => {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                pair_iter.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        }
        pair_iter.drop_with_heap(heap);

        // Drop iter and kwargs before propagating error to avoid refcount leak
        // Note: key and value are consumed by dict.set
        match dict.set(key, value, heap, interns) {
            Ok(Some(old_value)) => old_value.drop_with_heap(heap),
            Ok(None) => {}
            Err(e) => {
                iter.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
                return Err(e);
            }
        }
    }

    iter.drop_with_heap(heap);
    // Process kwargs after the iterable update
    dict_update_from_kwargs(dict, kwargs, heap, interns)
}

/// Helper to update a dict from keyword arguments.
fn dict_update_from_kwargs(
    dict: &mut Dict,
    kwargs: KwargsValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    // Use while let to allow draining on error
    let mut kwargs_iter = kwargs.into_iter();
    while let Some((key, value)) = kwargs_iter.next() {
        // Drop key, value, and remaining kwargs before propagating error
        match dict.set(key, value, heap, interns) {
            Ok(Some(old_value)) => old_value.drop_with_heap(heap),
            Ok(None) => {}
            Err(e) => {
                for (k, v) in kwargs_iter {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
                return Err(e);
            }
        }
    }
    Ok(Value::None)
}

/// Implements Python's `dict.setdefault(key[, default])` method.
///
/// If key is in the dict, return its value.
/// If not, insert key with a value of default (or None) and return default.
fn dict_setdefault(
    dict: &mut Dict,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (key, default) = args.get_one_two_args("setdefault", heap)?;
    let default = default.unwrap_or(Value::None);

    // Check if key exists
    let result = match dict.get(&key, heap, interns) {
        Ok(r) => r,
        Err(e) => {
            key.drop_with_heap(heap);
            default.drop_with_heap(heap);
            return Err(e);
        }
    };

    if let Some(existing) = result {
        // Key exists - return its value (cloned)
        let value = existing.clone_with_heap(heap);
        key.drop_with_heap(heap);
        default.drop_with_heap(heap);
        Ok(value)
    } else {
        // Key doesn't exist - insert default and return it (cloned before insertion)
        let return_value = default.clone_with_heap(heap);
        if let Some(old_value) = dict.set(key, default, heap, interns)? {
            // This shouldn't happen since we checked, but handle it anyway
            old_value.drop_with_heap(heap);
        }
        Ok(return_value)
    }
}

/// Implements Python's `dict.popitem()` method.
///
/// Removes and returns the last inserted key-value pair as a tuple.
/// Raises KeyError if the dict is empty.
fn dict_popitem(dict: &mut Dict, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if dict.is_empty() {
        return Err(ExcType::key_error_popitem_empty_dict());
    }

    // Remove the last entry (LIFO order)
    let entry = dict.entries.pop().expect("dict is not empty");

    // Remove from indices - need to find the entry with this index
    // Since we removed the last entry, we need to clear and rebuild indices
    // (This is simpler than trying to find and remove the specific hash entry)
    // TODO: This O(n) rebuild could be optimized by finding and removing the
    // specific hash entry directly from the hashbrown table.
    dict.indices.clear();
    for (idx, e) in dict.entries.iter().enumerate() {
        dict.indices.insert_unique(e.hash, idx, |&i| dict.entries[i].hash);
    }

    // Create tuple (key, value)
    Ok(allocate_tuple(smallvec![entry.key, entry.value], heap)?)
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
pub fn dict_fromkeys(args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (iterable, default) = args.get_one_two_args("dict.fromkeys", heap)?;
    let default = default.unwrap_or(Value::None);

    // Iterate over the iterable to get keys
    // Drop default before propagating error to avoid refcount leak
    let iter_result = MontyIter::new(iterable, heap, interns);
    let mut iter = match iter_result {
        Ok(i) => i,
        Err(e) => {
            default.drop_with_heap(heap);
            return Err(e);
        }
    };

    let mut dict = Dict::new();

    loop {
        // Drop iter and default before propagating error to avoid refcount leak
        let next_result = iter.for_next(heap, interns);
        let key = match next_result {
            Ok(Some(k)) => k,
            Ok(None) => break,
            Err(e) => {
                iter.drop_with_heap(heap);
                default.drop_with_heap(heap);
                return Err(e);
            }
        };

        // Clone the default value for each key
        let value = default.clone_with_heap(heap);
        // Drop key, value, iter, default before propagating error
        let set_result = dict.set(key, value, heap, interns);
        match set_result {
            Ok(Some(old_value)) => old_value.drop_with_heap(heap),
            Ok(None) => {}
            Err(e) => {
                // Note: key and value are consumed by dict.set, so we only drop iter and default
                iter.drop_with_heap(heap);
                default.drop_with_heap(heap);
                return Err(e);
            }
        }
    }

    iter.drop_with_heap(heap);
    default.drop_with_heap(heap);

    let heap_id = heap.allocate(HeapData::Dict(dict))?;
    Ok(Value::Ref(heap_id))
}
