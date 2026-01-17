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
//!
//! ## Efficient Iteration with `IterState`
//!
//! For the VM's `ForIter` opcode, we use a two-phase approach to avoid borrow conflicts:
//! 1. `iter_state()` - reads current state without mutation, returns `IterState`
//! 2. `advance()` - updates the index after the caller has done its work
//!
//! This allows `Heap::advance_iterator()` to coordinate access without extracting
//! the iterator from the heap (avoiding `std::mem::replace` overhead).

use crate::{
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{BytesId, Interns},
    resource::ResourceTracker,
    types::{PyTrait, Range, str::allocate_char},
    value::Value,
};

/// Snapshot of iterator state needed to produce the next value.
///
/// This enum captures all data needed to get the next item from an iterator
/// WITHOUT holding a borrow on the iterator. This enables a two-phase approach:
/// 1. Read `IterState` from iterator (immutable borrow ends)
/// 2. Use the state to get the value (may access other heap objects)
/// 3. Call `advance()` to update the iterator index
///
/// For types that yield values directly (Range, IterStr), the value is included.
/// For types that need heap access (List, Tuple, etc.), indices are provided.
#[derive(Debug, Clone, Copy)]
pub enum IterState {
    /// Iterator is exhausted, no more values.
    Exhausted,
    /// Range iterator yields this integer value.
    Range(i64),
    /// List iterator needs to read item at this index from the list.
    List { list_id: HeapId, index: usize },
    /// Tuple iterator needs to read item at this index.
    Tuple { tuple_id: HeapId, index: usize },
    /// Dict iterator needs to read key at this index; check len for mutation.
    DictKeys {
        dict_id: HeapId,
        index: usize,
        expected_len: usize,
    },
    /// String iterator yields this character; char_len is UTF-8 byte length for advance().
    IterStr { char: char, char_len: usize },
    /// Heap bytes iterator needs to read byte at this index.
    HeapBytes { bytes_id: HeapId, index: usize },
    /// Interned bytes iterator needs to read byte at this index.
    InternBytes { bytes_id: BytesId, index: usize },
    /// Set iterator needs to read value at this index; check len for mutation.
    Set {
        set_id: HeapId,
        index: usize,
        expected_len: usize,
    },
    /// FrozenSet iterator needs to read value at this index.
    FrozenSet { frozenset_id: HeapId, index: usize },
}

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
            let err = ExcType::type_error_not_iterable(value.py_type(heap));
            value.drop_with_heap(heap);
            Err(err)
        }
    }

    pub fn drop_with_heap(self, heap: &mut Heap<impl ResourceTracker>) {
        self.value.drop_with_heap(heap);
    }

    /// Collects HeapIds from this iterator for reference counting cleanup.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.value.py_dec_ref_ids(stack);
    }

    /// Returns whether this iterator holds a heap reference (`Value::Ref`).
    ///
    /// Used during allocation to determine if this container could create cycles.
    #[inline]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        matches!(self.value, Value::Ref(_))
    }

    /// Returns a reference to the underlying value being iterated.
    ///
    /// Used by GC to traverse heap references held by the iterator.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Returns the current iterator state without mutation.
    ///
    /// This is phase 1 of the two-phase iteration approach used by `Heap::advance_iterator()`.
    /// The returned `IterState` captures all data needed to produce the next value.
    /// After using the state, call `advance()` to update the iterator.
    ///
    /// Returns `IterState::Exhausted` if the iterator has no more values.
    pub fn iter_state(&self) -> IterState {
        match &self.iter_value {
            ForIterValue::Range { start, step, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    let idx = i64::try_from(self.index).expect("iterator index exceeds i64::MAX");
                    let value = *start + idx * *step;
                    IterState::Range(value)
                }
            }
            ForIterValue::List { heap_id } => {
                // Note: List length is checked later in Heap::advance_iterator
                // because lists can be mutated during iteration
                IterState::List {
                    list_id: *heap_id,
                    index: self.index,
                }
            }
            ForIterValue::Tuple { heap_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::Tuple {
                        tuple_id: *heap_id,
                        index: self.index,
                    }
                }
            }
            ForIterValue::DictKeys { heap_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::DictKeys {
                        dict_id: *heap_id,
                        index: self.index,
                        expected_len: *len,
                    }
                }
            }
            ForIterValue::IterStr {
                string,
                byte_offset,
                len,
            } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    // Get the next character at current byte offset
                    let c = string[*byte_offset..]
                        .chars()
                        .next()
                        .expect("index < len implies char exists");
                    IterState::IterStr {
                        char: c,
                        char_len: c.len_utf8(),
                    }
                }
            }
            ForIterValue::HeapBytes { heap_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::HeapBytes {
                        bytes_id: *heap_id,
                        index: self.index,
                    }
                }
            }
            ForIterValue::InternBytes { bytes_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::InternBytes {
                        bytes_id: *bytes_id,
                        index: self.index,
                    }
                }
            }
            ForIterValue::Set { heap_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::Set {
                        set_id: *heap_id,
                        index: self.index,
                        expected_len: *len,
                    }
                }
            }
            ForIterValue::FrozenSet { heap_id, len } => {
                if self.index >= *len {
                    IterState::Exhausted
                } else {
                    IterState::FrozenSet {
                        frozenset_id: *heap_id,
                        index: self.index,
                    }
                }
            }
        }
    }

    /// Advances the iterator by one step.
    ///
    /// This is phase 2 of the two-phase iteration approach. Call this after
    /// successfully retrieving the value using the data from `iter_state()`.
    ///
    /// For string iterators, `string_char_len` must be provided (the UTF-8 byte
    /// length of the character that was just yielded) to update the byte offset.
    /// For other iterator types, pass `None`.
    #[inline]
    pub fn advance(&mut self, string_char_len: Option<usize>) {
        self.index += 1;
        if let Some(char_len) = string_char_len
            && let ForIterValue::IterStr { byte_offset, .. } = &mut self.iter_value
        {
            *byte_offset += char_len;
        }
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
                let idx = i64::try_from(self.index).expect("iterator index exceeds i64::MAX");
                let value = *start + idx * *step;
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
                Ok(Some(allocate_char(c, heap)?))
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
            // Closures, FunctionDefaults, Cells, Exceptions, Dataclasses, Iterators, and LongInts are not iterable
            HeapData::Closure(_, _, _)
            | HeapData::FunctionDefaults(_, _)
            | HeapData::Cell(_)
            | HeapData::Exception(_)
            | HeapData::Dataclass(_)
            | HeapData::Iterator(_)
            | HeapData::LongInt(_) => None,
        }
    }
}
