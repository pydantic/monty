//! Python slice type implementation.
//!
//! Provides a slice object representing start:stop:step indices for sequence slicing.
//! Each field is optional (None in Python), where None means "use the default for that field".

use std::{fmt, fmt::Write, mem};

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{PyTrait, Type},
    value::{EitherStr, Value},
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
    pub fn init(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let heap = &mut *vm.heap;
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
    pub fn indices(&self, length: usize) -> RunResult<(i64, i64, i64)> {
        let step = self.step.unwrap_or(1);
        if step == 0 {
            return Err(ExcType::value_error_slice_step_zero());
        }

        let len = i64::try_from(length).unwrap_or(i64::MAX);

        if step > 0 {
            // Positive step: iterate forward
            let default_start = 0;
            let default_stop = len;

            let start = self.start.map_or(default_start, |s| normalize_index(s, len, 0, len));
            let stop = self.stop.map_or(default_stop, |s| normalize_index(s, len, 0, len));

            Ok((start, stop, step))
        } else {
            // Negative step: iterate backward
            // For negative step, we need different handling
            let default_start = len - 1;
            let default_stop = -1; // Before the beginning

            let start = self
                .start
                .map_or(default_start, |s| normalize_index(s, len, -1, len - 1));
            let stop = self.stop.map_or(default_stop, |s| normalize_index(s, len, -1, len - 1));

            Ok((start, stop, step))
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

impl<'h> PyTrait<'h> for HeapRead<'h, Slice> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::Slice
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        // Slices don't have a length in Python
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        Ok(a.start == b.start && a.stop == b.stop && a.step == b.step)
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        // Slice always truthy
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str("slice(")?;
        format_option_i64(f, self.get(vm.heap).start)?;
        f.write_str(", ")?;
        format_option_i64(f, self.get(vm.heap).stop)?;
        f.write_str(", ")?;
        format_option_i64(f, self.get(vm.heap).step)?;
        Ok(f.write_char(')')?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let this = self.get(vm.heap);
        // Fast path: interned strings can be matched by ID without string comparison
        if let Some(ss) = attr.static_string() {
            return match ss {
                StaticStrings::Start => Ok(Some(CallResult::Value(option_i64_to_value(this.start)))),
                StaticStrings::Stop => Ok(Some(CallResult::Value(option_i64_to_value(this.stop)))),
                StaticStrings::Step => Ok(Some(CallResult::Value(option_i64_to_value(this.step)))),
                _ => Ok(None),
            };
        }
        // Slow path: heap-allocated strings need string comparison
        match attr.as_str(vm.interns) {
            "start" => Ok(Some(CallResult::Value(option_i64_to_value(this.start)))),
            "stop" => Ok(Some(CallResult::Value(option_i64_to_value(this.stop)))),
            "step" => Ok(Some(CallResult::Value(option_i64_to_value(this.step)))),
            _ => Ok(None),
        }
    }
}

impl HeapItem for Slice {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Slice doesn't contain heap references, nothing to do
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
fn format_option_i64(f: &mut impl Write, value: Option<i64>) -> fmt::Result {
    match value {
        Some(i) => write!(f, "{i}"),
        None => f.write_str("None"),
    }
}

/// Applies a Python `slice` to an iterator and collects the result.
///
/// This is the shared back-end for `[start:stop:step]` indexing on every
/// sequence type — `str`, `bytes`, `list`, `tuple` — that wants to evaluate
/// the slice eagerly into a new container. (`range` is the exception: it
/// composes the slice symbolically into a new `Range`, so it doesn't go
/// through here.)
///
/// `collect_map` is applied **after** the slicing logic, so per-item work
/// (e.g. `clone_with_heap` for heap-allocated `Value`s) only runs on items
/// that survive the slice. Use `|x| x` when no transform is needed.
pub(crate) fn slice_collect_iterator<Iter: DoubleEndedIterator + Clone, U, T: FromIterator<U>>(
    vm: &VM<'_, '_, impl ResourceTracker>,
    slice: &Slice,
    iter: Iter,
    collect_map: impl Fn(Iter::Item) -> U,
) -> RunResult<T> {
    let length = iter.clone().count();
    let (start, stop, step) = slice.indices(length)?;

    let final_collect_op = |item| -> RunResult<U> {
        vm.heap.check_time()?;
        Ok(collect_map(item))
    };

    if step > 0 {
        // saturate at usize::MAX - will take just the first item if step is too large for usize
        let step: usize = step.try_into().unwrap_or(usize::MAX);
        // with step > 0, slice.indices() guarantee 0 <= start/stop <= length
        let start = start
            .try_into()
            .expect("slice.indices() guarantees start > 0 for step > 0");
        let stop = stop
            .try_into()
            .expect("slice.indices() guarantees stop > 0 for step > 0");
        iter.take(stop)
            .skip(start)
            .step_by(step)
            .map(final_collect_op)
            .collect()
    } else {
        // step < 0, iterate backward
        let step: usize = step.unsigned_abs().try_into().unwrap_or(usize::MAX);

        // the +1 is because when reverse iterating, at index 'start' we want to include the item at start,
        // which means we need to skip 'length - start - 1' items from the end. Similar for stop.
        //
        // Both start and stop can be in the range [-1, length-1] for negative step, so after the +1 they are
        // known to be positive and can convert to usize (saturating if needed)
        let normalized_start = length.saturating_sub(start.saturating_add(1).try_into().unwrap_or(usize::MAX));
        let normalized_stop = length.saturating_sub(stop.saturating_add(1).try_into().unwrap_or(usize::MAX));

        iter.rev()
            .take(normalized_stop)
            .skip(normalized_start)
            .step_by(step)
            .map(final_collect_op)
            .collect()
    }
}

/// Normalizes a Python-style index (allowing negative indexing) by adding `length` if negative,
/// and then clamping to the range [0, length].
pub(crate) fn normalize_sequence_index(index: i64, len: usize) -> usize {
    if index < 0 {
        let abs_index = index.unsigned_abs().try_into().unwrap_or(usize::MAX);
        len.saturating_sub(abs_index)
    } else {
        usize::try_from(index).unwrap_or(len).min(len)
    }
}
