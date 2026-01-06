//! Python range type implementation.
//!
//! Provides a range object that supports iteration over a sequence of integers
//! with configurable start, stop, and step values.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
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
pub struct Range {
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
    pub fn new(start: i64, stop: i64, step: i64) -> Self {
        debug_assert!(step != 0, "range step cannot be 0");
        Self { start, stop, step }
    }

    /// Creates a range from just a stop value (start=0, step=1).
    #[must_use]
    pub fn from_stop(stop: i64) -> Self {
        Self {
            start: 0,
            stop,
            step: 1,
        }
    }

    /// Creates a range from start and stop (step=1).
    #[must_use]
    pub fn from_start_stop(start: i64, stop: i64) -> Self {
        Self { start, stop, step: 1 }
    }

    /// Returns the length of the range (number of elements it will yield).
    #[must_use]
    pub fn len(&self) -> usize {
        if self.step > 0 {
            if self.stop > self.start {
                ((self.stop - self.start - 1) / self.step + 1) as usize
            } else {
                0
            }
        } else {
            // step < 0
            if self.start > self.stop {
                ((self.start - self.stop - 1) / (-self.step) + 1) as usize
            } else {
                0
            }
        }
    }

    /// Returns true if the range is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over the range values.
    pub fn iter(&self) -> RangeIter {
        RangeIter {
            current: self.start,
            stop: self.stop,
            step: self.step,
        }
    }

    /// Creates a range from the `range()` constructor call.
    ///
    /// Supports:
    /// - `range(stop)` - range from 0 to stop
    /// - `range(start, stop)` - range from start to stop
    /// - `range(start, stop, step)` - range with custom step
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let (positional, kwargs) = args.split();

        // range() doesn't accept keyword arguments
        if !kwargs.is_empty() {
            kwargs.drop_with_heap(heap);
            for v in positional {
                v.drop_with_heap(heap);
            }
            return Err(ExcType::type_error_no_kwargs("range"));
        }

        let result = match positional.len() {
            0 => Err(ExcType::type_error_at_least("range", 1, 0)),
            1 => {
                let stop = positional[0].as_int()?;
                Ok(Range::from_stop(stop))
            }
            2 => {
                let start = positional[0].as_int()?;
                let stop = positional[1].as_int()?;
                Ok(Range::from_start_stop(start, stop))
            }
            3 => {
                let start = positional[0].as_int()?;
                let stop = positional[1].as_int()?;
                let step = positional[2].as_int()?;
                if step == 0 {
                    Err(ExcType::value_error_range_step_zero())
                } else {
                    Ok(Range::new(start, stop, step))
                }
            }
            n => Err(ExcType::type_error_at_most("range", 3, n)),
        };

        // Drop all positional args
        for v in positional {
            v.drop_with_heap(heap);
        }

        // Allocate the range on the heap
        Ok(Value::Ref(heap.allocate(HeapData::Range(result?))?))
    }
}

impl Default for Range {
    fn default() -> Self {
        Self::from_stop(0)
    }
}

/// Iterator over range values.
pub struct RangeIter {
    current: i64,
    stop: i64,
    step: i64,
}

impl Iterator for RangeIter {
    type Item = i64;

    fn next(&mut self) -> Option<Self::Item> {
        if self.step > 0 {
            if self.current < self.stop {
                let value = self.current;
                self.current += self.step;
                Some(value)
            } else {
                None
            }
        } else {
            // step < 0
            if self.current > self.stop {
                let value = self.current;
                self.current += self.step;
                Some(value)
            } else {
                None
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = if self.step > 0 {
            if self.stop > self.current {
                ((self.stop - self.current - 1) / self.step + 1) as usize
            } else {
                0
            }
        } else if self.current > self.stop {
            ((self.current - self.stop - 1) / (-self.step) + 1) as usize
        } else {
            0
        };
        (len, Some(len))
    }
}

impl ExactSizeIterator for RangeIter {}

impl PyTrait for Range {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Range
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.len())
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
        std::mem::size_of::<Range>()
    }
}
