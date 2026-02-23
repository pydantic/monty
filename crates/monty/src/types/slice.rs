//! Python slice type implementation.
//!
//! Provides a slice object representing start:stop:step indices for sequence slicing.
//! Each field is optional (None in Python), where None means "use the default for that field".

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, Type},
    value::Value,
};

/// Python slice object representing start:stop:step indices.
///
/// Each field is `Option<i64>` where `None` corresponds to Python's `None`,
/// meaning "use the default value for this field based on context".
///
/// When indexing a sequence of length `n`:
/// - `start` defaults to 0 (or n-1 if step < 0)
/// - `stop` defaults to n (or -1 sentinel meaning "before index 0" if step < 0)
/// - `step` defaults to 1
///
/// The `indices(length)` method computes concrete indices from these optional values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Slice {
    pub start: Option<i64>,
    pub stop: Option<i64>,
    pub step: Option<i64>,
}

impl Slice {
    /// Creates a new slice with the given start, stop, and step values.
    #[must_use]
    pub fn new(start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> Self {
        Self { start, stop, step }
    }

    /// Creates a slice from the `slice()` constructor call.
    ///
    /// Supports:
    /// - `slice(stop)` - slice with only stop (start=None, step=None)
    /// - `slice(start, stop)` - slice with start and stop (step=None)
    /// - `slice(start, stop, step)` - slice with all three components
    ///
    /// Each argument can be None to indicate "use default".
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let pos_args = args.into_pos_only("slice", heap)?;
        defer_drop!(pos_args, heap);

        let slice = match pos_args.as_slice() {
            [] => return Err(ExcType::type_error_at_least("slice", 1, 0)),
            [first_arg] => {
                let stop = value_to_option_i64(first_arg)?;
                Self::new(None, stop, None)
            }
            [first_arg, second_arg] => {
                let start = value_to_option_i64(first_arg)?;
                let stop = value_to_option_i64(second_arg)?;
                Self::new(start, stop, None)
            }
            [first_arg, second_arg, third_arg] => {
                let start = value_to_option_i64(first_arg)?;
                let stop = value_to_option_i64(second_arg)?;
                let step = value_to_option_i64(third_arg)?;
                Self::new(start, stop, step)
            }
            _ => return Err(ExcType::type_error_at_most("slice", 3, pos_args.len())),
        };

        Ok(Value::Ref(heap.allocate(HeapData::Slice(slice))?))
    }

    /// Computes concrete indices for a sequence of the given length.
    ///
    /// This implements Python's `slice.indices(length)` semantics:
    /// - Handles negative indices (wrapping from the end)
    /// - Clamps indices to valid range [0, length]
    /// - Returns the step direction correctly for negative steps
    ///
    /// Returns `(start, stop, step)` as concrete values ready for iteration.
    /// Returns `Err(())` if step is 0 (invalid).
    ///
    /// # Algorithm
    /// For positive step:
    /// - start defaults to 0, stop defaults to length
    /// - Both are clamped to [0, length]
    ///
    /// For negative step:
    /// - start defaults to length-1, stop defaults to -1 (before beginning)
    /// - start is clamped to [-1, length-1], stop to [-1, length-1]
    pub fn indices(&self, length: usize) -> Result<(usize, usize, i64), ()> {
        let step = self.step.unwrap_or(1);
        if step == 0 {
            return Err(());
        }

        let len = i64::try_from(length).unwrap_or(i64::MAX);

        if step > 0 {
            // Positive step: iterate forward
            let default_start = 0;
            let default_stop = len;

            let start = self.start.map_or(default_start, |s| normalize_index(s, len, 0, len));
            let stop = self.stop.map_or(default_stop, |s| normalize_index(s, len, 0, len));

            // Convert to usize, clamping to valid range
            let start_usize = usize::try_from(start.max(0)).unwrap_or(0);
            let stop_usize = usize::try_from(stop.max(0)).unwrap_or(0).min(length);

            Ok((start_usize, stop_usize, step))
        } else {
            // Negative step: iterate backward
            // For negative step, we need different handling
            let default_start = len - 1;
            let default_stop = -1; // Before the beginning

            let start = self
                .start
                .map_or(default_start, |s| normalize_index(s, len, -1, len - 1));
            let stop = self.stop.map_or(default_stop, |s| normalize_index(s, len, -1, len - 1));

            // The start can be at most len-1
            let start_i64 = start.min(len - 1);
            let stop_i64 = stop; // can be -1 to mean "go all the way to beginning"

            // If start normalizes to < 0, it means the starting position is before index 0.
            // For negative step iteration, this produces an empty slice.
            // Return (0, 0, step) which makes the iteration condition `0 > 0` false immediately.
            if start_i64 < 0 {
                return Ok((0, 0, step));
            }

            let start_usize = usize::try_from(start_i64).unwrap_or(0);

            // For stop, we encode it specially: if stop is -1, it means "stop before index 0"
            // We'll use length + 1 as a sentinel to indicate "stop was None or evaluates to before 0"
            let stop_usize = if stop_i64 < 0 {
                length + 1 // sentinel value meaning "go all the way to the beginning"
            } else {
                usize::try_from(stop_i64).unwrap_or(0)
            };

            Ok((start_usize, stop_usize, step))
        }
    }
}

/// Converts a Value to Option<i64>, treating None as None.
///
/// Used for slice construction from both `slice()` builtin and `[start:stop:step]` syntax.
/// Returns Ok(None) for Value::None, Ok(Some(i)) for integers/bools,
/// or Err(TypeError) for other types.
pub(crate) fn value_to_option_i64(value: &Value) -> RunResult<Option<i64>> {
    match value {
        Value::None => Ok(None),
        Value::Int(i) => Ok(Some(*i)),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        _ => Err(ExcType::type_error_slice_indices()),
    }
}

/// Normalizes a slice index for a sequence of the given length.
///
/// Handles negative indices (counting from end) and clamps to [lower, upper].
fn normalize_index(index: i64, length: i64, lower: i64, upper: i64) -> i64 {
    let normalized = if index < 0 { index + length } else { index };
    normalized.clamp(lower, upper)
}

impl PyTrait for Slice {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Slice
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Slices don't have a length in Python
        None
    }

    fn py_eq(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        Ok(self.start == other.start && self.stop == other.stop && self.step == other.step)
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Slices are always truthy in Python
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        f.write_str("slice(")?;
        format_option_i64(f, self.start)?;
        f.write_str(", ")?;
        format_option_i64(f, self.stop)?;
        f.write_str(", ")?;
        format_option_i64(f, self.step)?;
        f.write_char(')')
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Slice doesn't contain heap references, nothing to do
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        // Slice attributes are computed values (Int or None), return Cow::Owned
        match StaticStrings::from_string_id(attr_id) {
            Some(StaticStrings::Start) => Ok(Some(AttrCallResult::Value(option_i64_to_value(self.start)))),
            Some(StaticStrings::Stop) => Ok(Some(AttrCallResult::Value(option_i64_to_value(self.stop)))),
            Some(StaticStrings::Step) => Ok(Some(AttrCallResult::Value(option_i64_to_value(self.step)))),
            _ => Ok(None),
        }
    }
}

/// Converts an Option<i64> to a Value (None or Int).
pub(crate) fn option_i64_to_value(opt: Option<i64>) -> Value {
    match opt {
        Some(i) => Value::Int(i),
        None => Value::None,
    }
}

/// Formats an Option<i64> for repr output (None or the integer).
fn format_option_i64(f: &mut impl Write, value: Option<i64>) -> std::fmt::Result {
    match value {
        Some(i) => write!(f, "{i}"),
        None => f.write_str("None"),
    }
}
