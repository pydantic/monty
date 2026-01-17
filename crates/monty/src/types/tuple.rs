/// Python tuple type, wrapping a `Vec<Value>`.
///
/// This type provides Python tuple semantics. Tuples are immutable sequences
/// that can contain any Python object. Like lists, tuples properly handle
/// reference counting for heap-allocated values.
use std::fmt::Write;

use ahash::AHashSet;

use super::{PyTrait, list::repr_sequence_fmt};
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    types::Type,
    value::Value,
};

/// Python tuple value stored on the heap.
///
/// Wraps a `Vec<Value>` and provides Python-compatible tuple operations.
/// Unlike lists, tuples are conceptually immutable (though this is not
/// enforced at the type level for internal operations).
///
/// # Reference Counting
/// When a tuple is freed, all contained heap references have their refcounts
/// decremented via `push_stack_ids`.
///
/// # GC Optimization
/// The `contains_refs` flag tracks whether the tuple contains any `Value::Ref` items.
/// This allows `collect_child_ids` and `py_dec_ref_ids` to skip iteration when the
/// tuple contains only primitive values (ints, bools, None, etc.).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Tuple {
    items: Vec<Value>,
    /// True if any item in the tuple is a `Value::Ref`. Set at creation time
    /// since tuples are immutable.
    contains_refs: bool,
}

impl Tuple {
    /// Creates a new tuple from a vector of values.
    ///
    /// Automatically computes the `contains_refs` flag by checking if any value
    /// is a `Value::Ref`. Since tuples are immutable, this flag never changes.
    ///
    /// Note: This does NOT increment reference counts - the caller must
    /// ensure refcounts are properly managed.
    #[must_use]
    pub fn new(vec: Vec<Value>) -> Self {
        let contains_refs = vec.iter().any(|v| matches!(v, Value::Ref(_)));
        Self {
            items: vec,
            contains_refs,
        }
    }

    /// Returns a reference to the underlying vector.
    #[must_use]
    pub fn as_vec(&self) -> &Vec<Value> {
        &self.items
    }

    /// Returns whether the tuple contains any heap references.
    ///
    /// When false, `collect_child_ids` and `py_dec_ref_ids` can skip iteration.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// Creates a tuple from the `tuple()` constructor call.
    ///
    /// - `tuple()` with no args returns an empty tuple
    /// - `tuple(iterable)` creates a tuple from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("tuple")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Tuple(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let mut iter = ForIterator::new(v, heap, interns)?;
                let items = iter.collect(heap, interns)?;
                iter.drop_with_heap(heap);
                let heap_id = heap.allocate(HeapData::Tuple(Self::new(items)))?;
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl FromIterator<Value> for Tuple {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

impl From<Tuple> for Vec<Value> {
    fn from(tuple: Tuple) -> Self {
        tuple.items
    }
}

impl PyTrait for Tuple {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Tuple
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.items.len() * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.items.len())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Extract integer index from key, returning TypeError if not an int
        let index = match key {
            Value::Int(i) => *i,
            _ => return Err(ExcType::type_error_indices(Type::Tuple, key.py_type(heap))),
        };

        // Convert to usize, handling negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.items.len()).expect("tuple length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::tuple_index_error());
        }

        // Return clone of the item with proper refcount increment
        // Safety: normalized_index is validated to be in [0, len) above
        let idx = usize::try_from(normalized_index).expect("tuple index validated non-negative");
        Ok(self.items[idx].clone_with_heap(heap))
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.items.len() != other.items.len() {
            return false;
        }
        for (i1, i2) in self.items.iter().zip(&other.items) {
            if !i1.py_eq(i2, heap, interns) {
                return false;
            }
        }
        true
    }

    /// Pushes all heap IDs contained in this tuple onto the stack.
    ///
    /// Called during garbage collection to decrement refcounts of nested values.
    /// When `ref-count-panic` is enabled, also marks all Values as Dereferenced.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - GC optimization for tuples of primitives
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                obj.dec_ref_forget();
            }
        }
    }

    // py_call_attr uses default implementation which returns AttributeError

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        repr_sequence_fmt('(', ')', &self.items, f, heap, heap_ids, interns)
    }
}
