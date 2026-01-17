//! Collection building and unpacking helpers for the VM.

use super::VM;
use crate::{
    exception_private::{ExcType, RunError, SimpleException},
    heap::HeapData,
    intern::StringId,
    io::PrintWriter,
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Set, Tuple, Type},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Builds a list from the top n stack values.
    pub(super) fn build_list(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let list = List::new(items);
        let heap_id = self.heap.allocate(HeapData::List(list))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a tuple from the top n stack values.
    pub(super) fn build_tuple(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let tuple = Tuple::new(items);
        let heap_id = self.heap.allocate(HeapData::Tuple(tuple))?;
        self.push(Value::Ref(heap_id));
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

    /// Extends a list with items from an iterable.
    ///
    /// Stack: [list, iterable] -> [list]
    /// Pops the iterable, extends the list in place, leaves list on stack.
    pub(super) fn list_extend(&mut self) -> Result<(), RunError> {
        let iterable = self.pop();
        let list_ref = self.pop();

        // Two-phase approach to avoid borrow conflicts:
        // Phase 1: Copy items without refcount changes
        let copied_items: Vec<Value> = match &iterable {
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::List(list) => list.as_vec().iter().map(Value::copy_for_extend).collect(),
                HeapData::Tuple(tuple) => tuple.as_vec().iter().map(Value::copy_for_extend).collect(),
                HeapData::Set(set) => set.storage().iter().map(Value::copy_for_extend).collect(),
                HeapData::Dict(dict) => dict.iter().map(|(k, _)| Value::copy_for_extend(k)).collect(),
                HeapData::Str(s) => {
                    // Need to allocate strings for each character
                    let chars: Vec<char> = s.as_str().chars().collect();
                    let mut items = Vec::with_capacity(chars.len());
                    for c in chars {
                        items.push(self.heap.allocate_char(c)?);
                    }
                    items
                }
                _ => {
                    let type_ = iterable.py_type(self.heap);
                    iterable.drop_with_heap(self.heap);
                    list_ref.drop_with_heap(self.heap);
                    return Err(ExcType::type_error_not_iterable(type_));
                }
            },
            Value::InternString(id) => {
                let s = self.interns.get_str(*id);
                let chars: Vec<char> = s.chars().collect();
                let mut items = Vec::with_capacity(chars.len());
                for c in chars {
                    items.push(self.heap.allocate_char(c)?);
                }
                items
            }
            _ => {
                let type_ = iterable.py_type(self.heap);
                iterable.drop_with_heap(self.heap);
                list_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_not_iterable(type_));
            }
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for item in &copied_items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        // Extend the list
        if let Value::Ref(id) = &list_ref
            && let HeapData::List(list) = self.heap.get_mut(*id)
        {
            list.as_vec_mut().extend(copied_items);
        }

        iterable.drop_with_heap(self.heap);
        self.push(list_ref);
        Ok(())
    }

    /// Converts a list to a tuple.
    ///
    /// Stack: [list] -> [tuple]
    pub(super) fn list_to_tuple(&mut self) -> Result<(), RunError> {
        let list_ref = self.pop();

        // Phase 1: Copy items without refcount changes
        let copied_items: Vec<Value> = if let Value::Ref(id) = &list_ref {
            if let HeapData::List(list) = self.heap.get(*id) {
                list.as_vec().iter().map(Value::copy_for_extend).collect()
            } else {
                return Err(RunError::internal("ListToTuple: expected list"));
            }
        } else {
            return Err(RunError::internal("ListToTuple: expected list ref"));
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for item in &copied_items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        list_ref.drop_with_heap(self.heap);

        let tuple = Tuple::new(copied_items);
        let heap_id = self.heap.allocate(HeapData::Tuple(tuple))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Merges a mapping into a dict for **kwargs unpacking.
    ///
    /// Stack: [dict, mapping] -> [dict]
    /// Validates that mapping is a dict and that keys are strings.
    pub(super) fn dict_merge(&mut self, func_name_id: u16) -> Result<(), RunError> {
        let mapping = self.pop();
        let dict_ref = self.pop();

        // Get function name for error messages
        let func_name = if func_name_id == 0xFFFF {
            "<unknown>".to_string()
        } else {
            self.interns.get_str(StringId::from_index(func_name_id)).to_string()
        };

        // Two-phase approach: copy items first, then inc refcounts
        // Phase 1: Copy key-value pairs without refcount changes
        // Check that mapping is a dict (Ref pointing to Dict)
        let copied_items: Vec<(Value, Value)> = if let Value::Ref(id) = &mapping {
            if let HeapData::Dict(dict) = self.heap.get(*id) {
                dict.iter()
                    .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
                    .collect()
            } else {
                let type_name = mapping.py_type(self.heap).to_string();
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
            }
        } else {
            let type_name = mapping.py_type(self.heap).to_string();
            mapping.drop_with_heap(self.heap);
            dict_ref.drop_with_heap(self.heap);
            return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for (key, value) in &copied_items {
            if let Value::Ref(id) = key {
                self.heap.inc_ref(*id);
            }
            if let Value::Ref(id) = value {
                self.heap.inc_ref(*id);
            }
        }

        // Merge into the dict, validating string keys
        let dict_id = if let Value::Ref(id) = &dict_ref {
            *id
        } else {
            mapping.drop_with_heap(self.heap);
            dict_ref.drop_with_heap(self.heap);
            return Err(RunError::internal("DictMerge: expected dict ref"));
        };

        for (key, value) in copied_items {
            // Validate key is a string (InternString or heap-allocated Str)
            let is_string = match &key {
                Value::InternString(_) => true,
                Value::Ref(id) => matches!(self.heap.get(*id), HeapData::Str(_)),
                _ => false,
            };
            if !is_string {
                key.drop_with_heap(self.heap);
                value.drop_with_heap(self.heap);
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_kwargs_nonstring_key());
            }

            // Get the string key for error messages (needed before moving key into closure)
            let key_str = match &key {
                Value::InternString(id) => self.interns.get_str(*id).to_string(),
                Value::Ref(id) => {
                    if let HeapData::Str(s) = self.heap.get(*id) {
                        s.as_str().to_string()
                    } else {
                        "<unknown>".to_string()
                    }
                }
                _ => "<unknown>".to_string(),
            };

            // Use with_entry_mut to avoid borrow conflict: takes data out temporarily
            let result = self.heap.with_entry_mut(dict_id, |heap, data| {
                if let HeapData::Dict(dict) = data {
                    dict.set(key, value, heap, self.interns)
                } else {
                    Err(RunError::internal("DictMerge: entry is not a Dict"))
                }
            });

            // If set returned Some, the key already existed (duplicate kwarg)
            if let Some(old_value) = result? {
                old_value.drop_with_heap(self.heap);
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_multiple_values(&func_name, &key_str));
            }
        }

        mapping.drop_with_heap(self.heap);
        self.push(dict_ref);
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
        let value = self.pop();

        // Copy values without incrementing refcounts (avoids borrow conflict with heap.get).
        // For strings, we allocate new string values for each character.
        let items: Vec<Value> = match &value {
            // Interned strings (string literals stored inline, not on heap)
            Value::InternString(string_id) => {
                let s = self.interns.get_str(*string_id);
                let str_len = s.chars().count();
                if str_len != count {
                    return Err(unpack_size_error(count, str_len));
                }
                // Allocate each character as a new string
                let mut items = Vec::with_capacity(str_len);
                for c in s.chars() {
                    items.push(self.heap.allocate_char(c)?);
                }
                // Push items in reverse order so first item is on top
                for item in items.into_iter().rev() {
                    self.push(item);
                }
                return Ok(());
            }
            // Heap-allocated sequences
            Value::Ref(heap_id) => match self.heap.get(*heap_id) {
                HeapData::List(list) => {
                    let list_len = list.len();
                    if list_len != count {
                        value.drop_with_heap(self.heap);
                        return Err(unpack_size_error(count, list_len));
                    }
                    list.as_vec().iter().map(Value::copy_for_extend).collect()
                }
                HeapData::Tuple(tuple) => {
                    let tuple_len = tuple.as_vec().len();
                    if tuple_len != count {
                        value.drop_with_heap(self.heap);
                        return Err(unpack_size_error(count, tuple_len));
                    }
                    tuple.as_vec().iter().map(Value::copy_for_extend).collect()
                }
                HeapData::Str(s) => {
                    let str_len = s.as_str().chars().count();
                    if str_len != count {
                        value.drop_with_heap(self.heap);
                        return Err(unpack_size_error(count, str_len));
                    }
                    // Collect characters first to avoid borrow conflict with heap
                    let chars: Vec<char> = s.as_str().chars().collect();
                    // Drop the original string value before allocating new ones
                    value.drop_with_heap(self.heap);
                    // Allocate each character as a new string
                    let mut items = Vec::with_capacity(chars.len());
                    for c in chars {
                        items.push(self.heap.allocate_char(c)?);
                    }
                    // Push items in reverse order so first item is on top
                    for item in items.into_iter().rev() {
                        self.push(item);
                    }
                    return Ok(());
                }
                other => {
                    let type_name = other.py_type(self.heap);
                    value.drop_with_heap(self.heap);
                    return Err(unpack_type_error(type_name));
                }
            },
            // Non-iterable types
            _ => {
                let type_name = value.py_type(self.heap);
                value.drop_with_heap(self.heap);
                return Err(unpack_type_error(type_name));
            }
        };

        // IMPORTANT: Increment refcounts BEFORE dropping the container.
        // The container holds references to its items. If we drop the container first,
        // it decrements the item refcounts, potentially freeing them before we can
        // increment the refcounts for our copies.
        for item in &items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        // Now safe to drop the original container
        value.drop_with_heap(self.heap);

        // Push items in reverse order so first item is on top
        for item in items.into_iter().rev() {
            self.push(item);
        }
        Ok(())
    }
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
