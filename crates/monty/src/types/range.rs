//! Python range type implementation.
//!
//! Provides a range object that supports iteration over a sequence of integers
//! with configurable start, stop, and step values.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    resource::{ResourceError, ResourceTracker},
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
        if self.step > 0 {
            if self.stop > self.start {
                let len_i64 = (self.stop - self.start - 1) / self.step + 1;
                usize::try_from(len_i64).expect("range length guaranteed non-negative")
            } else {
                0
            }
        } else {
            // step < 0
            if self.start > self.stop {
                let len_i64 = (self.start - self.stop - 1) / (-self.step) + 1;
                usize::try_from(len_i64).expect("range length guaranteed non-negative")
            } else {
                0
            }
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Checks if an integer value is contained within this range (O(1)).
    ///
    /// A value is contained if it falls within the range bounds and is aligned
    /// with the step (i.e., `(n - start) % step == 0`).
    #[must_use]
    pub fn contains(&self, n: i64) -> bool {
        if self.step > 0 {
            // Forward range: start <= n < stop
            if n < self.start || n >= self.stop {
                return false;
            }
        } else {
            // Backward range: stop < n <= start
            if n > self.start || n <= self.stop {
                return false;
            }
        }
        // Check if n is on the step grid
        (n - self.start) % self.step == 0
    }

    /// Creates a range from the `range()` constructor call.
    ///
    /// Supports:
    /// - `range(stop)` - range from 0 to stop
    /// - `range(start, stop)` - range from start to stop
    /// - `range(start, stop, step)` - range with custom step
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
    fn getitem_slice(&self, slice: &crate::types::Slice, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let range_len = self.len();
        let (start, stop, step) = slice
            .indices(range_len)
            .map_err(|()| ExcType::value_error_slice_step_zero())?;

        // Calculate the new range parameters
        // new_start = self.start + start * self.step
        // new_step = self.step * slice_step
        // new_stop needs to be computed based on the number of elements

        let new_step = self.step.saturating_mul(step);
        let start_i64 = i64::try_from(start).expect("start index fits in i64");
        let new_start = self.start.saturating_add(start_i64.saturating_mul(self.step));

        // Calculate the number of elements in the sliced range
        // try_from succeeds for non-negative step; step==0 rejected by slice.indices()
        let num_elements = if let Ok(step_usize) = usize::try_from(step) {
            // Forward iteration
            if start >= stop {
                0
            } else {
                ((stop - start - 1) / step_usize) + 1
            }
        } else {
            // Backward iteration
            let step_abs = usize::try_from(-step).expect("step is negative so -step is positive");
            if stop > range_len {
                // stop sentinel means "go to the beginning"
                (start / step_abs) + 1
            } else if start <= stop {
                0
            } else {
                ((start - stop - 1) / step_abs) + 1
            }
        };

        // new_stop = new_start + num_elements * new_step
        let num_elements_i64 = i64::try_from(num_elements).expect("num_elements fits in i64");
        let new_stop = new_start.saturating_add(num_elements_i64.saturating_mul(new_step));

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
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::Range
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let range = *self.get(vm.heap);
            return range.getitem_slice(slice, vm.heap);
        }

        let range = *self.get(vm.heap);

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::Range)?;

        // Get range length for normalization
        let len = i64::try_from(range.len()).expect("range length exceeds i64::MAX");
        let normalized = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized < 0 || normalized >= len {
            return Err(ExcType::range_index_error());
        }

        // Calculate: start + normalized * step
        // Use checked arithmetic to avoid overflow in intermediate calculations
        let offset = normalized
            .checked_mul(range.step)
            .and_then(|v| range.start.checked_add(v))
            .expect("range element calculation overflowed");
        Ok(Value::Int(offset))
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
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

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
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
        std::mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Range doesn't contain heap references, nothing to do
    }
}
