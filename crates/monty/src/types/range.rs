//! Python range type implementation.
//!
//! Provides a range object that supports iteration over a sequence of integers
//! with configurable start, stop, and step values.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
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
    fn new(start: i64, stop: i64, step: i64) -> Self {
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

    /// Creates a range from the `range()` constructor call.
    ///
    /// Supports:
    /// - `range(stop)` - range from 0 to stop
    /// - `range(start, stop)` - range from start to stop
    /// - `range(start, stop, step)` - range with custom step
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let range = match args {
            ArgValues::Empty => return Err(ExcType::type_error_at_least("range", 1, 0)),
            ArgValues::One(stop_val) => {
                let result = stop_val.as_int(heap);
                stop_val.drop_with_heap(heap);
                Self::from_stop(result?)
            }
            ArgValues::Two(start_val, stop_val) => {
                let start = start_val.as_int(heap);
                let stop = stop_val.as_int(heap);
                start_val.drop_with_heap(heap);
                stop_val.drop_with_heap(heap);
                Self::from_start_stop(start?, stop?)
            }
            ArgValues::ArgsKargs { args, kwargs } if kwargs.is_empty() && args.len() == 3 => {
                let mut iter = args.into_iter();
                let start_val = iter.next().unwrap();
                let stop_val = iter.next().unwrap();
                let step_val = iter.next().unwrap();

                let start = start_val.as_int(heap);
                let stop = stop_val.as_int(heap);
                let step = step_val.as_int(heap);
                start_val.drop_with_heap(heap);
                stop_val.drop_with_heap(heap);
                step_val.drop_with_heap(heap);

                let step = step?;
                if step == 0 {
                    return Err(ExcType::value_error_range_step_zero());
                }
                Self::new(start?, stop?, step)
            }
            ArgValues::Kwargs(kwargs) => {
                kwargs.drop_with_heap(heap);
                return Err(ExcType::type_error_no_kwargs("range"));
            }
            ArgValues::ArgsKargs { args, kwargs } => {
                let arg_count = args.len();
                for v in args {
                    v.drop_with_heap(heap);
                }
                if !kwargs.is_empty() {
                    kwargs.drop_with_heap(heap);
                    return Err(ExcType::type_error_no_kwargs("range"));
                }
                return Err(ExcType::type_error_at_most("range", 3, arg_count));
            }
        };

        Ok(Value::Ref(heap.allocate(HeapData::Range(range))?))
    }
}

impl Default for Range {
    fn default() -> Self {
        Self::from_stop(0)
    }
}

impl PyTrait for Range {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Range
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Extract integer index, accepting both Int and Bool (True=1, False=0)
        let index = match key {
            Value::Int(i) => *i,
            Value::Bool(b) => i64::from(*b),
            _ => return Err(ExcType::type_error_indices(Type::Range, key.py_type(heap))),
        };

        // Get range length for normalization
        let len = i64::try_from(self.len()).expect("range length exceeds i64::MAX");
        let normalized = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized < 0 || normalized >= len {
            return Err(ExcType::range_index_error());
        }

        // Calculate: start + normalized * step
        // Use checked arithmetic to avoid overflow in intermediate calculations
        let offset = normalized
            .checked_mul(self.step)
            .and_then(|v| self.start.checked_add(v))
            .expect("range element calculation overflowed");
        Ok(Value::Int(offset))
    }

    fn py_eq(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Compare ranges by their actual sequences, not parameters.
        // Two ranges are equal if they produce the same elements.
        let len1 = self.len();
        let len2 = other.len();
        if len1 != len2 {
            return false;
        }
        // Same length - compare first element and step (if non-empty)
        if len1 == 0 {
            return true; // Both empty
        }
        self.start == other.start && self.step == other.step
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _interns: &Interns,
    ) -> std::fmt::Result {
        if self.step == 1 {
            write!(f, "range({}, {})", self.start, self.stop)
        } else {
            write!(f, "range({}, {}, {})", self.start, self.stop, self.step)
        }
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Range doesn't contain heap references, nothing to do
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
