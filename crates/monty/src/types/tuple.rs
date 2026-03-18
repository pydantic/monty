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
use std::cmp::Ordering;
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
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId, HeapItem},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
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
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("tuple", vm.heap)?;
        match value {
            None => {
                // Use empty tuple singleton
                Ok(vm.heap.get_empty_tuple())
            }
            Some(v) => {
                let items = MontyIter::new(v, vm)?.collect(vm)?;
                Ok(allocate_tuple(items, vm.heap)?)
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

    fn py_len(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.items.len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
        let heap = &mut *vm.heap;
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = heap.get(*id)
        {
            let (start, stop, step) = slice
                .indices(self.items.len())
                .map_err(|()| ExcType::value_error_slice_step_zero())?;

            let items = get_slice_items(&self.items, start, stop, step, heap)?;
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

    fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if self.items.len() != other.items.len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        for (i1, i2) in self.items.iter().zip(&other.items) {
            vm.heap.check_time()?;
            if !i1.py_eq(i2, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Lexicographic comparison for tuples.
    ///
    /// Compares element-by-element left-to-right. The first non-equal pair
    /// determines the result. If all compared elements are equal, the shorter
    /// tuple is considered less than the longer one — matching Python semantics:
    /// `(1, 2) < (1, 2, 3)` is `True`.
    ///
    /// Returns `None` if any element pair is incomparable (e.g. `int` vs `str`).
    fn py_cmp(
        &self,
        other: &Self,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Ordering>, ResourceError> {
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        for (av, bv) in self.items.iter().zip(&other.items) {
            vm.heap.check_time()?;
            match av.py_cmp(bv, vm)? {
                Some(Ordering::Equal) => {}
                Some(ord) => return Ok(Some(ord)),
                None => {
                    // py_cmp returned None — the elements don't support ordering.
                    // CPython checks __eq__ first and only calls __lt__ for non-equal
                    // pairs, so equal-but-unorderable elements (e.g. None == None)
                    // should be treated as equal and not block comparison.
                    if !av.py_eq(bv, vm)? {
                        return Ok(None);
                    }
                }
            }
        }
        // All compared elements equal — shorter tuple is less
        Ok(Some(self.items.len().cmp(&other.items.len())))
    }

    fn py_add(
        &self,
        other: &Self,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        let heap = &mut *vm.heap;
        // Clone both tuples' contents with proper refcounting
        let mut result: TupleVec = self.items.iter().map(|obj| obj.clone_with_heap(heap)).collect();
        let other_cloned = other.items.iter().map(|obj| obj.clone_with_heap(heap));
        result.extend(other_cloned);
        Ok(Some(allocate_tuple(result, heap)?))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        match attr.static_string() {
            Some(StaticStrings::Index) => tuple_index(self, args, vm).map(CallResult::Value),
            Some(StaticStrings::Count) => tuple_count(self, args, vm).map(CallResult::Value),
            _ => {
                args.drop_with_heap(vm);
                Err(ExcType::attribute_error(Type::Tuple, attr.as_str(vm.interns)))
            }
        }
    }

    fn py_bool(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        !self.items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        repr_sequence_fmt('(', ')', &self.items, f, vm, heap_ids)
    }
}

impl HeapItem for Tuple {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.items.len() * std::mem::size_of::<Value>()
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
}

/// Implements Python's `tuple.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn tuple_index(tuple: &Tuple, args: ArgValues, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let pos_args = args.into_pos_only("tuple.index", vm.heap)?;
    defer_drop!(pos_args, vm);

    let len = tuple.as_slice().len();
    let (value, start, end) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("tuple.index", 1, 0)),
        [value] => (value, 0, len),
        [value, start_arg] => {
            let start = normalize_tuple_index(start_arg.as_int(vm.heap)?, len);
            (value, start, len)
        }
        [value, start_arg, end_arg] => {
            let start = normalize_tuple_index(start_arg.as_int(vm.heap)?, len);
            let end = normalize_tuple_index(end_arg.as_int(vm.heap)?, len).max(start);
            (value, start, end)
        }
        other => return Err(ExcType::type_error_at_most("tuple.index", 3, other.len())),
    };

    // Search for the value in the specified range
    for (i, item) in tuple.as_slice()[start..end].iter().enumerate() {
        if value.py_eq(item, vm)? {
            let idx = i64::try_from(start + i).expect("index exceeds i64::MAX");
            return Ok(Value::Int(idx));
        }
    }

    Err(ExcType::value_error_not_in_tuple())
}

/// Implements Python's `tuple.count(value)` method.
///
/// Returns the number of occurrences of value in the tuple.
fn tuple_count(tuple: &Tuple, args: ArgValues, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let value = args.get_one_arg("tuple.count", vm.heap)?;
    defer_drop!(value, vm);

    let mut count = 0usize;
    for item in tuple.as_slice() {
        if value.py_eq(item, vm)? {
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
