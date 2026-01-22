//! Iterator support for Python for loops and the `iter()` type constructor.
//!
//! This module provides the `MontyIter` struct which encapsulates iteration state
//! for different iterable types. It uses index-based iteration internally to avoid
//! borrow conflicts when accessing the heap during iteration.
//!
//! The design stores iteration state (indices) rather than Rust iterators, allowing
//! `for_next()` to take `&mut Heap` for cloning values and allocating strings.
//!
//! For constructors like `list()` and `tuple()`, use `MontyIter::new()` followed
//! by `collect()` to materialize all items into a Vec.
//!
//! ## Efficient Iteration with `IterState`
//!
//! For the VM's `ForIter` opcode, `advance_on_heap()` uses two strategies:
//!
//! **Fast path** for simple iterators (Range, InternBytes, ASCII IterStr):
//! - Single `get_mut()` call to compute value and advance index
//! - No additional heap access needed during iteration
//!
//! **Multi-phase approach** for complex iterators (IterStr, HeapRef):
//! 1. `iter_state()` - reads current state without mutation, returns `Option<IterState>`
//! 2. Get the value (may access other heap objects like strings or containers)
//! 3. `advance()` - updates the index after the caller has done its work
//!
//! This allows `advance_on_heap()` to coordinate access without extracting
//! the iterator from the heap (avoiding `std::mem::replace` overhead).
//!
//! ## Builtin Support
//!
//! The `iterator_next()` helper implements the `next()` builtin.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{BytesId, Interns, StringId},
    resource::ResourceTracker,
    types::{PyTrait, Range, str::allocate_char},
    value::Value,
};

/// Iterator state for Python for loops.
///
/// Contains the current iteration index and the type-specific iteration data.
/// Uses index-based iteration to avoid borrow conflicts when accessing the heap.
///
/// For strings, stores the string content with a byte offset for O(1) UTF-8 iteration.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MontyIter {
    /// Current iteration index, shared across all iterator types.
    index: usize,
    /// Type-specific iteration data.
    iter_value: IterValue,
    /// the actual Value being iterated over.
    value: Value,
}

impl MontyIter {
    /// Creates an iterator from the `iter()` constructor call.
    ///
    /// - `iter(iterable)` - Returns an iterator for the iterable. If the argument is
    ///   already an iterator, returns the same object.
    /// - `iter(callable, sentinel)` - Not yet supported.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let (iterable, sentinel) = args.get_one_two_args("iter", heap)?;

        if let Some(s) = sentinel {
            // Two-argument form: iter(callable, sentinel)
            // This is the sentinel iteration protocol, not yet supported
            iterable.drop_with_heap(heap);
            s.drop_with_heap(heap);
            return Err(ExcType::type_error("iter(callable, sentinel) is not yet supported"));
        }

        // Check if already an iterator - return self
        if let Value::Ref(id) = &iterable
            && matches!(heap.get(*id), HeapData::Iter(_))
        {
            // Already an iterator - return it (refcount already correct from caller)
            return Ok(iterable);
        }

        // Create new iterator
        let iter = Self::new(iterable, heap, interns)?;
        let id = heap.allocate(HeapData::Iter(iter))?;
        Ok(Value::Ref(id))
    }

    /// Creates a new MontyIter from a Value.
    ///
    /// Returns an error if the value is not iterable.
    /// For strings, copies the string content for byte-offset based iteration.
    /// For ranges, the data is copied so the heap reference is dropped immediately.
    pub fn new(mut value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        if let Some(iter_value) = IterValue::new(&value, heap, interns) {
            // For Range, we copy next/step/len into ForIterValue::Range, so we don't need
            // to keep the heap object alive during iteration. Drop it immediately to avoid
            // GC issues (the Range isn't in any namespace slot, so GC wouldn't see it).
            // Same for IterStr which copies the string content.
            if matches!(iter_value, IterValue::Range { .. } | IterValue::IterStr { .. }) {
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

    /// Drops the iterator and its held value properly.
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
    /// This is used by the multi-phase approach in `advance_on_heap()` for complex
    /// iterator types (IterStr, HeapRef). Simple types (Range, InternBytes, ASCII
    /// IterStr) are handled by the fast path and should not call this method.
    ///
    /// Returns `None` if the iterator is exhausted.
    fn iter_state(&self) -> Option<IterState> {
        match &self.iter_value {
            // Range, InternBytes, and ASCII IterStr are handled by try_advance_simple() fast path
            IterValue::Range { .. } | IterValue::InternBytes { .. } => {
                unreachable!("Range and InternBytes use fast path, not iter_state")
            }
            IterValue::IterStr {
                string,
                byte_offset,
                len,
                ..
            } => {
                if self.index >= *len {
                    None
                } else {
                    // Get the next character at current byte offset
                    let c = string[*byte_offset..]
                        .chars()
                        .next()
                        .expect("index < len implies char exists");
                    Some(IterState::IterStr {
                        char: c,
                        char_len: c.len_utf8(),
                    })
                }
            }
            IterValue::HeapRef {
                heap_id,
                len,
                checks_mutation,
            } => {
                // For types with captured len, check exhaustion here.
                // For List (len=None), exhaustion is checked in advance_on_heap().
                if let Some(l) = len
                    && self.index >= *l
                {
                    return None;
                }
                Some(IterState::HeapIndex {
                    heap_id: *heap_id,
                    index: self.index,
                    expected_len: if *checks_mutation { *len } else { None },
                })
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
            && let IterValue::IterStr { byte_offset, .. } = &mut self.iter_value
        {
            *byte_offset += char_len;
        }
    }

    /// Attempts to advance simple iterator types that don't need additional heap access.
    ///
    /// Returns `Some(result)` if handled (Range, InternBytes, ASCII IterStr),
    /// `None` if caller should use the multi-phase approach (non-ASCII IterStr, HeapRef).
    ///
    /// This optimization avoids two heap lookups for iterator types that can compute
    /// their next value without accessing other heap objects.
    #[inline]
    fn try_advance_simple(&mut self, interns: &Interns) -> Option<RunResult<Option<Value>>> {
        match &mut self.iter_value {
            IterValue::Range { next, step, len } => {
                if self.index >= *len {
                    Some(Ok(None))
                } else {
                    let value = *next;
                    *next += *step;
                    self.index += 1;
                    Some(Ok(Some(Value::Int(value))))
                }
            }
            IterValue::IterStr {
                string,
                byte_offset,
                len,
                is_ascii,
            } => {
                if !*is_ascii {
                    None
                } else if self.index >= *len {
                    Some(Ok(None))
                } else {
                    let byte = string.as_bytes()[*byte_offset];
                    *byte_offset += 1;
                    self.index += 1;
                    Some(Ok(Some(Value::InternString(StringId::from_ascii(byte)))))
                }
            }
            IterValue::InternBytes { bytes_id, len } => {
                if self.index >= *len {
                    Some(Ok(None))
                } else {
                    let i = self.index;
                    self.index += 1;
                    let bytes = interns.get_bytes(*bytes_id);
                    Some(Ok(Some(Value::Int(i64::from(bytes[i])))))
                }
            }
            IterValue::HeapRef { .. } => None,
        }
    }

    /// Returns the next item from the iterator, advancing the internal index.
    ///
    /// Returns `Ok(None)` when the iterator is exhausted.
    /// Returns `Err` if allocation fails (for string character iteration) or if
    /// a dict/set changes size during iteration (RuntimeError).
    pub fn for_next(&mut self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Option<Value>> {
        match &mut self.iter_value {
            IterValue::Range { next, step, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let value = *next;
                *next += *step;
                self.index += 1;
                Ok(Some(Value::Int(value)))
            }
            IterValue::IterStr {
                string,
                byte_offset,
                len,
                is_ascii,
            } => {
                if self.index >= *len {
                    Ok(None)
                } else if *is_ascii {
                    let byte = string.as_bytes()[*byte_offset];
                    *byte_offset += 1;
                    self.index += 1;
                    Ok(Some(Value::InternString(StringId::from_ascii(byte))))
                } else {
                    // Get next char at current byte offset
                    let c = string[*byte_offset..]
                        .chars()
                        .next()
                        .expect("index < len implies char exists");
                    *byte_offset += c.len_utf8();
                    self.index += 1;
                    Ok(Some(allocate_char(c, heap)?))
                }
            }
            IterValue::InternBytes { bytes_id, len } => {
                if self.index >= *len {
                    return Ok(None);
                }
                let i = self.index;
                self.index += 1;
                let bytes = interns.get_bytes(*bytes_id);
                Ok(Some(Value::Int(i64::from(bytes[i]))))
            }
            IterValue::HeapRef {
                heap_id,
                len,
                checks_mutation,
            } => {
                // Check exhaustion for types with captured len
                if let Some(l) = len
                    && self.index >= *l
                {
                    return Ok(None);
                }
                let i = self.index;
                let expected_len = if *checks_mutation { *len } else { None };
                let item = get_heap_item(heap, *heap_id, i, expected_len)?;
                // Check for list exhaustion (list can shrink during iteration)
                let Some(item) = item else {
                    return Ok(None);
                };
                self.index += 1;
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
            IterValue::Range { len, .. } | IterValue::IterStr { len, .. } | IterValue::InternBytes { len, .. } => *len,
            IterValue::HeapRef { heap_id, len, .. } => {
                // For List (len=None), check current length dynamically
                len.unwrap_or_else(|| {
                    let HeapData::List(list) = heap.get(*heap_id) else {
                        panic!("HeapRef with len=None should only be List")
                    };
                    list.len()
                })
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

/// Advances an iterator stored on the heap and returns the next value.
///
/// Uses a fast path for simple iterators (Range, InternBytes, ASCII IterStr) that don't need
/// additional heap access - these are handled with a single mutable borrow.
///
/// For complex iterators (IterStr, HeapRef), uses a multi-phase approach:
/// 1. Read iterator state (immutable borrow ends)
/// 2. Based on state, get the value (may access other heap objects)
/// 3. Update iterator index (mutable borrow)
///
/// This is more efficient than `std::mem::replace` with a placeholder because
/// it avoids creating and moving placeholder objects on every iteration.
///
/// Returns `Ok(None)` when the iterator is exhausted.
/// Returns `Err` for dict/set size changes or allocation failures.
pub(crate) fn advance_on_heap(
    heap: &mut Heap<impl ResourceTracker>,
    iter_id: HeapId,
    interns: &Interns,
) -> RunResult<Option<Value>> {
    // Fast path: Range and InternBytes don't need additional heap access,
    // so we can handle them with a single mutable borrow.
    {
        let HeapData::Iter(iter) = heap.get_mut(iter_id) else {
            panic!("advance_on_heap: expected Iterator on heap");
        };
        if let Some(result) = iter.try_advance_simple(interns) {
            return result;
        }
    }
    // Mutable borrow ends here, allowing the multi-phase approach below

    // Multi-phase approach for IterStr and HeapRef (need heap access during value retrieval)
    // Phase 1: Get iterator state (immutable borrow ends after this block)
    let HeapData::Iter(iter) = heap.get(iter_id) else {
        panic!("advance_on_heap: expected Iterator on heap");
    };
    let Some(state) = iter.iter_state() else {
        return Ok(None); // Iterator exhausted
    };

    // Phase 2: Based on state, get the value and determine char_len for strings
    let (value, string_char_len) = match state {
        IterState::IterStr { char, char_len } => {
            let value = allocate_char(char, heap)?;
            (value, Some(char_len))
        }
        IterState::HeapIndex {
            heap_id,
            index,
            expected_len,
        } => {
            let item = get_heap_item(heap, heap_id, index, expected_len)?;
            // Check for list exhaustion (list can shrink during iteration)
            let Some(item) = item else {
                return Ok(None);
            };
            // Inc refcount after borrow ends
            if let Value::Ref(id) = &item {
                heap.inc_ref(*id);
            }
            (item, None)
        }
    };

    // Phase 3: Advance the iterator
    let HeapData::Iter(iter) = heap.get_mut(iter_id) else {
        panic!("advance_on_heap: expected Iterator on heap");
    };
    iter.advance(string_char_len);

    Ok(Some(value))
}

/// Gets an item from a heap-allocated container at the given index.
///
/// Returns `Ok(None)` if the index is out of bounds (for lists that shrunk during iteration).
/// Returns `Err` if a dict/set changed size during iteration (RuntimeError).
fn get_heap_item(
    heap: &Heap<impl ResourceTracker>,
    heap_id: HeapId,
    index: usize,
    expected_len: Option<usize>,
) -> RunResult<Option<Value>> {
    match heap.get(heap_id) {
        HeapData::List(list) => {
            // Check if list shrunk during iteration
            if index >= list.len() {
                return Ok(None);
            }
            Ok(Some(list.as_vec()[index].copy_for_extend()))
        }
        HeapData::Tuple(tuple) => Ok(Some(tuple.as_vec()[index].copy_for_extend())),
        HeapData::NamedTuple(namedtuple) => Ok(Some(namedtuple.as_vec()[index].copy_for_extend())),
        HeapData::Dict(dict) => {
            // Check for dict mutation
            if let Some(expected) = expected_len
                && dict.len() != expected
            {
                return Err(ExcType::runtime_error_dict_changed_size());
            }
            Ok(Some(
                dict.key_at(index).expect("index should be valid").copy_for_extend(),
            ))
        }
        HeapData::Bytes(bytes) => Ok(Some(Value::Int(i64::from(bytes.as_slice()[index])))),
        HeapData::Set(set) => {
            // Check for set mutation
            if let Some(expected) = expected_len
                && set.len() != expected
            {
                return Err(ExcType::runtime_error_set_changed_size());
            }
            Ok(Some(
                set.storage()
                    .value_at(index)
                    .expect("index should be valid")
                    .copy_for_extend(),
            ))
        }
        HeapData::FrozenSet(frozenset) => Ok(Some(
            frozenset
                .storage()
                .value_at(index)
                .expect("index should be valid")
                .copy_for_extend(),
        )),
        _ => panic!("get_heap_item: unexpected heap data type"),
    }
}

/// Gets the next item from an iterator.
///
/// If the iterator is exhausted:
/// - If `default` is `Some`, returns the default value
/// - If `default` is `None`, raises `StopIteration`
///
/// This implements Python's `next()` builtin semantics.
///
/// # Arguments
/// * `iter_value` - Must be an iterator (heap-allocated MontyIter)
/// * `default` - Optional default value to return when exhausted
/// * `heap` - The heap for memory operations
/// * `interns` - String interning table
///
/// # Errors
/// Returns `StopIteration` if exhausted with no default, or propagates errors from iteration.
pub fn iterator_next(
    iter_value: &Value,
    default: Option<Value>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let Value::Ref(iter_id) = iter_value else {
        // Not a heap value - can't be an iterator
        if let Some(d) = default {
            d.drop_with_heap(heap);
        }
        return Err(ExcType::type_error_not_iterable(iter_value.py_type(heap)));
    };

    // Check that it's actually an iterator
    if !matches!(heap.get(*iter_id), HeapData::Iter(_)) {
        if let Some(d) = default {
            d.drop_with_heap(heap);
        }
        let data_type = heap.get(*iter_id).py_type(heap);
        return Err(ExcType::type_error(format!("'{data_type}' object is not an iterator")));
    }

    // Get next item using the MontyIter::advance_on_heap method
    match advance_on_heap(heap, *iter_id, interns)? {
        Some(item) => {
            // Drop default if provided since we don't need it
            if let Some(d) = default {
                d.drop_with_heap(heap);
            }
            Ok(item)
        }
        None => {
            // Iterator exhausted
            match default {
                Some(d) => Ok(d),
                None => Err(ExcType::stop_iteration()),
            }
        }
    }
}

/// Snapshot of iterator state needed to produce the next value.
///
/// This enum captures state for complex iterator types (IterStr, HeapRef) that
/// require the multi-phase approach in `advance_on_heap()`. Simple types (Range,
/// InternBytes, ASCII IterStr) are handled by the fast path and don't use this enum.
///
/// The multi-phase approach avoids borrow conflicts:
/// 1. Read `Option<IterState>` from iterator (immutable borrow ends, `None` means exhausted)
/// 2. Use the state to get the value (may access other heap objects)
/// 3. Call `advance()` to update the iterator index
#[derive(Debug, Clone, Copy)]
enum IterState {
    /// String iterator yields this character; char_len is UTF-8 byte length for advance().
    IterStr { char: char, char_len: usize },
    /// Heap-based iterator (List, Tuple, NamedTuple, Dict, Bytes, Set, FrozenSet).
    /// The expected_len is Some for types that check for mutation (Dict, Set).
    HeapIndex {
        heap_id: HeapId,
        index: usize,
        expected_len: Option<usize>,
    },
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
/// excluding the index which is stored in the parent `MontyIter` struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum IterValue {
    /// Iterating over a Range, yields `Value::Int`.
    Range {
        /// Next value to yield.
        next: i64,
        /// Step between values.
        step: i64,
        /// Total number of elements.
        len: usize,
    },
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
        /// Whether the string is ASCII (enables fast-path iteration).
        is_ascii: bool,
    },
    /// Iterating over interned bytes, yields `Value::Int` for each byte.
    InternBytes { bytes_id: BytesId, len: usize },
    /// Iterating over a heap-allocated container (List, Tuple, NamedTuple, Dict, Bytes, Set, FrozenSet).
    ///
    /// - `len`: `None` for List (checked dynamically since lists can mutate during iteration),
    ///   `Some(n)` for other types (captured at construction for exhaustion checking).
    /// - `checks_mutation`: `true` for Dict/Set (raises RuntimeError if size changes),
    ///   `false` for other types.
    HeapRef {
        heap_id: HeapId,
        len: Option<usize>,
        checks_mutation: bool,
    },
}

impl IterValue {
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
            next: range.start,
            step: range.step,
            len: range.len(),
        }
    }

    /// Creates an iterator value over a string.
    ///
    /// Copies the string content and counts characters for the length field.
    fn from_str(s: &str) -> Self {
        let is_ascii = s.is_ascii();
        let len = if is_ascii { s.len() } else { s.chars().count() };
        Self::IterStr {
            string: s.to_owned(),
            byte_offset: 0,
            len,
            is_ascii,
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
            // List: no captured len (checked dynamically), no mutation check
            HeapData::List(_) => Some(Self::HeapRef {
                heap_id,
                len: None,
                checks_mutation: false,
            }),
            // Tuple/NamedTuple/Bytes/FrozenSet: captured len, no mutation check
            HeapData::Tuple(tuple) => Some(Self::HeapRef {
                heap_id,
                len: Some(tuple.as_vec().len()),
                checks_mutation: false,
            }),
            HeapData::NamedTuple(namedtuple) => Some(Self::HeapRef {
                heap_id,
                len: Some(namedtuple.len()),
                checks_mutation: false,
            }),
            HeapData::Bytes(b) => Some(Self::HeapRef {
                heap_id,
                len: Some(b.len()),
                checks_mutation: false,
            }),
            HeapData::FrozenSet(frozenset) => Some(Self::HeapRef {
                heap_id,
                len: Some(frozenset.len()),
                checks_mutation: false,
            }),
            // Dict/Set: captured len, WITH mutation check
            HeapData::Dict(dict) => Some(Self::HeapRef {
                heap_id,
                len: Some(dict.len()),
                checks_mutation: true,
            }),
            HeapData::Set(set) => Some(Self::HeapRef {
                heap_id,
                len: Some(set.len()),
                checks_mutation: true,
            }),
            // String: copy content for iteration
            HeapData::Str(s) => Some(Self::from_str(s.as_str())),
            // Range: copy values for iteration
            HeapData::Range(range) => Some(Self::from_range(range)),
            // Closures, FunctionDefaults, Cells, Exceptions, Dataclasses, Iterators, LongInts, Slices, and Modules are
            // not iterable
            HeapData::Closure(_, _, _)
            | HeapData::FunctionDefaults(_, _)
            | HeapData::Cell(_)
            | HeapData::Exception(_)
            | HeapData::Dataclass(_)
            | HeapData::Iter(_)
            | HeapData::LongInt(_)
            | HeapData::Slice(_)
            | HeapData::Module(_) => None,
        }
    }
}
