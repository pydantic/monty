use std::fmt::Write;

use ahash::AHashSet;

use super::PyTrait;
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, attr},
    resource::ResourceTracker,
    types::Type,
    value::{Attr, Value},
};

/// Python list type, wrapping a Vec of Values.
///
/// This type provides Python list semantics including dynamic growth,
/// reference counting for heap values, and standard list methods like
/// append and insert.
///
/// # Reference Counting
/// When values are added to the list (via append, insert, etc.), their
/// reference counts are incremented if they are heap-allocated (Ref variants).
/// This ensures values remain valid while referenced by the list.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct List(Vec<Value>);

impl List {
    /// Creates a new list from a vector of values.
    ///
    /// Note: This does NOT increment reference counts - the caller must
    /// ensure refcounts are properly managed.
    #[must_use]
    pub fn new(vec: Vec<Value>) -> Self {
        Self(vec)
    }

    /// Returns a reference to the underlying vector.
    #[must_use]
    pub fn as_vec(&self) -> &Vec<Value> {
        &self.0
    }

    /// Returns a mutable reference to the underlying vector.
    ///
    /// # Safety Considerations
    /// Be careful when mutating the vector directly - you must manually
    /// manage reference counts for any heap values you add or remove.
    pub fn as_vec_mut(&mut self) -> &mut Vec<Value> {
        &mut self.0
    }

    /// Returns the number of elements in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Appends an element to the end of the list.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// Returns `Value::None`, matching Python's behavior where `list.append()` returns None.
    pub fn append(&mut self, _heap: &mut Heap<impl ResourceTracker>, item: Value) {
        // Ownership transfer - refcount was already handled by caller
        self.0.push(item);
    }

    /// Inserts an element at the specified index.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented.
    ///
    /// # Arguments
    /// * `index` - The position to insert at (0-based). If index >= len(),
    ///   the item is appended to the end (matching Python semantics).
    ///
    /// Returns `Value::None`, matching Python's behavior where `list.insert()` returns None.
    pub fn insert(&mut self, _heap: &mut Heap<impl ResourceTracker>, index: usize, item: Value) {
        // Ownership transfer - refcount was already handled by caller
        // Python's insert() appends if index is out of bounds
        if index >= self.0.len() {
            self.0.push(item);
        } else {
            self.0.insert(index, item);
        }
    }

    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::List(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let mut iter = ForIterator::new(v, heap, interns)?;
                let items = iter.collect(heap, interns)?;
                iter.drop_with_heap(heap);
                let heap_id = heap.allocate(HeapData::List(Self::new(items)))?;
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl From<List> for Vec<Value> {
    fn from(list: List) -> Self {
        list.0
    }
}

impl PyTrait for List {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::List
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.len() * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.0.len())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Extract integer index from key, returning TypeError if not an int
        let index = match key {
            Value::Int(i) => *i,
            _ => return Err(ExcType::type_error_indices(Type::List, key.py_type(heap))),
        };

        // Convert to usize, handling negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.0.len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_index_error());
        }

        // Return clone of the item with proper refcount increment
        // Safety: normalized_index is validated to be in [0, len) above
        let idx = usize::try_from(normalized_index).expect("list index validated non-negative");
        Ok(self.0[idx].clone_with_heap(heap))
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.0.len() != other.0.len() {
            return false;
        }
        for (i1, i2) in self.0.iter().zip(&other.0) {
            if !i1.py_eq(i2, heap, interns) {
                return false;
            }
        }
        true
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for obj in &mut self.0 {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                obj.dec_ref_forget();
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.0.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        repr_sequence_fmt('[', ']', &self.0, f, heap, heap_ids, interns)
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // Clone both lists' contents with proper refcounting
        let mut result: Vec<Value> = self.0.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        let other_cloned: Vec<Value> = other.0.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        result.extend(other_cloned);
        let id = heap.allocate(HeapData::List(Self::new(result)))?;
        Ok(Some(Value::Ref(id)))
    }

    fn py_iadd(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        self_id: Option<HeapId>,
        _interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        // Extract the value ID first, keeping `other` around to drop later
        let Value::Ref(other_id) = &other else { return Ok(false) };

        if Some(*other_id) == self_id {
            // Self-extend: clone our own items with proper refcounting
            let items = self.0.iter().map(|obj| obj.clone_with_heap(heap)).collect::<Vec<_>>();
            self.0.extend(items);
        } else {
            // Get items from other list using iadd_extend_from_heap helper
            // This handles the borrow checker limitations with lifetime propagation
            if !heap.iadd_extend_list(*other_id, &mut self.0) {
                return Ok(false);
            }
        }

        // Drop the other value - we've extracted its contents and are done with the temporary reference
        other.drop_with_heap(heap);
        Ok(true)
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(attr_id) = attr.string_id() else {
            args.drop_with_heap(heap);
            return Err(ExcType::attribute_error(Type::List, attr.as_str(interns)));
        };

        match attr_id {
            attr::APPEND => {
                let item = args.get_one_arg("list.append")?;
                self.append(heap, item);
                Ok(Value::None)
            }
            attr::INSERT => {
                let (index_obj, item) = args.get_two_args("insert")?;
                // Python's insert() handles negative indices by adding len
                // If still negative after adding len, clamps to 0
                // If >= len, appends to end
                let index_result = index_obj.as_int(heap);
                // Drop index_obj before propagating error - it could be a Ref (e.g., dict)
                index_obj.drop_with_heap(heap);
                let index_i64 = match index_result {
                    Ok(i) => i,
                    Err(e) => {
                        item.drop_with_heap(heap);
                        return Err(e);
                    }
                };
                let len = self.0.len();
                let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
                let index = if index_i64 < 0 {
                    // Negative index: add length, clamp to 0 if still negative
                    let adjusted = index_i64 + len_i64;
                    if adjusted < 0 {
                        0
                    } else {
                        usize::try_from(adjusted).expect("adjusted index fits in usize")
                    }
                } else {
                    // Positive index: clamp to len if too large
                    usize::try_from(index_i64).unwrap_or(len)
                };
                self.insert(heap, index, item);
                Ok(Value::None)
            }
            _ => {
                args.drop_with_heap(heap);
                Err(ExcType::attribute_error(Type::List, attr.as_str(interns)))
            }
        }
    }
}

/// Writes a formatted sequence of values to a formatter.
///
/// This helper function is used to implement `__repr__` for sequence types like
/// lists and tuples. It writes items as comma-separated repr interns.
///
/// # Arguments
/// * `start` - The opening character (e.g., '[' for lists, '(' for tuples)
/// * `end` - The closing character (e.g., ']' for lists, ')' for tuples)
/// * `items` - The slice of values to format
/// * `f` - The formatter to write to
/// * `heap` - The heap for resolving value references
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
/// * `interns` - The interned strings table for looking up string/bytes literals
pub(crate) fn repr_sequence_fmt(
    start: char,
    end: char,
    items: &[Value],
    f: &mut impl Write,
    heap: &Heap<impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
    interns: &Interns,
) -> std::fmt::Result {
    f.write_char(start)?;
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        first.py_repr_fmt(f, heap, heap_ids, interns)?;
        for item in iter {
            f.write_str(", ")?;
            item.py_repr_fmt(f, heap, heap_ids, interns)?;
        }
    }
    f.write_char(end)
}
