//! Implementation of the `numpy` module.
//!
//! Provides a subset of NumPy's array creation and manipulation functions,
//! backed by Monty's built-in `NdArray` type. This module is designed to
//! make LLM-generated numpy code run transparently in the Monty sandbox.
//!
//! # Supported functions
//!
//! ## Array creation
//! - `numpy.array(data)` — create an ndarray from a list
//! - `numpy.zeros(n)` / `numpy.zeros((m, n))` — array of zeros
//! - `numpy.ones(n)` / `numpy.ones((m, n))` — array of ones
//! - `numpy.arange([start,] stop[, step])` — evenly spaced values within a range
//! - `numpy.linspace(start, stop, num)` — evenly spaced values over an interval
//! - `numpy.full(shape, fill_value)` — array filled with a constant
//! - `numpy.eye(n)` — n×n identity matrix
//! - `numpy.empty(n)` — uninitialized array (returns zeros in Monty)
//! - `numpy.copy(a)` — copy an array
//! - `numpy.zeros_like(a)` / `numpy.ones_like(a)` — array of same shape/dtype
//!
//! ## Element-wise math
//! - `numpy.abs(a)`, `numpy.sqrt(a)`, `numpy.log(a)`, `numpy.exp(a)`
//! - `numpy.sin(a)`, `numpy.cos(a)`, `numpy.tan(a)`, `numpy.log2(a)`, `numpy.log10(a)`
//! - `numpy.ceil(a)`, `numpy.floor(a)`
//! - `numpy.power(base, exp)` — element-wise power
//! - `numpy.diff(a)` — discrete differences
//! - `numpy.round(a, decimals)`, `numpy.clip(a, a_min, a_max)`
//!
//! ## Aggregation
//! - `numpy.sum(a)`, `numpy.mean(a)`, `numpy.min(a)`, `numpy.max(a)`, `numpy.std(a)`
//! - `numpy.prod(a)`, `numpy.var(a)`, `numpy.median(a)`
//! - `numpy.argmin(a)`, `numpy.argmax(a)`
//! - `numpy.count_nonzero(a)`
//!
//! ## Testing & inspection
//! - `numpy.isnan(a)`, `numpy.isinf(a)`, `numpy.isfinite(a)`
//! - `numpy.array_equal(a, b)`
//! - `numpy.all(a)`, `numpy.any(a)`
//!
//! ## Selection & sorting
//! - `numpy.where(condition, x, y)`, `numpy.maximum(a, b)`, `numpy.minimum(a, b)`
//! - `numpy.sort(a)`, `numpy.unique(a)`
//!
//! ## Manipulation
//! - `numpy.reshape(a, shape)`, `numpy.transpose(a)`, `numpy.concatenate(arrays)`
//! - `numpy.append(a, values)`, `numpy.vstack(arrays)`, `numpy.hstack(arrays)`
//! - `numpy.stack(arrays)`, `numpy.tile(a, reps)`, `numpy.repeat(a, repeats)`
//! - `numpy.split(a, sections_or_indices)`, `numpy.cumsum(a)`, `numpy.dot(a, b)`
//!
//! ## Search & index
//! - `numpy.nonzero(a)`, `numpy.argwhere(a)`

use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker, check_array_alloc_size},
    types::{
        Module, NdArray, PyTrait, allocate_tuple,
        ndarray::{NdArrayDtype, promote_dtype},
    },
    value::Value,
};

/// Functions exposed by the `numpy` module.
///
/// Each variant corresponds to a module-level function like `np.array()` or `np.zeros()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum NumpyFunctions {
    /// `numpy.array(data)` — create an ndarray from a list.
    Array,
    /// `numpy.zeros(shape)` — create an array filled with zeros.
    Zeros,
    /// `numpy.ones(shape)` — create an array filled with ones.
    Ones,
    /// `numpy.arange([start,] stop[, step])` — evenly spaced values within a range.
    Arange,
    /// `numpy.linspace(start, stop, num)` — evenly spaced values over an interval.
    Linspace,
    /// `numpy.sum(a)` — sum of array elements.
    Sum,
    /// `numpy.mean(a)` — mean of array elements.
    Mean,
    /// `numpy.min(a)` — minimum of array elements.
    Min,
    /// `numpy.max(a)` — maximum of array elements.
    Max,
    /// `numpy.abs(a)` — element-wise absolute value.
    Abs,
    /// `numpy.sqrt(a)` — element-wise square root.
    Sqrt,
    /// `numpy.log(a)` — element-wise natural logarithm.
    Log,
    /// `numpy.exp(a)` — element-wise exponential.
    Exp,
    /// `numpy.round(a, decimals)` — element-wise rounding.
    Round,
    /// `numpy.clip(a, a_min, a_max)` — clip values to range.
    Clip,
    /// `numpy.where(condition, x, y)` — conditional selection.
    Where,
    /// `numpy.maximum(a, b)` — element-wise maximum.
    Maximum,
    /// `numpy.minimum(a, b)` — element-wise minimum.
    Minimum,
    /// `numpy.sort(a)` — return sorted copy of array.
    Sort,
    /// `numpy.unique(a)` — return sorted unique elements.
    Unique,
    /// `numpy.concatenate(arrays)` — join arrays along axis.
    Concatenate,
    /// `numpy.cumsum(a)` — cumulative sum.
    Cumsum,
    /// `numpy.dot(a, b)` — dot product.
    Dot,
    /// `numpy.ceil(a)` — element-wise ceiling.
    Ceil,
    /// `numpy.floor(a)` — element-wise floor.
    Floor,
    /// `numpy.log10(a)` — element-wise base-10 logarithm.
    Log10,
    /// `numpy.std(a)` — standard deviation of array elements.
    Std,
    /// `numpy.sin(a)` — element-wise sine.
    Sin,
    /// `numpy.cos(a)` — element-wise cosine.
    Cos,
    /// `numpy.tan(a)` — element-wise tangent.
    Tan,
    /// `numpy.log2(a)` — element-wise base-2 logarithm.
    Log2,
    /// `numpy.power(a, b)` — element-wise power.
    Power,
    /// `numpy.diff(a)` — n-th discrete difference.
    Diff,
    /// `numpy.full(shape, fill_value)` — array filled with a constant.
    Full,
    /// `numpy.eye(n)` — identity matrix.
    Eye,
    /// `numpy.copy(a)` — copy of an array.
    NpCopy,
    /// `numpy.empty(n)` — uninitialized array (returns zeros in Monty).
    Empty,
    /// `numpy.zeros_like(a)` — array of zeros with same shape/dtype.
    ZerosLike,
    /// `numpy.ones_like(a)` — array of ones with same shape/dtype.
    OnesLike,
    /// `numpy.isnan(a)` — element-wise NaN test.
    Isnan,
    /// `numpy.isinf(a)` — element-wise infinity test.
    Isinf,
    /// `numpy.isfinite(a)` — element-wise finiteness test.
    Isfinite,
    /// `numpy.array_equal(a, b)` — true if arrays are element-wise equal.
    ArrayEqual,
    /// `numpy.count_nonzero(a)` — count of non-zero elements.
    CountNonzero,
    /// `numpy.all(a)` — true if all elements are truthy.
    All,
    /// `numpy.any(a)` — true if any element is truthy.
    Any,
    /// `numpy.prod(a)` — product of array elements.
    Prod,
    /// `numpy.var(a)` — variance of array elements.
    Var,
    /// `numpy.median(a)` — median of array elements.
    Median,
    /// `numpy.argmin(a)` — index of minimum element.
    Argmin,
    /// `numpy.argmax(a)` — index of maximum element.
    Argmax,
    /// `numpy.reshape(a, shape)` — reshape an array.
    Reshape,
    // Note: np.flatten doesn't exist in real NumPy — use arr.flatten() method instead
    /// `numpy.transpose(a)` — transpose an array.
    Transpose,
    /// `numpy.append(a, values)` — append values to end of array.
    Append,
    /// `numpy.vstack(arrays)` — stack arrays vertically.
    Vstack,
    /// `numpy.hstack(arrays)` — stack arrays horizontally.
    Hstack,
    /// `numpy.stack(arrays)` — stack arrays along new axis.
    Stack,
    /// `numpy.nonzero(a)` — indices of non-zero elements.
    Nonzero,
    /// `numpy.argwhere(a)` — indices where elements are non-zero.
    Argwhere,
    /// `numpy.tile(a, reps)` — construct by repeating array.
    Tile,
    /// `numpy.repeat(a, repeats)` — repeat elements of array.
    Repeat,
    /// `numpy.split(a, indices_or_sections)` — split array into sub-arrays.
    Split,
}

/// Creates the `numpy` module and allocates it on the heap.
///
/// Registers all numpy functions as module attributes.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Numpy);

    for (name, func) in NUMPY_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Numpy(*func)), vm);
    }

    vm.heap.allocate(HeapData::Module(module))
}

/// Static mapping of attribute names to numpy functions for module creation.
const NUMPY_FUNCTIONS: &[(StaticStrings, NumpyFunctions)] = &[
    (StaticStrings::NpArray, NumpyFunctions::Array),
    (StaticStrings::NpZeros, NumpyFunctions::Zeros),
    (StaticStrings::NpOnes, NumpyFunctions::Ones),
    (StaticStrings::NpArange, NumpyFunctions::Arange),
    (StaticStrings::NpLinspace, NumpyFunctions::Linspace),
    (StaticStrings::NpSum, NumpyFunctions::Sum),
    (StaticStrings::Mean, NumpyFunctions::Mean),
    (StaticStrings::NpMin, NumpyFunctions::Min),
    (StaticStrings::NpMax, NumpyFunctions::Max),
    (StaticStrings::Abs, NumpyFunctions::Abs),
    (StaticStrings::Sqrt, NumpyFunctions::Sqrt),
    (StaticStrings::Log, NumpyFunctions::Log),
    (StaticStrings::Exp, NumpyFunctions::Exp),
    (StaticStrings::Round, NumpyFunctions::Round),
    (StaticStrings::Clip, NumpyFunctions::Clip),
    (StaticStrings::NpWhere, NumpyFunctions::Where),
    (StaticStrings::Maximum, NumpyFunctions::Maximum),
    (StaticStrings::Minimum, NumpyFunctions::Minimum),
    (StaticStrings::Sort, NumpyFunctions::Sort),
    (StaticStrings::Unique, NumpyFunctions::Unique),
    (StaticStrings::Concatenate, NumpyFunctions::Concatenate),
    (StaticStrings::Cumsum, NumpyFunctions::Cumsum),
    (StaticStrings::Dot, NumpyFunctions::Dot),
    (StaticStrings::Ceil, NumpyFunctions::Ceil),
    (StaticStrings::Floor, NumpyFunctions::Floor),
    (StaticStrings::Log10, NumpyFunctions::Log10),
    (StaticStrings::Std, NumpyFunctions::Std),
    (StaticStrings::Sin, NumpyFunctions::Sin),
    (StaticStrings::Cos, NumpyFunctions::Cos),
    (StaticStrings::Tan, NumpyFunctions::Tan),
    (StaticStrings::Log2, NumpyFunctions::Log2),
    (StaticStrings::NpPower, NumpyFunctions::Power),
    (StaticStrings::NpDiff, NumpyFunctions::Diff),
    (StaticStrings::NpFull, NumpyFunctions::Full),
    (StaticStrings::NpEye, NumpyFunctions::Eye),
    (StaticStrings::Copy, NumpyFunctions::NpCopy),
    (StaticStrings::NpEmpty, NumpyFunctions::Empty),
    (StaticStrings::NpZerosLike, NumpyFunctions::ZerosLike),
    (StaticStrings::NpOnesLike, NumpyFunctions::OnesLike),
    (StaticStrings::Isnan, NumpyFunctions::Isnan),
    (StaticStrings::Isinf, NumpyFunctions::Isinf),
    (StaticStrings::Isfinite, NumpyFunctions::Isfinite),
    (StaticStrings::NpArrayEqual, NumpyFunctions::ArrayEqual),
    (StaticStrings::NpCountNonzero, NumpyFunctions::CountNonzero),
    (StaticStrings::NpAll, NumpyFunctions::All),
    (StaticStrings::NpAny, NumpyFunctions::Any),
    (StaticStrings::NpProd, NumpyFunctions::Prod),
    (StaticStrings::NpVar, NumpyFunctions::Var),
    (StaticStrings::NpMedian, NumpyFunctions::Median),
    (StaticStrings::Argmin, NumpyFunctions::Argmin),
    (StaticStrings::Argmax, NumpyFunctions::Argmax),
    (StaticStrings::Reshape, NumpyFunctions::Reshape),
    // np.flatten doesn't exist in real NumPy
    (StaticStrings::NpTranspose, NumpyFunctions::Transpose),
    (StaticStrings::Append, NumpyFunctions::Append),
    (StaticStrings::NpVstack, NumpyFunctions::Vstack),
    (StaticStrings::NpHstack, NumpyFunctions::Hstack),
    (StaticStrings::NpStack, NumpyFunctions::Stack),
    (StaticStrings::NpNonzero, NumpyFunctions::Nonzero),
    (StaticStrings::NpArgwhere, NumpyFunctions::Argwhere),
    (StaticStrings::NpTile, NumpyFunctions::Tile),
    (StaticStrings::NpRepeat, NumpyFunctions::Repeat),
    (StaticStrings::Split, NumpyFunctions::Split),
];

/// Dispatches a call to a `numpy` module function.
pub(super) fn call(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    function: NumpyFunctions,
    args: ArgValues,
) -> RunResult<CallResult> {
    match function {
        NumpyFunctions::Array => call_array(vm, args).map(CallResult::Value),
        NumpyFunctions::Zeros => call_zeros(vm, args).map(CallResult::Value),
        NumpyFunctions::Ones => call_ones(vm, args).map(CallResult::Value),
        NumpyFunctions::Arange => call_arange(vm, args).map(CallResult::Value),
        NumpyFunctions::Linspace => call_linspace(vm, args).map(CallResult::Value),
        NumpyFunctions::Sum => call_aggregate(vm, args, NdArray::sum, "numpy.sum").map(CallResult::Value),
        NumpyFunctions::Mean => call_aggregate(vm, args, NdArray::mean, "numpy.mean").map(CallResult::Value),
        NumpyFunctions::Min => call_aggregate_result(vm, args, NdArray::min_val, "numpy.min").map(CallResult::Value),
        NumpyFunctions::Max => call_aggregate_result(vm, args, NdArray::max_val, "numpy.max").map(CallResult::Value),
        NumpyFunctions::Abs => call_elementwise(vm, args, f64::abs, "numpy.abs", None).map(CallResult::Value),
        NumpyFunctions::Sqrt => {
            call_elementwise(vm, args, f64::sqrt, "numpy.sqrt", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Log => {
            call_elementwise(vm, args, f64::ln, "numpy.log", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Exp => {
            call_elementwise(vm, args, f64::exp, "numpy.exp", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Ceil => {
            call_elementwise(vm, args, f64::ceil, "numpy.ceil", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Floor => {
            call_elementwise(vm, args, f64::floor, "numpy.floor", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Log10 => {
            call_elementwise(vm, args, f64::log10, "numpy.log10", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Round => call_round(vm, args).map(CallResult::Value),
        NumpyFunctions::Clip => call_clip(vm, args).map(CallResult::Value),
        NumpyFunctions::Where => call_where(vm, args).map(CallResult::Value),
        NumpyFunctions::Maximum => call_pairwise(vm, args, f64::max, "numpy.maximum").map(CallResult::Value),
        NumpyFunctions::Minimum => call_pairwise(vm, args, f64::min, "numpy.minimum").map(CallResult::Value),
        NumpyFunctions::Sort => call_sort(vm, args).map(CallResult::Value),
        NumpyFunctions::Unique => call_unique(vm, args).map(CallResult::Value),
        NumpyFunctions::Concatenate => call_concatenate(vm, args).map(CallResult::Value),
        NumpyFunctions::Cumsum => call_cumsum(vm, args).map(CallResult::Value),
        NumpyFunctions::Dot => call_dot(vm, args).map(CallResult::Value),
        NumpyFunctions::Std => call_aggregate(vm, args, NdArray::std_dev, "numpy.std").map(CallResult::Value),
        NumpyFunctions::Sin => {
            call_elementwise(vm, args, f64::sin, "numpy.sin", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Cos => {
            call_elementwise(vm, args, f64::cos, "numpy.cos", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Tan => {
            call_elementwise(vm, args, f64::tan, "numpy.tan", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Log2 => {
            call_elementwise(vm, args, f64::log2, "numpy.log2", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Power => call_power(vm, args).map(CallResult::Value),
        NumpyFunctions::Diff => call_diff(vm, args).map(CallResult::Value),
        NumpyFunctions::Full => call_full(vm, args).map(CallResult::Value),
        NumpyFunctions::Eye => call_eye(vm, args).map(CallResult::Value),
        NumpyFunctions::NpCopy => call_copy(vm, args).map(CallResult::Value),
        NumpyFunctions::Empty => call_empty(vm, args).map(CallResult::Value),
        NumpyFunctions::ZerosLike => call_like(vm, args, 0.0, "numpy.zeros_like").map(CallResult::Value),
        NumpyFunctions::OnesLike => call_like(vm, args, 1.0, "numpy.ones_like").map(CallResult::Value),
        NumpyFunctions::Isnan => call_bool_test(vm, args, f64::is_nan, "numpy.isnan").map(CallResult::Value),
        NumpyFunctions::Isinf => call_bool_test(vm, args, f64::is_infinite, "numpy.isinf").map(CallResult::Value),
        NumpyFunctions::Isfinite => call_bool_test(vm, args, f64::is_finite, "numpy.isfinite").map(CallResult::Value),
        NumpyFunctions::ArrayEqual => call_array_equal(vm, args).map(CallResult::Value),
        NumpyFunctions::CountNonzero => call_count_nonzero(vm, args).map(CallResult::Value),
        NumpyFunctions::All => call_all(vm, args).map(CallResult::Value),
        NumpyFunctions::Any => call_any(vm, args).map(CallResult::Value),
        NumpyFunctions::Prod => call_prod(vm, args).map(CallResult::Value),
        NumpyFunctions::Var => call_aggregate(vm, args, NdArray::var, "numpy.var").map(CallResult::Value),
        NumpyFunctions::Median => call_median(vm, args).map(CallResult::Value),
        NumpyFunctions::Argmin => call_argmin_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Argmax => call_argmax_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Reshape => call_reshape_mod(vm, args).map(CallResult::Value),
        // np.flatten doesn't exist in real NumPy
        NumpyFunctions::Transpose => call_transpose_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Append => call_append(vm, args).map(CallResult::Value),
        NumpyFunctions::Vstack => call_vstack(vm, args).map(CallResult::Value),
        NumpyFunctions::Hstack => call_hstack(vm, args).map(CallResult::Value),
        // Note: np.stack with axis=0 is equivalent to np.vstack for 1D inputs.
        // For 2D+ inputs, np.stack creates a new axis, which differs from vstack.
        // We only support the 1D case which is the LLM-common pattern.
        NumpyFunctions::Stack => call_vstack(vm, args).map(CallResult::Value),
        NumpyFunctions::Nonzero => call_nonzero(vm, args).map(CallResult::Value),
        NumpyFunctions::Argwhere => call_argwhere(vm, args).map(CallResult::Value),
        NumpyFunctions::Tile => call_tile(vm, args).map(CallResult::Value),
        NumpyFunctions::Repeat => call_repeat(vm, args).map(CallResult::Value),
        NumpyFunctions::Split => call_split(vm, args).map(CallResult::Value),
    }
}

// ===========================
// Array creation functions
// ===========================

/// `numpy.array(data)` — create an ndarray from a list or nested list.
fn call_array(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.array", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = crate::types::ndarray::ndarray_from_list(arg, vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.zeros(shape)` — create an array of zeros with the given shape.
///
/// Accepts an integer for 1D or a tuple/list for multi-dimensional shapes.
fn call_zeros(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.zeros", vm.heap)?;
    let shape = extract_shape(arg, "numpy.zeros", vm)?;
    let total: usize = shape.iter().product();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let arr = NdArray::new(vec![0.0; total], shape, NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.ones(shape)` — create an array of ones with the given shape.
///
/// Accepts an integer for 1D or a tuple/list for multi-dimensional shapes.
fn call_ones(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ones", vm.heap)?;
    let shape = extract_shape(arg, "numpy.ones", vm)?;
    let total: usize = shape.iter().product();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let arr = NdArray::new(vec![1.0; total], shape, NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.arange([start,] stop[, step])` — evenly spaced values within a range.
///
/// Supports 1, 2, or 3 arguments matching NumPy's behavior:
/// - `arange(stop)` — values from 0 to stop with step 1
/// - `arange(start, stop)` — values from start to stop with step 1
/// - `arange(start, stop, step)` — values from start to stop with given step
fn call_arange(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.arange", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let first = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.arange() requires at least 1 argument"))?;
    let second = pos.next();
    let third = pos.next();

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let (start, stop, step) = match (second, third) {
        (None, None) => (0.0, to_f64(&first, vm)?, 1.0),
        (Some(stop_val), None) => (to_f64(&first, vm)?, to_f64(&stop_val, vm)?, 1.0),
        (Some(stop_val), Some(step_val)) => (to_f64(&first, vm)?, to_f64(&stop_val, vm)?, to_f64(&step_val, vm)?),
        (None, Some(_)) => unreachable!("third arg without second"),
    };

    first.drop_with_heap(vm);

    if step == 0.0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "step must not be zero").into());
    }

    // Pre-check allocation size before building the Vec.
    // Estimate element count the same way NumPy does: ceil((stop - start) / step).
    let estimated_len = ((stop - start) / step).ceil().max(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "estimated_len is non-negative and capped by usize::MAX"
    )]
    let estimated_count = if estimated_len.is_finite() {
        (estimated_len as u64).min(usize::MAX as u64) as usize
    } else {
        0
    };
    check_array_alloc_size(estimated_count, vm.heap.tracker())?;

    let mut data = Vec::new();
    let mut val = start;
    if step > 0.0 {
        while val < stop {
            data.push(val);
            val += step;
        }
    } else {
        while val > stop {
            data.push(val);
            val += step;
        }
    }

    let has_float = start.fract() != 0.0 || stop.fract() != 0.0 || step.fract() != 0.0;
    let dtype = if has_float {
        NdArrayDtype::Float64
    } else {
        NdArrayDtype::Int64
    };
    let len = data.len();
    let arr = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.linspace(start, stop, num)` — evenly spaced values over an interval.
///
/// Returns `num` values including both endpoints.
fn call_linspace(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.linspace", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let start_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;
    let stop_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;
    let num_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let start = to_f64(&start_val, vm)?;
    let stop = to_f64(&stop_val, vm)?;
    let num = match &num_val {
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "num is checked non-negative above"
        )]
        Value::Int(n) => {
            if *n < 0 {
                return Err(SimpleException::new_msg(
                    ExcType::ValueError,
                    "Number of samples, num, must be non-negative.",
                )
                .into());
            }
            *n as usize
        }
        _ => {
            return Err(ExcType::type_error("num must be an integer"));
        }
    };

    start_val.drop_with_heap(vm);
    stop_val.drop_with_heap(vm);
    num_val.drop_with_heap(vm);

    check_array_alloc_size(num, vm.heap.tracker())?;

    let data = if num == 0 {
        Vec::new()
    } else if num == 1 {
        vec![start]
    } else {
        let step = (stop - start) / (num - 1) as f64;
        (0..num).map(|i| start + step * i as f64).collect()
    };

    let len = data.len();
    let arr = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

// ===========================
// Aggregate functions
// ===========================

/// Helper for aggregate functions like `numpy.sum(a)` that return a float.
///
/// Accepts both ndarray and plain list arguments — lists are auto-converted to
/// a temporary NdArray, matching real NumPy's behavior of `np.mean([1,2,3])`.
fn call_aggregate(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(&NdArray) -> f64,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        )));
    };
    match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => Ok(Value::Float(f(arr))),
        HeapData::List(list) => {
            let tmp = list_to_ndarray(list, name)?;
            Ok(Value::Float(f(&tmp)))
        }
        _ => Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        ))),
    }
}

/// Helper for aggregate functions that can fail (min/max on empty arrays).
///
/// Accepts both ndarray and plain list arguments — lists are auto-converted.
fn call_aggregate_result(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(&NdArray) -> RunResult<f64>,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        )));
    };
    match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => Ok(Value::Float(f(arr)?)),
        HeapData::List(list) => {
            let tmp = list_to_ndarray(list, name)?;
            Ok(Value::Float(f(&tmp)?))
        }
        _ => Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        ))),
    }
}

/// Converts a `List` of numeric values to a 1-D `NdArray`.
///
/// Used by aggregate functions to accept plain lists like `np.mean([1, 2, 3])`
/// in addition to ndarray arguments.
fn list_to_ndarray(list: &crate::types::List, name: &str) -> RunResult<NdArray> {
    let data: Vec<f64> = list
        .as_slice()
        .iter()
        .map(|v| match v {
            Value::Int(i) => Ok(*i as f64),
            Value::Float(f) => Ok(*f),
            _ => Err(ExcType::type_error(format!("{name}() list elements must be numeric"))),
        })
        .collect::<RunResult<Vec<_>>>()?;
    let len = data.len();
    Ok(NdArray::new(data, vec![len], NdArrayDtype::Float64))
}

// ===========================
// Element-wise functions
// ===========================

/// Helper for element-wise unary functions like `numpy.abs(a)`, `numpy.sqrt(a)`, etc.
///
/// Accepts both ndarray and plain list arguments — lists are auto-converted to
/// a temporary NdArray, matching real NumPy's behavior of `np.abs([1, -2, 3])`.
fn call_elementwise(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64) -> f64,
    name: &str,
    result_dtype: Option<NdArrayDtype>,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        )));
    };
    let (data, shape, source_dtype) = match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => (
            arr.data().iter().map(|&v| f(v)).collect::<Vec<f64>>(),
            arr.shape().to_vec(),
            arr.dtype(),
        ),
        HeapData::List(list) => {
            let tmp = list_to_ndarray(list, name)?;
            let data = tmp.data().iter().map(|&v| f(v)).collect::<Vec<f64>>();
            let shape = tmp.shape().to_vec();
            let dtype = tmp.dtype();
            (data, shape, dtype)
        }
        _ => {
            return Err(ExcType::type_error(format!(
                "{name}() requires an array or list argument"
            )));
        }
    };
    let dtype = result_dtype.unwrap_or(source_dtype);
    let new_arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.round(a, decimals=0)` — element-wise rounding.
fn call_round(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, decimals_val) = args.get_one_two_args("numpy.round", vm.heap)?;
    defer_drop!(arr_val, vm);

    let decimals = match decimals_val {
        #[expect(clippy::cast_possible_truncation, reason = "decimals value from user input")]
        Some(Value::Int(n)) => n as i32,
        Some(other) => {
            other.drop_with_heap(vm);
            return Err(ExcType::type_error("decimals must be an integer"));
        }
        None => 0,
    };

    let Value::Ref(heap_id) = arr_val else {
        return Err(ExcType::type_error("numpy.round() requires an ndarray argument"));
    };
    let HeapData::NdArray(arr) = vm.heap.get(*heap_id) else {
        return Err(ExcType::type_error("numpy.round() requires an ndarray argument"));
    };

    let factor = 10f64.powi(decimals);
    let data: Vec<f64> = arr.data().iter().map(|&v| (v * factor).round() / factor).collect();
    let new_arr = NdArray::new(data, arr.shape().to_vec(), NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.clip(a, a_min, a_max)` — clip (limit) array values to a range.
fn call_clip(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.clip", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;
    let min_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;
    let max_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let a_min = to_f64(&min_val, vm)?;
    let a_max = to_f64(&max_val, vm)?;
    min_val.drop_with_heap(vm);
    max_val.drop_with_heap(vm);

    let Value::Ref(heap_id) = &arr_val else {
        arr_val.drop_with_heap(vm);
        return Err(ExcType::type_error(
            "numpy.clip() requires an ndarray as the first argument",
        ));
    };
    let heap_id = *heap_id;
    let HeapData::NdArray(arr) = vm.heap.get(heap_id) else {
        arr_val.drop_with_heap(vm);
        return Err(ExcType::type_error(
            "numpy.clip() requires an ndarray as the first argument",
        ));
    };

    let data: Vec<f64> = arr.data().iter().map(|&v| v.clamp(a_min, a_max)).collect();
    let dtype = arr.dtype();
    let shape = arr.shape().to_vec();
    arr_val.drop_with_heap(vm);

    let new_arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.where(condition, x, y)` — conditional element selection.
fn call_where(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.where", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let cond_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;
    let x_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;
    let y_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let Value::Ref(cond_id) = &cond_val else {
        cond_val.drop_with_heap(vm);
        x_val.drop_with_heap(vm);
        y_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.where() condition must be an ndarray"));
    };
    let cond_id = *cond_id;
    let HeapData::NdArray(cond_arr) = vm.heap.get(cond_id) else {
        cond_val.drop_with_heap(vm);
        x_val.drop_with_heap(vm);
        y_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.where() condition must be an ndarray"));
    };

    let cond_data: Vec<f64> = cond_arr.data().to_vec();
    let cond_shape = cond_arr.shape().to_vec();
    let len = cond_data.len();

    let x_data = extract_array_or_scalar(&x_val, len, vm)?;
    let y_data = extract_array_or_scalar(&y_val, len, vm)?;

    cond_val.drop_with_heap(vm);
    x_val.drop_with_heap(vm);
    y_val.drop_with_heap(vm);

    let data: Vec<f64> = cond_data
        .iter()
        .zip(x_data.iter().zip(y_data.iter()))
        .map(|(&c, (&x, &y))| if c == 0.0 { y } else { x })
        .collect();

    let has_float = x_data.iter().chain(y_data.iter()).any(|v| v.fract() != 0.0);
    let dtype = if has_float {
        NdArrayDtype::Float64
    } else {
        NdArrayDtype::Int64
    };

    let new_arr = NdArray::new(data, cond_shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// Extracts data from a value that is either an ndarray (must match `len`) or a scalar
/// (broadcast to `len`).
///
/// When the value is an ndarray, its length is validated against `len` so that the
/// caller never builds an output with mismatched shape/data — matching NumPy's
/// broadcasting error on incompatible shapes.
fn extract_array_or_scalar(val: &Value, len: usize, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<Vec<f64>> {
    match val {
        Value::Ref(heap_id) => {
            if let HeapData::NdArray(arr) = vm.heap.get(*heap_id) {
                if arr.data().len() != len {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        format!(
                            "operands could not be broadcast together with shapes ({},) ({},)",
                            len,
                            arr.data().len()
                        ),
                    )
                    .into());
                }
                Ok(arr.data().to_vec())
            } else {
                Err(ExcType::type_error(
                    "numpy.where() arguments must be ndarrays or scalars",
                ))
            }
        }
        Value::Int(n) => Ok(vec![*n as f64; len]),
        Value::Float(f) => Ok(vec![*f; len]),
        Value::Bool(b) => Ok(vec![if *b { 1.0 } else { 0.0 }; len]),
        _ => Err(ExcType::type_error(
            "numpy.where() arguments must be ndarrays or scalars",
        )),
    }
}

/// Helper for element-wise binary functions like `numpy.maximum(a, b)`.
fn call_pairwise(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64, f64) -> f64,
    name: &str,
) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);

    let Value::Ref(a_id) = a_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error(format!("{name}() requires ndarray arguments")));
    };
    let Value::Ref(b_id) = &b_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error(format!("{name}() requires ndarray arguments")));
    };
    let b_id = *b_id;

    let HeapData::NdArray(a_arr) = vm.heap.get(*a_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error(format!("{name}() requires ndarray arguments")));
    };
    let a_data: Vec<f64> = a_arr.data().to_vec();
    let a_shape = a_arr.shape().to_vec();
    let a_dtype = a_arr.dtype();

    let HeapData::NdArray(b_arr) = vm.heap.get(b_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error(format!("{name}() requires ndarray arguments")));
    };

    if a_shape != b_arr.shape() {
        b_val.drop_with_heap(vm);
        return Err(SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into());
    }

    let data: Vec<f64> = a_data.iter().zip(b_arr.data().iter()).map(|(&a, &b)| f(a, b)).collect();
    let result_dtype = promote_dtype(a_dtype, b_arr.dtype());
    b_val.drop_with_heap(vm);

    let new_arr = NdArray::new(data, a_shape, result_dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

// ===========================
// Sorting and unique functions
// ===========================

/// `numpy.sort(a)` — return a sorted copy of the array.
fn call_sort(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.sort", vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error("numpy.sort() requires an ndarray argument"));
    };
    let HeapData::NdArray(arr) = vm.heap.get(*heap_id) else {
        return Err(ExcType::type_error("numpy.sort() requires an ndarray argument"));
    };

    let mut data = arr.data().to_vec();
    let dtype = arr.dtype();
    let shape = arr.shape().to_vec();
    data.sort_by(crate::types::ndarray::nan_last_cmp);
    let new_arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.unique(a)` — return the sorted unique elements of an array.
fn call_unique(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.unique", vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error("numpy.unique() requires an ndarray argument"));
    };
    let HeapData::NdArray(arr) = vm.heap.get(*heap_id) else {
        return Err(ExcType::type_error("numpy.unique() requires an ndarray argument"));
    };

    let mut data = arr.data().to_vec();
    let dtype = arr.dtype();
    data.sort_by(crate::types::ndarray::nan_last_cmp);
    data.dedup();
    let len = data.len();
    let new_arr = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.concatenate(arrays)` — join a sequence of arrays along the first axis.
fn call_concatenate(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.concatenate", vm.heap)?;
    defer_drop!(arg, vm);

    let Value::Ref(list_id) = arg else {
        return Err(ExcType::type_error("numpy.concatenate() requires a list of arrays"));
    };
    let arr_ids: Vec<HeapId> = {
        let HeapData::List(list) = vm.heap.get(*list_id) else {
            return Err(ExcType::type_error("numpy.concatenate() requires a list of arrays"));
        };
        let mut ids = Vec::new();
        for v in list.as_slice() {
            let Value::Ref(id) = v else {
                return Err(ExcType::type_error(
                    "numpy.concatenate() requires all elements to be ndarrays",
                ));
            };
            ids.push(*id);
        }
        ids
    };

    let mut total_len: usize = 0;
    let mut result_dtype = NdArrayDtype::Int64;

    for arr_id in &arr_ids {
        let HeapData::NdArray(arr) = vm.heap.get(*arr_id) else {
            return Err(ExcType::type_error(
                "numpy.concatenate() requires all elements to be ndarrays",
            ));
        };
        total_len = total_len.saturating_add(arr.data().len());
        result_dtype = promote_dtype(result_dtype, arr.dtype());
    }

    check_array_alloc_size(total_len, vm.heap.tracker())?;

    let mut combined_data = Vec::with_capacity(total_len);
    for arr_id in &arr_ids {
        let HeapData::NdArray(arr) = vm.heap.get(*arr_id) else {
            unreachable!("already validated above");
        };
        combined_data.extend_from_slice(arr.data());
    }

    let len = combined_data.len();
    let new_arr = NdArray::new(combined_data, vec![len], result_dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.cumsum(a)` — return the cumulative sum of array elements.
fn call_cumsum(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.cumsum", vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error("numpy.cumsum() requires an ndarray argument"));
    };
    let HeapData::NdArray(arr) = vm.heap.get(*heap_id) else {
        return Err(ExcType::type_error("numpy.cumsum() requires an ndarray argument"));
    };

    let src = arr.data();
    let dtype = arr.dtype();
    let mut data = Vec::with_capacity(src.len());
    let mut running = 0.0;
    for &v in src {
        running += v;
        data.push(running);
    }
    let len = data.len();
    let new_arr = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.dot(a, b)` — dot product of two 1D arrays.
fn call_dot(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.dot", vm.heap)?;
    defer_drop!(a_val, vm);

    let Value::Ref(a_id) = a_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.dot() requires ndarray arguments"));
    };
    let Value::Ref(b_id) = &b_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.dot() requires ndarray arguments"));
    };
    let b_id = *b_id;

    let HeapData::NdArray(a_arr) = vm.heap.get(*a_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.dot() requires ndarray arguments"));
    };
    let a_data: Vec<f64> = a_arr.data().to_vec();
    let a_dtype = a_arr.dtype();

    let HeapData::NdArray(b_arr) = vm.heap.get(b_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.dot() requires ndarray arguments"));
    };

    if a_data.len() != b_arr.data().len() {
        b_val.drop_with_heap(vm);
        return Err(SimpleException::new_msg(ExcType::ValueError, "shapes are not aligned for dot product").into());
    }

    let result: f64 = a_data.iter().zip(b_arr.data().iter()).map(|(&a, &b)| a * b).sum();
    let b_dtype = b_arr.dtype();
    b_val.drop_with_heap(vm);

    let value = if a_dtype == NdArrayDtype::Int64 && b_dtype == NdArrayDtype::Int64 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64 to i64 truncation is intended for int dot product"
        )]
        Value::Int(result as i64)
    } else {
        Value::Float(result)
    };
    Ok(value)
}

// ===========================
// Element-wise math, array creation, testing, aggregation, manipulation,
// search, and utility functions
// ===========================

/// `numpy.power(a, b)` — element-wise power (like `a ** b`).
///
/// Supports array-array, array-scalar, and scalar-array combinations.
fn call_power(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.power", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_info = extract_ndarray_info(a_val, "numpy.power", vm);
    let b_info = extract_ndarray_info(b_val, "numpy.power", vm);

    match (a_info, b_info) {
        // Both arrays
        (Ok((a_data, a_shape, a_dtype)), Ok((b_data, b_shape, b_dtype))) => {
            if a_shape != b_shape {
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
                );
            }
            let data: Vec<f64> = a_data.iter().zip(b_data.iter()).map(|(&a, &b)| a.powf(b)).collect();
            let result_dtype = promote_dtype(a_dtype, b_dtype);
            let arr = NdArray::new(data, a_shape, result_dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        // First is array, second is scalar
        (Ok((a_data, a_shape, a_dtype)), Err(_)) => {
            let scalar = to_f64(b_val, vm)?;
            let is_float = matches!(b_val, Value::Float(_));
            let data: Vec<f64> = a_data.iter().map(|&a| a.powf(scalar)).collect();
            let dtype = crate::types::ndarray::promote_dtype_with_scalar(a_dtype, is_float);
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        // First is scalar, second is array
        (Err(_), Ok((b_data, b_shape, b_dtype))) => {
            let scalar = to_f64(a_val, vm)?;
            let is_float = matches!(a_val, Value::Float(_));
            let data: Vec<f64> = b_data.iter().map(|&b| scalar.powf(b)).collect();
            let dtype = crate::types::ndarray::promote_dtype_with_scalar(b_dtype, is_float);
            let arr = NdArray::new(data, b_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        // Neither is array
        (Err(e), _) => Err(e),
    }
}

/// `numpy.diff(a)` — first-order discrete difference: `a[1:] - a[:-1]`.
fn call_diff(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.diff", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.diff", vm)?;
    if arr.len() <= 1 {
        let result = NdArray::new(Vec::new(), vec![0], arr.dtype());
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }
    let data: Vec<f64> = arr.data().windows(2).map(|w| w[1] - w[0]).collect();
    let len = data.len();
    let arr = NdArray::new(data, vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.full(shape, fill_value)` — create an array filled with a constant.
fn call_full(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (shape_val, fill_val) = args.get_two_args("numpy.full", vm.heap)?;
    defer_drop!(shape_val, vm);
    let shape = extract_shape(shape_val.clone_immediate(), "numpy.full", vm)?;
    let (fill, dtype) = match fill_val {
        Value::Int(n) => (n as f64, NdArrayDtype::Int64),
        Value::Float(f) => (f, NdArrayDtype::Float64),
        Value::Bool(b) => (if b { 1.0 } else { 0.0 }, NdArrayDtype::Bool),
        other => {
            other.drop_with_heap(vm);
            return Err(ExcType::type_error("numpy.full() fill_value must be numeric"));
        }
    };
    let total: usize = shape.iter().product();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let arr = NdArray::new(vec![fill; total], shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.eye(n)` — create an n×n identity matrix (Float64).
fn call_eye(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.eye", vm.heap)?;
    let n = extract_size(arg, "numpy.eye", vm)?;
    check_array_alloc_size(n * n, vm.heap.tracker())?;
    let mut data = vec![0.0; n * n];
    for i in 0..n {
        data[i * n + i] = 1.0;
    }
    let arr = NdArray::new(data, vec![n, n], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.copy(a)` — return a copy of the array, also accepts plain lists.
fn call_copy(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.copy", vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error("numpy.copy() requires an array or list"));
    };
    let result = match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => NdArray::new(arr.data().to_vec(), arr.shape().to_vec(), arr.dtype()),
        HeapData::List(_) => {
            // Use ndarray_from_list which handles proper dtype tracking
            crate::types::ndarray::ndarray_from_list(arg, vm.heap)?
        }
        _ => return Err(ExcType::type_error("numpy.copy() requires an array or list")),
    };
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.empty(shape)` — create an uninitialized array (returns zeros in Monty).
fn call_empty(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.empty", vm.heap)?;
    let shape = extract_shape(arg, "numpy.empty", vm)?;
    let total: usize = shape.iter().product();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let arr = NdArray::new(vec![0.0; total], shape, NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// Helper for `numpy.zeros_like(a)` and `numpy.ones_like(a)`.
fn call_like(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues, fill: f64, name: &str) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, name, vm)?;
    let total = arr.len();
    let result = NdArray::new(vec![fill; total], arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Helper for element-wise boolean test functions like `numpy.isnan`, `numpy.isinf`, etc.
///
/// Applies the predicate to each element and returns a Bool dtype array.
fn call_bool_test(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    args: ArgValues,
    pred: fn(f64) -> bool,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, name, vm)?;
    let data: Vec<f64> = arr.data().iter().map(|&v| if pred(v) { 1.0 } else { 0.0 }).collect();
    let result = NdArray::new(data, arr.shape().to_vec(), NdArrayDtype::Bool);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.array_equal(a, b)` — true if two arrays have same shape and elements.
///
/// Uses direct f64 equality, so `NaN != NaN` — matching NumPy's behavior.
fn call_array_equal(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.array_equal", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_arr = ndarray_from_value(a_val, "numpy.array_equal", vm)?;
    let b_arr = ndarray_from_value(b_val, "numpy.array_equal", vm)?;

    let equal = a_arr.shape() == b_arr.shape() && a_arr.data() == b_arr.data();
    Ok(Value::Bool(equal))
}

/// `numpy.count_nonzero(a)` — count non-zero elements.
fn call_count_nonzero(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.count_nonzero", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.count_nonzero", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "count won't exceed i64::MAX")]
    let count = arr.data().iter().filter(|&&v| v != 0.0).count() as i64;
    Ok(Value::Int(count))
}

/// `numpy.all(a)` — true if all elements are truthy (module-level wrapper).
fn call_all(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.all", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.all", vm)?;
    Ok(Value::Bool(arr.all()))
}

/// `numpy.any(a)` — true if any element is truthy (module-level wrapper).
fn call_any(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.any", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.any", vm)?;
    Ok(Value::Bool(arr.any()))
}

/// `numpy.prod(a)` — product of array elements.
fn call_prod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.prod", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.prod", vm)?;
    let product = arr.prod();
    match arr.dtype() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64 to i64 truncation is intended for int prod"
        )]
        NdArrayDtype::Int64 => Ok(Value::Int(product as i64)),
        NdArrayDtype::Float64 | NdArrayDtype::Bool => Ok(Value::Float(product)),
    }
}

/// `numpy.median(a)` — median of array elements.
fn call_median(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.median", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.median", vm)?;
    if arr.len() == 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "zero-size array has no median").into());
    }
    let mut sorted = arr.data().to_vec();
    sorted.sort_by(crate::types::ndarray::nan_last_cmp);
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 0 {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    };
    Ok(Value::Float(median))
}

/// `numpy.argmin(a)` — index of minimum element (module-level wrapper).
fn call_argmin_mod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argmin", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argmin", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
    Ok(Value::Int(arr.argmin()? as i64))
}

/// `numpy.argmax(a)` — index of maximum element (module-level wrapper).
fn call_argmax_mod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argmax", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argmax", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
    Ok(Value::Int(arr.argmax()? as i64))
}

/// `numpy.reshape(a, shape)` — reshape an array (module-level wrapper).
fn call_reshape_mod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.reshape", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.reshape() requires 2 arguments"))?;
    let shape_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.reshape() requires 2 arguments"))?;

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let shape = extract_shape_from_value(&shape_val, "numpy.reshape", vm)?;
    shape_val.drop_with_heap(vm);

    let arr = ndarray_from_value(&arr_val, "numpy.reshape", vm)?;
    let result = arr.reshape(shape, vm.heap);
    arr_val.drop_with_heap(vm);
    result
}

/// `numpy.transpose(a)` — transpose an array (module-level wrapper).
fn call_transpose_mod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.transpose", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.transpose", vm)?;
    arr.transpose(vm.heap)
}

/// `numpy.append(a, values)` — append values to end of array (flattened).
fn call_append(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.append", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_arr = ndarray_from_value(a_val, "numpy.append", vm)?;
    let b_arr = ndarray_from_value(b_val, "numpy.append", vm)?;

    let mut combined = a_arr.data().to_vec();
    combined.extend_from_slice(b_arr.data());
    let len = combined.len();
    check_array_alloc_size(len, vm.heap.tracker())?;
    let result_dtype = promote_dtype(a_arr.dtype(), b_arr.dtype());
    let arr = NdArray::new(combined, vec![len], result_dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.vstack(arrays)` / `numpy.stack(arrays)` — stack 1D arrays as rows of a 2D array.
fn call_vstack(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.vstack", vm.heap)?;
    defer_drop!(arg, vm);

    let Value::Ref(list_id) = arg else {
        return Err(ExcType::type_error("numpy.vstack() requires a list of arrays"));
    };
    let arr_ids: Vec<HeapId> = {
        let HeapData::List(list) = vm.heap.get(*list_id) else {
            return Err(ExcType::type_error("numpy.vstack() requires a list of arrays"));
        };
        list.as_slice()
            .iter()
            .map(|v| match v {
                Value::Ref(id) => Ok(*id),
                _ => Err(ExcType::type_error(
                    "numpy.vstack() requires all elements to be ndarrays",
                )),
            })
            .collect::<RunResult<Vec<_>>>()?
    };

    if arr_ids.is_empty() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "need at least one array to stack").into());
    }

    // Get the column count from the first array.
    let HeapData::NdArray(first) = vm.heap.get(arr_ids[0]) else {
        return Err(ExcType::type_error("numpy.vstack() requires ndarrays"));
    };
    let cols = if first.ndim() == 1 {
        first.len()
    } else {
        first.shape()[1]
    };
    let mut result_dtype = first.dtype();

    let mut combined = Vec::new();
    for &arr_id in &arr_ids {
        let HeapData::NdArray(arr) = vm.heap.get(arr_id) else {
            return Err(ExcType::type_error("numpy.vstack() requires ndarrays"));
        };
        let arr_cols = if arr.ndim() == 1 { arr.len() } else { arr.shape()[1] };
        if arr_cols != cols {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                "all input arrays must have the same number of columns",
            )
            .into());
        }
        combined.extend_from_slice(arr.data());
        result_dtype = promote_dtype(result_dtype, arr.dtype());
    }

    let rows = combined.len() / cols;
    check_array_alloc_size(combined.len(), vm.heap.tracker())?;
    let arr = NdArray::new(combined, vec![rows, cols], result_dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.hstack(arrays)` — concatenate arrays horizontally.
///
/// For 1D arrays, hstack is equivalent to concatenate (the LLM-common case).
/// For 2D+ arrays, hstack should concatenate along axis=1 — this is not yet
/// implemented and will incorrectly concatenate along axis=0 instead.
fn call_hstack(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    call_concatenate(vm, args)
}

/// `numpy.nonzero(a)` — indices of non-zero elements, returned as a tuple of arrays.
fn call_nonzero(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.nonzero", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.nonzero", vm)?;

    let indices: Vec<f64> = arr
        .data()
        .iter()
        .enumerate()
        .filter(|&(_, v)| *v != 0.0)
        .map(|(i, _)| i as f64)
        .collect();

    let len = indices.len();
    let idx_arr = NdArray::new(indices, vec![len], NdArrayDtype::Int64);
    let idx_val = Value::Ref(vm.heap.allocate(HeapData::NdArray(idx_arr))?);

    // NumPy returns a tuple with one array per dimension. For 1D input, it's a 1-tuple.
    // Note: if allocate_tuple fails (resource limit), idx_val may be leaked. This is
    // acceptable per project convention — resource exhaustion is a terminal error.
    let values: SmallVec<[Value; 3]> = smallvec::smallvec![idx_val];
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// `numpy.argwhere(a)` — indices where elements are non-zero, as 2D array.
fn call_argwhere(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argwhere", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argwhere", vm)?;

    let indices: Vec<f64> = arr
        .data()
        .iter()
        .enumerate()
        .filter(|&(_, v)| *v != 0.0)
        .map(|(i, _)| i as f64)
        .collect();

    let rows = indices.len();
    // For 1D input, argwhere returns shape (n_nonzero, 1)
    let result = NdArray::new(indices, vec![rows, 1], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.tile(a, reps)` — construct array by repeating `a` `reps` times.
fn call_tile(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, reps_val) = args.get_two_args("numpy.tile", vm.heap)?;
    defer_drop!(arr_val, vm);

    let arr = ndarray_from_value(arr_val, "numpy.tile", vm)?;
    defer_drop!(reps_val, vm);
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "reps checked non-negative"
    )]
    let reps = if let Value::Int(n) = reps_val {
        if *n < 0 {
            return Err(SimpleException::new_msg(ExcType::ValueError, "negative number of repetitions").into());
        }
        *n as usize
    } else {
        return Err(ExcType::type_error("numpy.tile() reps must be an integer"));
    };

    if reps == 0 || arr.len() == 0 {
        let result = NdArray::new(Vec::new(), vec![0], arr.dtype());
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }

    let total = arr.len() * reps;
    check_array_alloc_size(total, vm.heap.tracker())?;
    let data: Vec<f64> = arr.data().iter().copied().cycle().take(total).collect();
    let result = NdArray::new(data, vec![total], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.repeat(a, repeats)` — repeat each element `repeats` times.
fn call_repeat(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, reps_val) = args.get_two_args("numpy.repeat", vm.heap)?;
    defer_drop!(arr_val, vm);

    let arr = ndarray_from_value(arr_val, "numpy.repeat", vm)?;
    defer_drop!(reps_val, vm);
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "repeats checked non-negative"
    )]
    let reps = if let Value::Int(n) = reps_val {
        if *n < 0 {
            return Err(SimpleException::new_msg(ExcType::ValueError, "negative number of repetitions").into());
        }
        *n as usize
    } else {
        return Err(ExcType::type_error("numpy.repeat() repeats must be an integer"));
    };

    if arr.len() == 0 {
        let result = NdArray::new(Vec::new(), vec![0], arr.dtype());
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }

    let total = arr.len() * reps;
    check_array_alloc_size(total, vm.heap.tracker())?;
    let mut data = Vec::with_capacity(total);
    for &v in arr.data() {
        for _ in 0..reps {
            data.push(v);
        }
    }
    let result = NdArray::new(data, vec![total], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.split(a, indices_or_sections)` — split array into sub-arrays.
///
/// If the second argument is an integer, splits into that many equal parts.
/// If it's a list/array, splits at the given indices.
fn call_split(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, idx_val) = args.get_two_args("numpy.split", vm.heap)?;
    defer_drop!(arr_val, vm);

    let arr = ndarray_from_value(arr_val, "numpy.split", vm)?;
    let data = arr.data();
    let dtype = arr.dtype();

    // Determine split points
    let split_indices: Vec<usize> = match &idx_val {
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "sections checked > 0"
        )]
        Value::Int(n) => {
            if *n <= 0 {
                idx_val.drop_with_heap(vm);
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "number sections must be larger than 0").into(),
                );
            }
            let sections = *n as usize;
            if data.len() % sections != 0 {
                idx_val.drop_with_heap(vm);
                return Err(SimpleException::new_msg(
                    ExcType::ValueError,
                    "array split does not result in an equal division",
                )
                .into());
            }
            let chunk_size = data.len() / sections;
            (1..sections).map(|i| i * chunk_size).collect()
        }
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => list
                .as_slice()
                .iter()
                .map(|v| match v {
                    #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation, reason = "index from user")]
                    Value::Int(n) => Ok(*n as usize),
                    _ => Err(ExcType::type_error("split indices must be integers")),
                })
                .collect::<RunResult<Vec<_>>>()?,
            HeapData::NdArray(idx_arr) =>
            {
                #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation, reason = "index from user")]
                idx_arr.data().iter().map(|&v| v as usize).collect()
            }
            _ => {
                idx_val.drop_with_heap(vm);
                return Err(ExcType::type_error("numpy.split() second arg must be int or list"));
            }
        },
        _ => {
            idx_val.drop_with_heap(vm);
            return Err(ExcType::type_error("numpy.split() second arg must be int or list"));
        }
    };
    idx_val.drop_with_heap(vm);

    // Build sub-arrays. Note: if allocation fails partway through, previously allocated
    // sub-arrays in `parts` are leaked. This is acceptable — allocation failure is a
    // terminal resource-limit error (see CLAUDE.md reference counting docs).
    let mut parts = Vec::new();
    let mut prev = 0;
    for &idx in &split_indices {
        let end = idx.min(data.len());
        let chunk = data[prev..end].to_vec();
        let len = chunk.len();
        parts.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(NdArray::new(
            chunk,
            vec![len],
            dtype,
        )))?));
        prev = end;
    }
    // Last chunk
    let chunk = data[prev..].to_vec();
    let len = chunk.len();
    parts.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(NdArray::new(
        chunk,
        vec![len],
        dtype,
    )))?));

    let list = crate::types::List::new(parts);
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(list))?))
}

// ===========================
// Utility helpers
// ===========================

/// Extracts ndarray data from a Value, auto-converting lists.
///
/// Returns (data, shape, dtype) tuple — copies data out to avoid lifetime issues.
/// Uses `ndarray_from_list` for lists so dtype tracking (int vs float vs bool) is correct.
fn extract_ndarray_info(
    value: &Value,
    name: &str,
    vm: &VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(Vec<f64>, Vec<usize>, NdArrayDtype)> {
    match value {
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::NdArray(arr) => Ok((arr.data().to_vec(), arr.shape().to_vec(), arr.dtype())),
            HeapData::List(_) => {
                let tmp = crate::types::ndarray::ndarray_from_list(value, vm.heap)?;
                Ok((tmp.data().to_vec(), tmp.shape().to_vec(), tmp.dtype()))
            }
            _ => Err(ExcType::type_error(format!(
                "{name}() requires an array or list argument"
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{name}() requires an array or list argument"
        ))),
    }
}

/// Convenience wrapper that returns an NdArray (owned).
fn ndarray_from_value(value: &Value, name: &str, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<NdArray> {
    let (data, shape, dtype) = extract_ndarray_info(value, name, vm)?;
    Ok(NdArray::new(data, shape, dtype))
}

/// Extracts a shape from a Value — supports int (1D), list, or tuple.
fn extract_shape(value: Value, func_name: &str, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Vec<usize>> {
    match &value {
        Value::Int(_) => {
            let n = extract_size(value, func_name, vm)?;
            Ok(vec![n])
        }
        Value::Ref(heap_id) => {
            let shape = match vm.heap.get(*heap_id) {
                HeapData::List(list) => extract_shape_from_items(list.as_slice(), func_name)?,
                HeapData::Tuple(tuple) => extract_shape_from_items(tuple.as_slice(), func_name)?,
                _ => {
                    value.drop_with_heap(vm);
                    return Err(ExcType::type_error(format!(
                        "{func_name}() requires an integer or tuple of integers"
                    )));
                }
            };
            value.drop_with_heap(vm);
            Ok(shape)
        }
        _ => {
            value.drop_with_heap(vm);
            Err(ExcType::type_error(format!(
                "{func_name}() requires an integer or tuple of integers"
            )))
        }
    }
}

/// Extracts shape from a Value without consuming it (for reshape where we borrow).
fn extract_shape_from_value(
    value: &Value,
    func_name: &str,
    vm: &VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    match value {
        #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation, reason = "shape from user")]
        Value::Int(n) if *n >= 0 => Ok(vec![*n as usize]),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::List(list) => extract_shape_from_items(list.as_slice(), func_name),
            HeapData::Tuple(tuple) => extract_shape_from_items(tuple.as_slice(), func_name),
            _ => Err(ExcType::type_error(format!(
                "{func_name}() requires an integer or tuple of integers"
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{func_name}() requires an integer or tuple of integers"
        ))),
    }
}

/// Extracts a shape vector from a slice of Values (list or tuple items).
fn extract_shape_from_items(items: &[Value], func_name: &str) -> RunResult<Vec<usize>> {
    items
        .iter()
        .map(|v| match v {
            #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation, reason = "shape from user")]
            Value::Int(n) if *n >= 0 => Ok(*n as usize),
            _ => Err(ExcType::type_error(format!(
                "{func_name}() shape must contain non-negative integers"
            ))),
        })
        .collect()
}

/// Extracts an integer size from a Value for array creation functions.
fn extract_size(value: Value, func_name: &str, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<usize> {
    match value {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "n is guaranteed non-negative"
        )]
        Value::Int(n) if n >= 0 => Ok(n as usize),
        Value::Int(_) => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{func_name}(): negative dimensions are not allowed"),
        )
        .into()),
        _ => {
            value.drop_with_heap(vm);
            Err(ExcType::type_error(format!(
                "{func_name}() requires an integer argument"
            )))
        }
    }
}

/// Converts a Value to f64 for numeric operations.
fn to_f64(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<f64> {
    match value {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(ExcType::type_error(format!(
            "a number is required, not '{}'",
            value.py_type(vm)
        ))),
    }
}
