/// Python tuple type using `SmallVec` for inline storage of small tuples.
///
/// This type provides Python tuple semantics. Tuples are immutable sequences
/// that can contain any Python object. Like lists, tuples properly handle
/// reference counting for heap-allocated values.
///
/// # Optimization
/// Uses `SmallVec<[Value; 2]>` to store up to 2 elements inline without heap
/// allocation. This benefits common cases like 2-tuples from `enumerate()`,
/// `dict.items()`, and function return values.
///
/// # Implemented Methods
/// - `index(value[, start[, end]])` - Find first index of value
/// - `count(value)` - Count occurrences
///
/// All tuple methods from Python's builtins are implemented.
use std::fmt::Write;

use ahash::AHashSet;
use smallvec::SmallVec;

/// Inline capacity for small tuples. Tuples with 2 or fewer elements avoid
/// heap allocation for the items storage.
const TUPLE_INLINE_CAPACITY: usize = 3;

/// Storage type for tuple items. Uses SmallVec to inline small tuples.
pub(crate) type TupleVec = SmallVec<[Value; TUPLE_INLINE_CAPACITY]>;

use super::{
    MontyIter, PyTrait,
    list::{get_slice_items, repr_sequence_fmt},
};
use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::Type,
    value::{EitherStr, Value},
};

/// Python tuple value stored on the heap.
///
/// Uses `SmallVec<[Value; 3]>` internally to avoid separate heap allocation
/// for tuples with 3 or fewer elements. This is a significant optimization
/// since small tuples are very common (enumerate, dict items, returns, etc.).
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
    items: TupleVec,
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
    /// For tuples with 3 or fewer elements, the items are stored inline in the
    /// SmallVec without additional heap allocation.
    ///
    /// Note: This does NOT increment reference counts - the caller must
    /// ensure refcounts are properly managed.
    #[must_use]
    fn new(items: TupleVec) -> Self {
        let contains_refs = items.iter().any(|v| matches!(v, Value::Ref(_)));
        Self { items, contains_refs }
    }

    /// Returns a reference to the underlying SmallVec.
    #[must_use]
    pub fn as_slice(&self) -> &[Value] {
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
    /// - `tuple()` with no args returns an empty tuple (singleton)
    /// - `tuple(iterable)` creates a tuple from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("tuple", heap)?;
        match value {
            None => {
                // Use empty tuple singleton
                Ok(heap.get_empty_tuple())
            }
            Some(v) => {
                let mut iter = MontyIter::new(v, heap, interns)?;
                let items = iter.collect(heap, interns)?;
                iter.drop_with_heap(heap);
                Ok(allocate_tuple(items, heap)?)
            }
        }
    }
}

impl From<Tuple> for Vec<Value> {
    fn from(tuple: Tuple) -> Self {
        tuple.items.into_vec()
    }
}

impl From<Tuple> for TupleVec {
    fn from(tuple: Tuple) -> Self {
        tuple.items
    }
}

/// Allocates a tuple, using the empty tuple singleton when appropriate.
///
/// This is the preferred way to allocate tuples as it provides:
/// - Empty tuple interning: `() is ()` returns `True`
/// - SmallVec optimization for small tuples (≤3 elements)
///
/// # Example Usage
/// ```ignore
/// // Empty tuple - returns singleton
/// let empty = allocate_tuple(Vec::new(), heap)?;
///
/// // Small tuple - stored inline in SmallVec
/// let pair = allocate_tuple(vec![Value::Int(1), Value::Int(2)], heap)?;
/// ```
pub fn allocate_tuple(
    items: SmallVec<[Value; TUPLE_INLINE_CAPACITY]>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Value, crate::resource::ResourceError> {
    if items.is_empty() {
        Ok(heap.get_empty_tuple())
    } else {
        // Allocate a new tuple (SmallVec will inline if ≤3 elements)
        let heap_id = heap.allocate(HeapData::Tuple(Tuple::new(items)))?;
        Ok(Value::Ref(heap_id))
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
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = heap.get(*id)
        {
            let (start, stop, step) = slice
                .indices(self.items.len())
                .map_err(|()| ExcType::value_error_slice_step_zero())?;

            let items = get_slice_items(&self.items, start, stop, step, heap);
            return Ok(allocate_tuple(items.into(), heap)?);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(heap, Type::Tuple)?;

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

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        if self.items.len() != other.items.len() {
            return Ok(false);
        }
        guard.increase_err()?;

        for (i1, i2) in self.items.iter().zip(&other.items) {
            if !i1.py_eq(i2, heap, guard, interns)? {
                guard.decrease();
                return Ok(false);
            }
        }
        guard.decrease();
        Ok(true)
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        // Clone both tuples' contents with proper refcounting
        let mut result: TupleVec = self.items.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        let other_cloned = other.items.iter().map(|obj| obj.clone_with_heap(heap));
        result.extend(other_cloned);
        Ok(Some(allocate_tuple(result, heap)?))
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

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match attr.static_string() {
            Some(StaticStrings::Index) => tuple_index(self, args, heap, interns),
            Some(StaticStrings::Count) => tuple_count(self, args, heap, interns),
            _ => {
                args.drop_with_heap(heap);
                Err(ExcType::attribute_error(Type::Tuple, attr.as_str(interns)))
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> std::fmt::Result {
        repr_sequence_fmt('(', ')', &self.items, f, heap, heap_ids, guard, interns)
    }
}

/// Implements Python's `tuple.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn tuple_index(
    tuple: &Tuple,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let pos_args = args.into_pos_only("tuple.index", heap)?;
    defer_drop!(pos_args, heap);

    let len = tuple.as_slice().len();
    let (value, start, end) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("tuple.index", 1, 0)),
        [value] => (value, 0, len),
        [value, start_arg] => {
            let start = normalize_tuple_index(start_arg.as_int(heap)?, len);
            (value, start, len)
        }
        [value, start_arg, end_arg] => {
            let start = normalize_tuple_index(start_arg.as_int(heap)?, len);
            let end = normalize_tuple_index(end_arg.as_int(heap)?, len).max(start);
            (value, start, end)
        }
        other => return Err(ExcType::type_error_at_most("tuple.index", 3, other.len())),
    };

    let mut guard = DepthGuard::default();
    // Search for the value in the specified range
    for (i, item) in tuple.as_slice()[start..end].iter().enumerate() {
        if value.py_eq(item, heap, &mut guard, interns)? {
            let idx = i64::try_from(start + i).expect("index exceeds i64::MAX");
            return Ok(Value::Int(idx));
        }
    }

    Err(ExcType::value_error_not_in_tuple())
}

/// Implements Python's `tuple.count(value)` method.
///
/// Returns the number of occurrences of value in the tuple.
fn tuple_count(
    tuple: &Tuple,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let value = args.get_one_arg("tuple.count", heap)?;
    defer_drop!(value, heap);

    let mut guard = DepthGuard::default();
    let mut count = 0usize;
    for item in tuple.as_slice() {
        if value.py_eq(item, heap, &mut guard, interns)? {
            count += 1;
        }
    }

    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}

/// Normalizes a Python-style tuple index to a valid index in range [0, len].
fn normalize_tuple_index(index: i64, len: usize) -> usize {
    if index < 0 {
        let abs_index = usize::try_from(-index).unwrap_or(usize::MAX);
        len.saturating_sub(abs_index)
    } else {
        usize::try_from(index).unwrap_or(len).min(len)
    }
}
