//! Python range type implementation.
//!
//! Provides a range object that supports iteration over a sequence of integers
//! with configurable start, stop, and step values.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;
use num_integer::div_ceil;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    resource::ResourceTracker,
    types::{PyTrait, Type},
    value::Value,
};

/// Python range object representing an immutable sequence of integers.
///
/// Supports three forms of construction:
/// - `range(stop)` - integers from 0 to stop-1
/// - `range(start, stop)` - integers from start to stop-1
/// - `range(start, stop, step)` - integers from start, incrementing by step
///
/// The range is computed lazily during iteration, not stored as a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Range {
    /// The starting value (inclusive). Defaults to 0.
    pub start: i64,
    /// The ending value (exclusive).
    pub stop: i64,
    /// The step between values. Defaults to 1. Cannot be 0.
    pub step: i64,
}

impl Range {
    /// Creates a new range with the given start, stop, and step.
    ///
    /// # Panics
    /// Panics if step is 0. Use `new_checked` for fallible construction.
    #[must_use]
    pub(crate) fn new(start: i64, stop: i64, step: i64) -> Self {
        debug_assert!(step != 0, "range step cannot be 0");
        Self { start, stop, step }
    }

    /// Creates a range from just a stop value (start=0, step=1).
    #[must_use]
    fn from_stop(stop: i64) -> Self {
        Self {
            start: 0,
            stop,
            step: 1,
        }
    }

    /// Creates a range from start and stop (step=1).
    #[must_use]
    fn from_start_stop(start: i64, stop: i64) -> Self {
        Self { start, stop, step: 1 }
    }

    /// Returns the length of the range (number of elements it will yield).
    #[must_use]
    pub fn len(&self) -> usize {
        self.len_i128().try_into().unwrap_or(usize::MAX)
    }

    fn len_i128(&self) -> i128 {
        // self.stop - self.start could be up to i64::MAX - i64::MIN, which overflows i64,
        // so we use i128 for the calculation to avoid overflow.
        let start = i128::from(self.start);
        let stop = i128::from(self.stop);
        let step = i128::from(self.step);

        div_ceil(stop - start, step).max(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Checks if an integer value is contained within this range (O(1)).
    ///
    /// A value is contained if it falls within the range bounds and is aligned
    /// with the step (i.e., `(n - start) % step == 0`).
    ///
    /// The subtraction is widened to `i128` because `n - self.start` would
    /// overflow `i64` for ranges spanning the full integer span (e.g.
    /// `range(i64::MIN, i64::MAX, k)` checked against any positive `n`).
    #[must_use]
    pub fn contains(&self, n: i64) -> bool {
        let in_bounds = if self.step > 0 {
            n >= self.start && n < self.stop
        } else {
            n <= self.start && n > self.stop
        };
        if !in_bounds {
            return false;
        }
        (i128::from(n) - i128::from(self.start)) % i128::from(self.step) == 0
    }

    /// Creates a range from the `range()` constructor call.
    ///
    /// Supports:
    /// - `range(stop)` - range from 0 to stop
    /// - `range(start, stop)` - range from start to stop
    /// - `range(start, stop, step)` - range with custom step
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let pos_args = args.into_pos_only("range", vm.heap)?;
        defer_drop!(pos_args, vm);

        let range = match pos_args.as_slice() {
            [] => return Err(ExcType::type_error_at_least("range", 1, 0)),
            [first_arg] => {
                let stop = first_arg.as_int(vm)?;
                Self::from_stop(stop)
            }
            [first_arg, second_arg] => {
                let start = first_arg.as_int(vm)?;
                let stop = second_arg.as_int(vm)?;
                Self::from_start_stop(start, stop)
            }
            [first_arg, second_arg, third_arg] => {
                let start = first_arg.as_int(vm)?;
                let stop = second_arg.as_int(vm)?;
                let step = third_arg.as_int(vm)?;
                if step == 0 {
                    return Err(ExcType::value_error_range_step_zero());
                }
                Self::new(start, stop, step)
            }
            _ => return Err(ExcType::type_error_at_most("range", 3, pos_args.len())),
        };

        Ok(Value::Ref(vm.heap.allocate(HeapData::Range(range))?))
    }

    /// Handles slice-based indexing for ranges.
    ///
    /// Returns a new range object representing the sliced view.
    /// The new range has computed start, stop, and step values.
    fn getitem_slice(&self, slice: &super::Slice, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let range_len = self.len();
        let (start, stop, step) = slice.indices(range_len)?;

        // All intermediate arithmetic is done in `i128` to avoid saturating during
        // calculation. If any of the resulting `start`, `stop`, or `step`
        // values do not fit in `i64`, we raise `OverflowError` — Monty's `Range`
        // stores `i64`, so unlike CPython we cannot represent a range whose
        // parameters exceed that span.
        let self_step = i128::from(self.step);
        let self_start = i128::from(self.start);
        let slice_step = i128::from(step);

        let new_step_i128 = self_step * slice_step;
        let new_start_i128 = self_start + i128::from(start) * self_step;

        // The guarantee on `slice.indices` is that `stop` and `start` are at most
        // `range_len` apart, so the subtraction won't overflow.
        let num_elements = div_ceil(i128::from(stop) - i128::from(start), slice_step);
        let new_stop_i128 = new_start_i128 + num_elements * new_step_i128;

        let new_step = i64::try_from(new_step_i128).map_err(|_| ExcType::overflow_c_ssize_t())?;
        let new_start = i64::try_from(new_start_i128).map_err(|_| ExcType::overflow_c_ssize_t())?;
        let new_stop = i64::try_from(new_stop_i128).map_err(|_| ExcType::overflow_c_ssize_t())?;

        let new_range = Self::new(new_start, new_stop, new_step);
        Ok(Value::Ref(heap.allocate(HeapData::Range(new_range))?))
    }
}

impl Default for Range {
    fn default() -> Self {
        Self::from_stop(0)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Range> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Range
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let range = *self.get(vm.heap);
            return range.getitem_slice(slice, vm.heap);
        }

        let range = *self.get(vm.heap);

        // Calculate in i128 space to avoid overflow issues with large ranges and indices.

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = i128::from(key.as_index(vm, Type::Range)?);

        // Get range length for normalization
        let len = range.len_i128();

        let normalized = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized < 0 || normalized >= len {
            return Err(ExcType::range_index_error());
        }

        // Calculate: start + normalized * step.
        //
        // Mathematically `offset` falls within `[min(start, stop), max(start, stop))`
        // — within i64 — when the `Range` invariant holds (every element is a valid
        // i64). The fallible conversion below is defence-in-depth so that an invariant
        // violation surfaces as a Python `OverflowError` rather than a host panic.
        let offset = i128::from(range.start) + (normalized * i128::from(range.step));
        let offset_i64 = i64::try_from(offset).map_err(|_| ExcType::overflow_c_ssize_t())?;
        Ok(Value::Int(offset_i64))
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<bool> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        // Compare ranges by their actual sequences, not parameters.
        // Two ranges are equal if they produce the same elements.
        let len1 = a.len();
        let len2 = b.len();
        if len1 != len2 {
            return Ok(false);
        }
        // Same length - compare first element and step (if non-empty)
        if len1 == 0 {
            return Ok(true); // Both empty
        }
        Ok(a.start == b.start && a.step == b.step)
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let this = self.get(vm.heap);
        if this.step == 1 {
            Ok(write!(f, "range({}, {})", this.start, this.stop)?)
        } else {
            Ok(write!(f, "range({}, {}, {})", this.start, this.stop, this.step)?)
        }
    }
}

impl HeapItem for Range {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Range doesn't contain heap references, nothing to do
    }
}
