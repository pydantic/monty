use std::fmt::Write;

use ahash::AHashSet;
use hashbrown::{hash_table::Entry, HashTable};

use super::{List, PyTrait, Tuple};
use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::{Heap, HeapData, HeapId},
    intern::{attr, Interns},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::Type,
    value::{Attr, Value},
};

/// Python dict type preserving insertion order.
///
/// This type provides Python dict semantics including dynamic key-value namespaces,
/// reference counting for heap values, and standard dict methods like get, keys,
/// values, items, and pop.
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
#[derive(Debug, Default)]
pub struct Dict {
    /// indices mapping from the entry hash to its index.
    indices: HashTable<usize>,
    /// entries is a dense vec maintaining entry order.
    entries: Vec<DictEntry>,
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
        }
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
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };

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
        let (opt_index, hash) = self.find_index_hash(&key, heap, interns)?;

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
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(Some(heap))))?;

        let entry = self.indices.entry(
            hash,
            |v| key.py_eq(&self.entries[*v].key, heap, interns),
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

    /// Creates a deep clone of this dict with proper reference counting.
    ///
    /// All heap-allocated keys and values have their reference counts
    /// incremented. This should be used instead of `.clone()` which would
    /// bypass reference counting.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self {
            indices: self.indices.clone(),
            entries: self
                .entries
                .iter()
                .map(|entry| DictEntry {
                    key: entry.key.clone_with_heap(heap),
                    value: entry.value.clone_with_heap(heap),
                    hash: entry.hash,
                })
                .collect(),
        }
    }

    /// Creates a dict from the `dict()` constructor call.
    ///
    /// - `dict()` with no args returns an empty dict
    /// - `dict(dict)` returns a shallow copy of the dict
    ///
    /// Note: Full Python semantics also support dict(iterable) where iterable
    /// yields (key, value) pairs, and dict(**kwargs) for keyword arguments.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("dict")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Dict(Dict::new()))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let Value::Ref(id) = &v else {
                    let err = ExcType::type_error_not_iterable(v.py_type(Some(heap)));
                    v.drop_with_heap(heap);
                    return Err(err);
                };
                let id = *id;

                // Check if it's a dict and get key-value pairs
                let HeapData::Dict(dict) = heap.get(id) else {
                    let err = ExcType::type_error_not_iterable(v.py_type(Some(heap)));
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

                let new_dict = Dict::from_pairs(pairs, heap, interns)?;
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
            .ok_or_else(|| ExcType::type_error_unhashable_dict_key(key.py_type(Some(heap))))?;

        let opt_index = self
            .indices
            .find(hash, |v| key.py_eq(&self.entries[*v].key, heap, interns))
            .copied();
        Ok((opt_index, hash))
    }
}

/// Iterator over borrowed (key, value) pairs in a dict.
pub struct DictIter<'a>(std::slice::Iter<'a, DictEntry>);

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
pub struct DictIntoIter(std::vec::IntoIter<DictEntry>);

impl Iterator for DictIntoIter {
    type Item = (Value, Value);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|e| (e.key, e.value))
    }
}

impl IntoIterator for Dict {
    type Item = (Value, Value);
    type IntoIter = DictIntoIter;
    fn into_iter(self) -> Self::IntoIter {
        DictIntoIter(self.entries.into_iter())
    }
}

impl PyTrait for Dict {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Dict
    }

    fn py_estimate_size(&self) -> usize {
        // Dict size: struct overhead + entries (2 Values per entry for key+value)
        std::mem::size_of::<Self>() + self.len() * 2 * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.len() != other.len() {
            return false;
        }

        // Check that all keys in self exist in other with equal values
        for entry in &self.entries {
            match other.get(&entry.key, heap, interns) {
                Ok(Some(other_v)) => {
                    if !entry.value.py_eq(other_v, heap, interns) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
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
        interns: &Interns,
    ) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("{}");
        }

        f.write_char('{')?;
        let mut first = true;
        for entry in &self.entries {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            entry.key.py_repr_fmt(f, heap, heap_ids, interns)?;
            f.write_str(": ")?;
            entry.value.py_repr_fmt(f, heap, heap_ids, interns)?;
        }
        f.write_char('}')
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
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(attr_id) = attr.string_id() else {
            return Err(ExcType::attribute_error(Type::Dict, attr.as_str(interns)));
        };

        match attr_id {
            attr::GET => {
                // dict.get() accepts 1 or 2 arguments
                let (key, default) = args.get_one_two_args("get")?;
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
            attr::KEYS => {
                args.check_zero_args("dict.keys")?;
                let keys = self.keys(heap);
                let list_id = heap.allocate(HeapData::List(List::new(keys)))?;
                Ok(Value::Ref(list_id))
            }
            attr::VALUES => {
                args.check_zero_args("dict.values")?;
                let values = self.values(heap);
                let list_id = heap.allocate(HeapData::List(List::new(values)))?;
                Ok(Value::Ref(list_id))
            }
            attr::ITEMS => {
                args.check_zero_args("dict.items")?;
                // Return list of tuples
                let items = self.items(heap);
                let mut tuples: Vec<Value> = Vec::with_capacity(items.len());
                for (k, v) in items {
                    let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(vec![k, v])))?;
                    tuples.push(Value::Ref(tuple_id));
                }
                let list_id = heap.allocate(HeapData::List(List::new(tuples)))?;
                Ok(Value::Ref(list_id))
            }
            attr::POP => {
                // dict.pop() accepts 1 or 2 arguments (key, optional default)
                let (key, default) = args.get_one_two_args("pop")?;
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
            _ => Err(ExcType::attribute_error(Type::Dict, attr.as_str(interns))),
        }
    }
}

// Custom serde implementation for Dict.
// Only serializes entries; rebuilds the indices hash table on deserialize.
impl serde::Serialize for Dict {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Dict {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries: Vec<DictEntry> = Vec::deserialize(deserializer)?;
        // Rebuild the indices hash table from the entries
        let mut indices = HashTable::with_capacity(entries.len());
        for (idx, entry) in entries.iter().enumerate() {
            indices.insert_unique(entry.hash, idx, |&i| entries[i].hash);
        }
        Ok(Self { indices, entries })
    }
}
