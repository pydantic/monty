//! Iterator support for Python for loops and constructors.
//!
//! This module provides the `ForIterator` struct which encapsulates iteration state
//! for different iterable types. It uses index-based iteration internally to avoid
//! borrow conflicts when accessing the heap during iteration.
//!
//! The design stores iteration state (indices) rather than Rust iterators, allowing
//! `for_next()` to take `&mut Heap` for cloning values and allocating strings.
//!
//! For constructors like `list()` and `tuple()`, use `ForIterator::new()` followed
//! by `collect()` to materialize all items into a Vec.

use crate::{
    exception_private::ExcType,
    heap::{Heap, HeapData, HeapId},
    intern::{BytesId, Interns},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Range, Str},
    value::Value,
};

/// Iterator state for Python for loops.
///
/// Contains the current iteration index and the type-specific iteration data.
/// Uses index-based iteration to avoid borrow conflicts when accessing the heap.
///
/// For strings, stores the string content with a byte offset for O(1) UTF-8 iteration.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ForIterator {
    /// Current iteration index, shared across all iterator types.
    index: usize,
    /// Type-specific iteration data.
    iter_value: ForIterValue,
    /// the actual Value being iterated over.
    value: Value,
}

impl Clone for ForIterator {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            iter_value: self.iter_value.clone(),
            value: self.value.clone_immediate(),
        }
    }
}

impl ForIterator {
    /// Creates a new ForIterator from a Value.
    ///
    /// Returns `None` if the value is not iterable.
    /// For strings, copies the string content for byte-offset based iteration.
    /// For ranges, the data is copied so the heap reference is dropped immediately.
    pub fn new(mut value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        if let Some(iter_value) = ForIterValue::new(&value, heap, interns) {
            // For Range, we copy start/step/len into ForIterValue::Range, so we don't need
            // to keep the heap object alive during iteration. Drop it immediately to avoid
            // GC issues (the Range isn't in any namespace slot, so GC wouldn't see it).
            // Same for IterStr which copies the string content.
            if matches!(iter_value, ForIterValue::Range { .. } | ForIterValue::IterStr { .. }) {
                value.drop_with_heap(heap);
                value = Value::None;
            }
            Ok(Self {
                index: 0,
                iter_value,
                value,
            })
        } else {
            let err = ExcType::type_error_not_iterable(value.py_type(Some(heap)));
            value.drop_with_heap(heap);
            Err(err)
        }
    }

    pub fn drop_with_heap(self, heap: &mut Heap<impl ResourceTracker>) {
        self.value.drop_with_heap(heap);
    }

    /// Returns the next item from the iterator, advancing the internal index.
    ///
    /// Returns `Ok(None)` when the iterator is exhausted.
    /// Returns `Err` if allocation fails (for string character iteration) or if
    /// a dict/set changes size during iteration (RuntimeError).
    ///
    /// Use `decr()` to revert one step back (e.g., when snapshotting mid-iteration).
    pub fn for_next(&mut self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Option<Value>> {
        match &mut self.iter_value {
            ForIterValue::Range { start, step, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let value = *start + (self.index as i64) * *step;
                self.index += 1;
                Ok(Some(Value::Int(value)))
            }
            ForIterValue::List { heap_id } => {
                // Check current list length on each iteration (not captured snapshot)
                // to match CPython behavior where list mutation during iteration is allowed
                let i = self.index;
                let item = {
                    let HeapData::List(list) = heap.get(*heap_id) else {
                        unreachable!("ForIterValue::List should only hold list heap IDs")
                    };
                    if i >= list.len() {
                        return Ok(None);
                    }
                    list.as_vec()[i].copy_for_extend()
                };
                self.index += 1;
                Ok(Some(clone_and_inc_ref(item, heap)))
            }
            ForIterValue::Tuple { heap_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let item = {
                    let HeapData::Tuple(tuple) = heap.get(*heap_id) else {
                        unreachable!("ForIterValue::Tuple should only hold tuple heap IDs")
                    };
                    tuple.as_vec()[i].copy_for_extend()
                };
                Ok(Some(clone_and_inc_ref(item, heap)))
            }
            ForIterValue::DictKeys { heap_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let key = {
                    let HeapData::Dict(dict) = heap.get(*heap_id) else {
                        unreachable!("ForIterValue::DictKeys should only hold dict heap IDs")
                    };
                    // Check for dict mutation - if size changed, raise RuntimeError
                    if dict.len() != *len {
                        return Err(ExcType::runtime_error_dict_changed_size());
                    }
                    dict.key_at(i).expect("index should be valid").copy_for_extend()
                };
                Ok(Some(clone_and_inc_ref(key, heap)))
            }
            ForIterValue::IterStr {
                string,
                byte_offset,
                len,
            } => {
                if self.index >= *len {
                    return Ok(None);
                }
                // Get next char at current byte offset using char_indices pattern
                let c = string[*byte_offset..]
                    .chars()
                    .next()
                    .expect("index < len implies char exists");
                *byte_offset += c.len_utf8();
                self.index += 1;
                // Allocate a new single-character string on the heap
                let char_str = c.to_string();
                let char_id = heap.allocate(HeapData::Str(Str::new(char_str)))?;
                Ok(Some(Value::Ref(char_id)))
            }
            ForIterValue::HeapBytes { heap_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let HeapData::Bytes(bytes) = heap.get(*heap_id) else {
                    unreachable!("ForIterValue::HeapBytes should only hold bytes heap IDs")
                };
                Ok(Some(Value::Int(i64::from(bytes.as_slice()[i]))))
            }
            ForIterValue::InternBytes { bytes_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let bytes = interns.get_bytes(*bytes_id);
                Ok(Some(Value::Int(i64::from(bytes[i]))))
            }
            ForIterValue::Set { heap_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let item = {
                    let HeapData::Set(set) = heap.get(*heap_id) else {
                        unreachable!("ForIterValue::Set should only hold set heap IDs")
                    };
                    // Check for set mutation - if size changed, raise RuntimeError
                    if set.len() != *len {
                        return Err(ExcType::runtime_error_set_changed_size());
                    }
                    set.storage()
                        .value_at(i)
                        .expect("index should be valid")
                        .copy_for_extend()
                };
                Ok(Some(clone_and_inc_ref(item, heap)))
            }
            ForIterValue::FrozenSet { heap_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let item = {
                    let HeapData::FrozenSet(frozenset) = heap.get(*heap_id) else {
                        unreachable!("ForIterValue::FrozenSet should only hold frozenset heap IDs")
                    };
                    frozenset
                        .storage()
                        .value_at(i)
                        .expect("index should be valid")
                        .copy_for_extend()
                };
                Ok(Some(clone_and_inc_ref(item, heap)))
            }
        }
    }

    /// Decrements the iterator position by one step.
    ///
    /// Used when snapshotting mid-iteration: after `for_next()` returns a value and advances,
    /// call `decr()` before saving the iterator to `ClauseState::For`. On resume, `for_next()`
    /// will return the same value again.
    ///
    /// For strings, this scans backwards through the UTF-8 to find the previous character
    /// boundary. This is O(1) amortized since UTF-8 characters are at most 4 bytes.
    ///
    /// # Panics
    /// Panics if called when `index == 0` (cannot decrement below zero).
    pub fn decr(&mut self) {
        debug_assert!(self.index > 0, "cannot decrement iterator below zero");
        self.index -= 1;

        // For strings, move byte_offset back to the previous character's start
        if let ForIterValue::IterStr {
            string, byte_offset, ..
        } = &mut self.iter_value
        {
            // Find the previous character by scanning backwards.
            // UTF-8 continuation bytes have the form 10xxxxxx (0x80-0xBF).
            // Start bytes have other forms, so we scan back until we find one.
            let prev_char = string[..*byte_offset]
                .chars()
                .last()
                .expect("byte_offset > 0 implies previous char exists");
            *byte_offset -= prev_char.len_utf8();
        }
    }

    /// Returns the remaining size for iterables based on current state.
    ///
    /// For immutable types (Range, Tuple, Str, Bytes, FrozenSet), returns the exact remaining count.
    /// For List, returns current length minus index (may change if list is mutated).
    /// For Dict and Set, returns the captured length minus index (used for size-change detection).
    pub fn size_hint(&self, heap: &Heap<impl ResourceTracker>) -> usize {
        let len = match &self.iter_value {
            ForIterValue::Range { len, .. }
            | ForIterValue::Tuple { len, .. }
            | ForIterValue::IterStr { len, .. }
            | ForIterValue::HeapBytes { len, .. }
            | ForIterValue::InternBytes { len, .. }
            | ForIterValue::DictKeys { len, .. }
            | ForIterValue::Set { len, .. }
            | ForIterValue::FrozenSet { len, .. } => *len,
            ForIterValue::List { heap_id } => {
                let HeapData::List(list) = heap.get(*heap_id) else {
                    unreachable!("ForIterValue::List should only hold list heap IDs")
                };
                list.len()
            }
        };
        len.saturating_sub(self.index)
    }

    /// Collects all remaining items from the iterator into a Vec.
    ///
    /// Consumes the iterator and returns all items. Used by `list()`, `tuple()`,
    /// and similar constructors that need to materialize all items.
    ///
    /// Pre-allocates capacity based on `size_hint()` for better performance.
    pub fn collect(&mut self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Vec<Value>> {
        let mut items = Vec::with_capacity(self.size_hint(heap));
        while let Some(item) = self.for_next(heap, interns)? {
            items.push(item);
        }
        Ok(items)
    }
}

/// Increments the reference count for a value copied via `copy_for_extend()`.
///
/// This is the second half of the two-phase clone pattern: first copy the value
/// without incrementing refcount (to avoid borrow conflicts), then increment
/// the refcount once the heap borrow is released.
fn clone_and_inc_ref(value: Value, heap: &mut Heap<impl ResourceTracker>) -> Value {
    if let Value::Ref(ref_id) = &value {
        heap.inc_ref(*ref_id);
    }
    value
}

/// Type-specific iteration data for different Python iterable types.
///
/// Each variant stores the data needed to iterate over a specific type,
/// excluding the index which is stored in the parent `ForIterator` struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum ForIterValue {
    /// Iterating over a Range, yields `Value::Int`.
    Range { start: i64, step: i64, len: usize },
    /// Iterating over a heap-allocated List, yields cloned items.
    /// Unlike dict, list mutation during iteration is allowed in Python - the iterator
    /// checks the current list length on each iteration (not a captured snapshot).
    List { heap_id: HeapId },
    /// Iterating over a heap-allocated Tuple, yields cloned items.
    /// Tuples are immutable so we capture the length at construction.
    Tuple { heap_id: HeapId, len: usize },
    /// Iterating over a heap-allocated Dict, yields cloned keys.
    /// Checks `len` against current dict size to detect mutation (raises RuntimeError).
    DictKeys { heap_id: HeapId, len: usize },
    /// Iterating over a string (heap or interned), yields single-char Str values.
    ///
    /// Stores a copy of the string content plus a byte offset for O(1) UTF-8 character access.
    /// We store the string rather than referencing the heap because `for_next()` needs mutable
    /// heap access to allocate the returned character strings, which would conflict with
    /// borrowing the source string from the heap.
    IterStr {
        /// Copy of the string content for iteration.
        string: String,
        /// Current byte offset into the string (points to next char to yield).
        byte_offset: usize,
        /// Total number of characters in the string.
        len: usize,
    },
    /// Iterating over a heap-allocated Bytes, yields `Value::Int` for each byte.
    HeapBytes { heap_id: HeapId, len: usize },
    /// Iterating over interned bytes, yields `Value::Int` for each byte.
    InternBytes { bytes_id: BytesId, len: usize },
    /// Iterating over a heap-allocated Set, yields cloned values.
    /// Checks `len` against current set size to detect mutation (raises RuntimeError).
    Set { heap_id: HeapId, len: usize },
    /// Iterating over a heap-allocated FrozenSet, yields cloned values.
    /// FrozenSets are immutable so we capture the length at construction.
    FrozenSet { heap_id: HeapId, len: usize },
}

impl ForIterValue {
    fn new(value: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<Self> {
        match &value {
            Value::InternString(string_id) => Some(Self::from_str(interns.get_str(*string_id))),
            Value::InternBytes(bytes_id) => Some(Self::from_intern_bytes(*bytes_id, interns)),
            Value::Ref(heap_id) => Self::from_heap_data(*heap_id, heap),
            _ => None,
        }
    }

    /// Creates a Range iterator value.
    fn from_range(range: &Range) -> Self {
        Self::Range {
            start: range.start,
            step: range.step,
            len: range.len(),
        }
    }

    /// Creates an iterator value over a string.
    ///
    /// Copies the string content and counts characters for the length field.
    fn from_str(s: &str) -> Self {
        Self::IterStr {
            string: s.to_owned(),
            byte_offset: 0,
            len: s.chars().count(),
        }
    }

    /// Creates an iterator value over interned bytes.
    fn from_intern_bytes(bytes_id: BytesId, interns: &Interns) -> Self {
        let bytes = interns.get_bytes(bytes_id);
        Self::InternBytes {
            bytes_id,
            len: bytes.len(),
        }
    }

    /// Creates an iterator value from heap data.
    fn from_heap_data(heap_id: HeapId, heap: &Heap<impl ResourceTracker>) -> Option<Self> {
        match heap.get(heap_id) {
            HeapData::List(_) => Some(Self::List { heap_id }),
            HeapData::Tuple(tuple) => Some(Self::Tuple {
                heap_id,
                len: tuple.as_vec().len(),
            }),
            HeapData::Dict(dict) => Some(Self::DictKeys {
                heap_id,
                len: dict.len(),
            }),
            HeapData::Str(s) => Some(Self::from_str(s.as_str())),
            HeapData::Bytes(b) => Some(Self::HeapBytes { heap_id, len: b.len() }),
            HeapData::Set(set) => Some(Self::Set {
                heap_id,
                len: set.len(),
            }),
            HeapData::FrozenSet(frozenset) => Some(Self::FrozenSet {
                heap_id,
                len: frozenset.len(),
            }),
            HeapData::Range(range) => Some(Self::from_range(range)),
            // Closures, FunctionDefaults, Cells, Exceptions, and Dataclasses are not iterable
            HeapData::Closure(_, _, _)
            | HeapData::FunctionDefaults(_, _)
            | HeapData::Cell(_)
            | HeapData::Exception(_)
            | HeapData::Dataclass(_) => None,
        }
    }
}
