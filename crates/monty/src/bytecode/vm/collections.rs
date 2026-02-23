//! Collection building and unpacking helpers for the VM.

use smallvec::SmallVec;

use super::VM;
use crate::{
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, SimpleException},
    heap::{HeapData, HeapGuard},
    intern::StringId,
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Set, Slice, Type, allocate_tuple, slice::value_to_option_i64, str::allocate_char},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, '_, T> {
    /// Builds a list from the top n stack values.
    pub(super) fn build_list(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let list = List::new(items);
        let heap_id = self.heap.allocate(HeapData::List(list))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a tuple from the top n stack values.
    ///
    /// Uses the empty tuple singleton when count is 0, and SmallVec
    /// optimization for small tuples (≤2 elements).
    pub(super) fn build_tuple(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let value = allocate_tuple(items.into(), self.heap)?;
        self.push(value);
        Ok(())
    }

    /// Builds a dict from the top 2n stack values (key/value pairs).
    pub(super) fn build_dict(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count * 2);
        let mut dict = Dict::new();
        // Use into_iter to consume items by value, avoiding clone and proper ownership transfer
        let mut iter = items.into_iter();
        while let (Some(key), Some(value)) = (iter.next(), iter.next()) {
            dict.set(key, value, self.heap, self.interns)?;
        }
        let heap_id = self.heap.allocate(HeapData::Dict(dict))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a set from the top n stack values.
    pub(super) fn build_set(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let mut set = Set::new();
        for item in items {
            set.add(item, self.heap, self.interns)?;
        }
        let heap_id = self.heap.allocate(HeapData::Set(set))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a slice object from the top 3 stack values.
    ///
    /// Stack: [start, stop, step] -> [slice]
    /// Each value can be None (for default) or an integer.
    pub(super) fn build_slice(&mut self) -> Result<(), RunError> {
        let this = self;

        let step_val = this.pop();
        defer_drop!(step_val, this);
        let stop_val = this.pop();
        defer_drop!(stop_val, this);
        let start_val = this.pop();
        defer_drop!(start_val, this);

        let start = value_to_option_i64(start_val)?;
        let stop = value_to_option_i64(stop_val)?;
        let step = value_to_option_i64(step_val)?;

        let slice = Slice::new(start, stop, step);
        let heap_id = this.heap.allocate(HeapData::Slice(slice))?;
        this.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Extends a list with items from an iterable.
    ///
    /// Stack: [list, iterable] -> [list]
    /// Pops the iterable, extends the list in place, leaves list on stack.
    ///
    /// Uses `HeapGuard` for `list_ref` because it is pushed back on success,
    /// and `defer_drop!` for `iterable` because it is always dropped.
    pub(super) fn list_extend(&mut self) -> Result<(), RunError> {
        let this = self;

        let iterable = this.pop();
        defer_drop!(iterable, this);
        // HeapGuard for list_ref: pushed back on success via into_parts, dropped on error
        let mut list_ref_guard = HeapGuard::new(this.pop(), this);
        let (list_ref, this) = list_ref_guard.as_parts();

        let copied_items: Vec<Value> = match iterable {
            Value::Ref(id) => match this.heap.get(*id) {
                HeapData::List(list) => list.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect(),
                HeapData::Tuple(tuple) => tuple.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect(),
                HeapData::Set(set) => set.storage().iter().map(|v| v.clone_with_heap(this.heap)).collect(),
                HeapData::Dict(dict) => dict.iter().map(|(k, _)| k.clone_with_heap(this.heap)).collect(),
                HeapData::Str(s) => {
                    // Need to allocate strings for each character
                    let chars: Vec<char> = s.as_str().chars().collect();
                    let mut items = Vec::with_capacity(chars.len());
                    for c in chars {
                        items.push(allocate_char(c, this.heap)?);
                    }
                    items
                }
                _ => {
                    let type_ = iterable.py_type(this.heap);
                    return Err(ExcType::type_error_not_iterable(type_));
                }
            },
            Value::InternString(id) => {
                let s = this.interns.get_str(*id);
                let chars: Vec<char> = s.chars().collect();
                let mut items = Vec::with_capacity(chars.len());
                for c in chars {
                    items.push(allocate_char(c, this.heap)?);
                }
                items
            }
            _ => {
                let type_ = iterable.py_type(this.heap);
                return Err(ExcType::type_error_not_iterable(type_));
            }
        };

        // Check if any copied items are refs (for updating contains_refs)
        let has_refs = copied_items.iter().any(|v| matches!(v, Value::Ref(_)));

        // Extend the list
        if let Value::Ref(id) = list_ref
            && let HeapData::List(list) = this.heap.get_mut(*id)
        {
            // Update contains_refs before extending
            if has_refs {
                list.set_contains_refs();
            }
            list.as_vec_mut().extend(copied_items);
        }

        // Mark potential cycle after the mutable borrow ends
        if has_refs {
            this.heap.mark_potential_cycle();
        }

        // Push list_ref back on the stack (don't drop it)
        let (list_ref, this) = list_ref_guard.into_parts();
        this.push(list_ref);
        Ok(())
    }

    /// Converts a list to a tuple.
    ///
    /// Stack: [list] -> [tuple]
    pub(super) fn list_to_tuple(&mut self) -> Result<(), RunError> {
        let this = self;

        let list_ref = this.pop();
        defer_drop!(list_ref, this);

        let copied_items: SmallVec<_> = if let Value::Ref(id) = list_ref {
            if let HeapData::List(list) = this.heap.get(*id) {
                list.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect()
            } else {
                return Err(RunError::internal("ListToTuple: expected list"));
            }
        } else {
            return Err(RunError::internal("ListToTuple: expected list ref"));
        };

        // list_ref is dropped by the guard at scope exit; allocate the tuple
        let value = allocate_tuple(copied_items, this.heap)?;
        this.push(value);
        Ok(())
    }

    /// Merges a mapping into a dict for **kwargs unpacking.
    ///
    /// Stack: [dict, mapping] -> [dict]
    /// Validates that mapping is a dict and that keys are strings.
    ///
    /// Uses `defer_drop!` for `mapping` (always dropped) and `HeapGuard` for
    /// `dict_ref` (pushed back on success, dropped on error).
    pub(super) fn dict_merge(&mut self, func_name_id: u16) -> Result<(), RunError> {
        let this = self;

        let mapping = this.pop();
        defer_drop!(mapping, this);
        // HeapGuard for dict_ref: pushed back on success via into_parts, dropped on error
        let mut dict_ref_guard = HeapGuard::new(this.pop(), this);
        let (dict_ref, this) = dict_ref_guard.as_parts();

        // Get function name for error messages
        let func_name = if func_name_id == 0xFFFF {
            "<unknown>".to_string()
        } else {
            this.interns.get_str(StringId::from_index(func_name_id)).to_string()
        };

        // Check that mapping is a dict (Ref pointing to Dict) and clone key-value pairs
        let copied_items: Vec<(Value, Value)> = if let Value::Ref(id) = mapping {
            if let HeapData::Dict(dict) = this.heap.get(*id) {
                dict.iter()
                    .map(|(k, v)| (k.clone_with_heap(this.heap), v.clone_with_heap(this.heap)))
                    .collect()
            } else {
                let type_name = mapping.py_type(this.heap).to_string();
                return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
            }
        } else {
            let type_name = mapping.py_type(this.heap).to_string();
            return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
        };

        // Merge into the dict, validating string keys
        let dict_id = if let Value::Ref(id) = dict_ref {
            *id
        } else {
            return Err(RunError::internal("DictMerge: expected dict ref"));
        };

        for (key, value) in copied_items {
            // Validate key is a string (InternString or heap-allocated Str)
            let is_string = match &key {
                Value::InternString(_) => true,
                Value::Ref(id) => matches!(this.heap.get(*id), HeapData::Str(_)),
                _ => false,
            };
            if !is_string {
                key.drop_with_heap(this.heap);
                value.drop_with_heap(this.heap);
                return Err(ExcType::type_error_kwargs_nonstring_key());
            }

            // Get the string key for error messages (needed before moving key into closure)
            let key_str = match &key {
                Value::InternString(id) => this.interns.get_str(*id).to_string(),
                Value::Ref(id) => {
                    if let HeapData::Str(s) = this.heap.get(*id) {
                        s.as_str().to_string()
                    } else {
                        "<unknown>".to_string()
                    }
                }
                _ => "<unknown>".to_string(),
            };

            // Use with_entry_mut to avoid borrow conflict: takes data out temporarily
            let result = this.heap.with_entry_mut(dict_id, |heap, data| {
                if let HeapData::Dict(dict) = data {
                    dict.set(key, value, heap, this.interns)
                } else {
                    Err(RunError::internal("DictMerge: entry is not a Dict"))
                }
            });

            // If set returned Some, the key already existed (duplicate kwarg)
            if let Some(old_value) = result? {
                old_value.drop_with_heap(this.heap);
                return Err(ExcType::type_error_multiple_values(&func_name, &key_str));
            }
        }

        // Push dict_ref back on the stack (don't drop it)
        let (dict_ref, this) = dict_ref_guard.into_parts();
        this.push(dict_ref);
        Ok(())
    }

    // ========================================================================
    // Comprehension Building
    // ========================================================================

    /// Appends TOS to list for comprehension.
    ///
    /// Stack: [..., list, iter1, ..., iterN, value] -> [..., list, iter1, ..., iterN]
    /// The `depth` parameter is the number of iterators between the list and the value.
    /// List is at stack position: len - 2 - depth (0-indexed from bottom).
    pub(super) fn list_append(&mut self, depth: usize) -> Result<(), RunError> {
        let value = self.pop();
        let stack_len = self.stack.len();
        let list_pos = stack_len - 1 - depth;

        // Get the list reference
        let Value::Ref(list_id) = self.stack[list_pos] else {
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("ListAppend: expected list ref on stack"));
        };

        // Append to the list using with_entry_mut to handle proper contains_refs tracking
        self.heap.with_entry_mut(list_id, |heap, data| {
            if let HeapData::List(list) = data {
                list.append(heap, value);
                Ok(())
            } else {
                value.drop_with_heap(heap);
                Err(RunError::internal("ListAppend: expected list on heap"))
            }
        })
    }

    /// Adds TOS to set for comprehension.
    ///
    /// Stack: [..., set, iter1, ..., iterN, value] -> [..., set, iter1, ..., iterN]
    /// The `depth` parameter is the number of iterators between the set and the value.
    /// May raise TypeError if value is unhashable.
    pub(super) fn set_add(&mut self, depth: usize) -> Result<(), RunError> {
        let value = self.pop();
        let stack_len = self.stack.len();
        let set_pos = stack_len - 1 - depth;

        // Get the set reference
        let Value::Ref(set_id) = self.stack[set_pos] else {
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("SetAdd: expected set ref on stack"));
        };

        // Add to the set using with_entry_mut to avoid borrow conflicts
        self.heap.with_entry_mut(set_id, |heap, data| {
            if let HeapData::Set(set) = data {
                set.add(value, heap, self.interns)
            } else {
                value.drop_with_heap(heap);
                Err(RunError::internal("SetAdd: expected set on heap"))
            }
        })?;

        Ok(())
    }

    /// Sets dict[key] = value for comprehension.
    ///
    /// Stack: [..., dict, iter1, ..., iterN, key, value] -> [..., dict, iter1, ..., iterN]
    /// The `depth` parameter is the number of iterators between the dict and the key-value pair.
    /// May raise TypeError if key is unhashable.
    pub(super) fn dict_set_item(&mut self, depth: usize) -> Result<(), RunError> {
        let value = self.pop();
        let key = self.pop();
        let stack_len = self.stack.len();
        let dict_pos = stack_len - 1 - depth;

        // Get the dict reference
        let Value::Ref(dict_id) = self.stack[dict_pos] else {
            key.drop_with_heap(self.heap);
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("DictSetItem: expected dict ref on stack"));
        };

        // Set item in the dict using with_entry_mut to avoid borrow conflicts
        let old_value = self.heap.with_entry_mut(dict_id, |heap, data| {
            if let HeapData::Dict(dict) = data {
                dict.set(key, value, heap, self.interns)
            } else {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                Err(RunError::internal("DictSetItem: expected dict on heap"))
            }
        })?;

        // Drop old value if key already existed
        if let Some(old) = old_value {
            old.drop_with_heap(self.heap);
        }

        Ok(())
    }

    // ========================================================================
    // Unpacking
    // ========================================================================

    /// Unpacks a sequence into n values on the stack.
    ///
    /// Supports lists, tuples, and strings. For strings, each character becomes
    /// a separate single-character string.
    pub(super) fn unpack_sequence(&mut self, count: usize) -> Result<(), RunError> {
        let this = self;

        let value = this.pop();
        defer_drop!(value, this);

        // Copy values without incrementing refcounts (avoids borrow conflict with heap.get).
        // For strings, we allocate new string values for each character.
        let items: Vec<Value> = match value {
            // Interned strings (string literals stored inline, not on heap)
            Value::InternString(string_id) => {
                let s = this.interns.get_str(*string_id);
                let str_len = s.chars().count();
                if str_len != count {
                    return Err(unpack_size_error(count, str_len));
                }
                // Allocate each character as a new string
                let mut items = Vec::with_capacity(str_len);
                for c in s.chars() {
                    items.push(allocate_char(c, this.heap)?);
                }
                // Push items in reverse order so first item is on top
                for item in items.into_iter().rev() {
                    this.push(item);
                }
                return Ok(());
            }
            // Heap-allocated sequences
            Value::Ref(heap_id) => {
                match this.heap.get(*heap_id) {
                    HeapData::List(list) => {
                        let list_len = list.len();
                        if list_len != count {
                            return Err(unpack_size_error(count, list_len));
                        }
                        list.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect()
                    }
                    HeapData::Tuple(tuple) => {
                        let tuple_len = tuple.as_slice().len();
                        if tuple_len != count {
                            return Err(unpack_size_error(count, tuple_len));
                        }
                        tuple.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect()
                    }
                    HeapData::Str(s) => {
                        let str_len = s.as_str().chars().count();
                        if str_len != count {
                            return Err(unpack_size_error(count, str_len));
                        }
                        let chars: Vec<char> = s.as_str().chars().collect();
                        let mut items = Vec::with_capacity(chars.len());
                        for c in chars {
                            items.push(allocate_char(c, this.heap)?);
                        }
                        // Push items in reverse order so first item is on top
                        for item in items.into_iter().rev() {
                            this.push(item);
                        }
                        return Ok(());
                    }
                    other => {
                        let type_name = other.py_type(this.heap);
                        return Err(unpack_type_error(type_name));
                    }
                }
            }
            // Non-iterable types
            _ => {
                let type_name = value.py_type(this.heap);
                return Err(unpack_type_error(type_name));
            }
        };

        // Push items in reverse order so first item is on top
        for item in items.into_iter().rev() {
            this.push(item);
        }
        Ok(())
    }

    /// Unpacks a sequence with a starred target.
    ///
    /// `before` is the number of targets before the star, `after` is the number after.
    /// The starred target collects all middle items into a list.
    ///
    /// For example, `first, *rest, last = [1, 2, 3, 4, 5]` has before=1, after=1.
    /// After execution, the stack has: first (top), rest_list, last.
    pub(super) fn unpack_ex(&mut self, before: usize, after: usize) -> Result<(), RunError> {
        let this = self;

        let value = this.pop();
        defer_drop_mut!(value, this);

        let min_items = before + after;

        // Extract items from the sequence
        let items: Vec<Value> = match value {
            Value::InternString(string_id) => {
                let s = this.interns.get_str(*string_id);
                // Collect chars once to avoid double iteration over UTF-8 data
                let chars: Vec<char> = s.chars().collect();
                if chars.len() < min_items {
                    return Err(unpack_ex_too_few_error(min_items, chars.len()));
                }
                // Allocate each character as a new string
                let mut items = Vec::with_capacity(chars.len());
                for c in chars {
                    items.push(allocate_char(c, this.heap)?);
                }
                items
            }
            Value::Ref(heap_id) => {
                match this.heap.get(*heap_id) {
                    HeapData::List(list) => {
                        let list_len = list.len();
                        if list_len < min_items {
                            return Err(unpack_ex_too_few_error(min_items, list_len));
                        }
                        list.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect()
                    }
                    HeapData::Tuple(tuple) => {
                        let tuple_len = tuple.as_slice().len();
                        if tuple_len < min_items {
                            return Err(unpack_ex_too_few_error(min_items, tuple_len));
                        }
                        tuple.as_slice().iter().map(|v| v.clone_with_heap(this.heap)).collect()
                    }
                    HeapData::Str(s) => {
                        // Collect chars once to avoid double iteration over UTF-8 data
                        let chars: Vec<char> = s.as_str().chars().collect();
                        if chars.len() < min_items {
                            return Err(unpack_ex_too_few_error(min_items, chars.len()));
                        }
                        let mut items = Vec::with_capacity(chars.len());
                        for c in chars {
                            items.push(allocate_char(c, this.heap)?);
                        }
                        items
                    }
                    other => {
                        let type_name = other.py_type(this.heap);
                        return Err(unpack_type_error(type_name));
                    }
                }
            }
            _ => {
                let type_name = value.py_type(this.heap);
                return Err(unpack_type_error(type_name));
            }
        };

        this.push_unpack_ex_results(items, before, after)
    }

    /// Helper to push unpacked items with starred target onto the stack.
    ///
    /// Takes a slice of items and creates the middle list.
    fn push_unpack_ex_results(&mut self, items: Vec<Value>, before: usize, after: usize) -> Result<(), RunError> {
        let this = self;

        defer_drop_mut!(items, this);

        // Items get pushed onto the stack backwards, so a lot of .rev() calls

        for item in items.drain(items.len() - after..).rev() {
            this.push(item);
        }

        // Middle items as a list (starred target)
        let middle_list: Vec<Value> = items.drain(before..).collect();
        let list_id = this.heap.allocate(HeapData::List(List::new(middle_list)))?;
        this.push(Value::Ref(list_id));

        // Before items
        for item in items.drain(..).rev() {
            this.push(item);
        }

        Ok(())
    }
}

/// Creates the ValueError for star unpacking when there are too few values.
fn unpack_ex_too_few_error(min_needed: usize, actual: usize) -> RunError {
    let message = format!("not enough values to unpack (expected at least {min_needed}, got {actual})");
    SimpleException::new_msg(ExcType::ValueError, message).into()
}

/// Creates the appropriate ValueError for unpacking size mismatches.
///
/// Python uses different messages depending on whether there are too few or too many values:
/// - Too few: "not enough values to unpack (expected X, got Y)"
/// - Too many: "too many values to unpack (expected X, got Y)"
fn unpack_size_error(expected: usize, actual: usize) -> RunError {
    let message = if actual < expected {
        format!("not enough values to unpack (expected {expected}, got {actual})")
    } else {
        format!("too many values to unpack (expected {expected}, got {actual})")
    };
    SimpleException::new_msg(ExcType::ValueError, message).into()
}

/// Creates a TypeError for attempting to unpack a non-iterable type.
fn unpack_type_error(type_name: Type) -> RunError {
    SimpleException::new_msg(
        ExcType::TypeError,
        format!("cannot unpack non-iterable {type_name} object"),
    )
    .into()
}
