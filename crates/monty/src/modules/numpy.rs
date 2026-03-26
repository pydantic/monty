//! Implementation of the `numpy` module.
//!
//! Provides a subset of NumPy's array creation and manipulation functions,
//! backed by Monty's built-in `NdArray` type. This module is designed to
//! make LLM-generated numpy code run transparently in the Monty sandbox.
//!
//! # Supported functions
//!
//! - `numpy.array(data)` — create an ndarray from a list
//! - `numpy.zeros(n)` — create an array of zeros
//! - `numpy.ones(n)` — create an array of ones
//! - `numpy.arange([start,] stop[, step])` — evenly spaced values within a range
//! - `numpy.linspace(start, stop, num)` — evenly spaced values over an interval
//! - `numpy.sum(a)`, `numpy.mean(a)`, `numpy.min(a)`, `numpy.max(a)`, `numpy.std(a)`
//! - `numpy.abs(a)`, `numpy.sqrt(a)`, `numpy.log(a)`, `numpy.exp(a)`, etc.
//! - `numpy.round(a, decimals)`, `numpy.clip(a, a_min, a_max)`
//! - `numpy.where(condition, x, y)`, `numpy.maximum(a, b)`, `numpy.minimum(a, b)`
//! - `numpy.sort(a)`, `numpy.unique(a)`, `numpy.concatenate(arrays)`
//! - `numpy.cumsum(a)`, `numpy.dot(a, b)`

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
        Module, NdArray, PyTrait,
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

/// `numpy.zeros(n)` — create an array of zeros with the given length.
fn call_zeros(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.zeros", vm.heap)?;
    let n = extract_size(arg, "numpy.zeros", vm)?;
    check_array_alloc_size(n, vm.heap.tracker())?;
    let arr = NdArray::from_vec_f64(vec![0.0; n]);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.ones(n)` — create an array of ones with the given length.
fn call_ones(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ones", vm.heap)?;
    let n = extract_size(arg, "numpy.ones", vm)?;
    check_array_alloc_size(n, vm.heap.tracker())?;
    let arr = NdArray::from_vec_f64(vec![1.0; n]);
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
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "num from user, validated non-negative"
        )]
        Value::Int(n) => *n as usize,
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

/// Extracts data from a value that is either an ndarray or a scalar (broadcast to `len`).
fn extract_array_or_scalar(val: &Value, len: usize, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<Vec<f64>> {
    match val {
        Value::Ref(heap_id) => {
            if let HeapData::NdArray(arr) = vm.heap.get(*heap_id) {
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
    data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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
    data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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

    let mut combined_data = Vec::new();
    let mut result_dtype = NdArrayDtype::Int64;

    for arr_id in &arr_ids {
        let HeapData::NdArray(arr) = vm.heap.get(*arr_id) else {
            return Err(ExcType::type_error(
                "numpy.concatenate() requires all elements to be ndarrays",
            ));
        };
        combined_data.extend_from_slice(arr.data());
        result_dtype = promote_dtype(result_dtype, arr.dtype());
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
// Utility helpers
// ===========================

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
