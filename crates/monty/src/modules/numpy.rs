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

use std::{
    cmp::Ordering,
    f64::consts::{E, PI},
};

use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{HeapData, HeapId},
    heap_traits::DropWithHeap,
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker, check_array_alloc_size},
    types::{
        List, Module, NdArray, PyTrait, allocate_tuple,
        ndarray::{NdArrayDtype, nan_last_cmp, ndarray_from_list, promote_dtype, promote_dtype_with_scalar},
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
    /// `numpy.add(a, b)` — element-wise addition.
    Add,
    /// `numpy.subtract(a, b)` — element-wise subtraction.
    Subtract,
    /// `numpy.multiply(a, b)` — element-wise multiplication.
    Multiply,
    /// `numpy.divide(a, b)` / `numpy.true_divide(a, b)` — element-wise true division.
    Divide,
    /// `numpy.floor_divide(a, b)` — element-wise floor division.
    FloorDivide,
    /// `numpy.mod(a, b)` / `numpy.remainder(a, b)` — element-wise Python modulo.
    Mod,
    /// `numpy.equal(a, b)` — element-wise equality comparison.
    Equal,
    /// `numpy.not_equal(a, b)` — element-wise inequality comparison.
    NotEqual,
    /// `numpy.greater(a, b)` — element-wise greater-than comparison.
    Greater,
    /// `numpy.greater_equal(a, b)` — element-wise greater-or-equal comparison.
    GreaterEqual,
    /// `numpy.less(a, b)` — element-wise less-than comparison.
    Less,
    /// `numpy.less_equal(a, b)` — element-wise less-or-equal comparison.
    LessEqual,
    /// `numpy.shape(a)` — tuple of dimensions.
    Shape,
    /// `numpy.size(a)` — total number of elements.
    Size,
    /// `numpy.ndim(a)` — number of dimensions.
    Ndim,

    // --- Phase 3: Inverse trig, hyperbolic, remaining math ---
    /// `numpy.arcsin(a)` — element-wise inverse sine.
    Arcsin,
    /// `numpy.arccos(a)` — element-wise inverse cosine.
    Arccos,
    /// `numpy.arctan(a)` — element-wise inverse tangent.
    Arctan,
    /// `numpy.arctan2(y, x)` — element-wise two-argument arctangent.
    Arctan2,
    /// `numpy.sinh(a)` — element-wise hyperbolic sine.
    Sinh,
    /// `numpy.cosh(a)` — element-wise hyperbolic cosine.
    Cosh,
    /// `numpy.tanh(a)` — element-wise hyperbolic tangent.
    Tanh,
    /// `numpy.arcsinh(a)` — element-wise inverse hyperbolic sine.
    Arcsinh,
    /// `numpy.arccosh(a)` — element-wise inverse hyperbolic cosine.
    Arccosh,
    /// `numpy.arctanh(a)` — element-wise inverse hyperbolic tangent.
    Arctanh,
    /// `numpy.sign(a)` — element-wise sign (-1, 0, or 1).
    Sign,
    /// `numpy.square(a)` — element-wise square.
    Square,
    /// `numpy.cbrt(a)` — element-wise cube root.
    Cbrt,
    /// `numpy.reciprocal(a)` — element-wise 1/x.
    Reciprocal,
    /// `numpy.log1p(a)` — element-wise log(1 + x).
    Log1p,
    /// `numpy.exp2(a)` — element-wise 2^x.
    Exp2,
    /// `numpy.expm1(a)` — element-wise exp(x) - 1.
    Expm1,
    /// `numpy.deg2rad(a)` — convert degrees to radians.
    Deg2rad,
    /// `numpy.rad2deg(a)` — convert radians to degrees.
    Rad2deg,
    /// `numpy.hypot(a, b)` — element-wise hypotenuse.
    Hypot,
    /// `numpy.nan_to_num(a)` — replace NaN with 0 and Inf with large finite.
    NanToNum,
    /// `numpy.fmin(a, b)` — element-wise minimum ignoring NaN.
    Fmin,
    /// `numpy.fmax(a, b)` — element-wise maximum ignoring NaN.
    Fmax,
    /// `numpy.fmod(a, b)` — element-wise C-style modulo.
    Fmod,
    /// `numpy.rint(a)` — round to nearest integer.
    Rint,
    /// `numpy.fabs(a)` — element-wise absolute value (float result).
    Fabs,
    /// `numpy.positive(a)` — element-wise unary +.
    Positive,
    /// `numpy.negative(a)` — element-wise unary -.
    Negative,
    /// `numpy.conj(a)` — return the real-valued conjugate.
    Conj,
    /// `numpy.real(a)` — return the real component.
    Real,
    /// `numpy.imag(a)` — return the imaginary component.
    Imag,
    /// `numpy.isreal(a)` — element-wise predicate for real values.
    Isreal,
    /// `numpy.isrealobj(a)` — true when the input is not complex-valued.
    Isrealobj,
    /// `numpy.iscomplex(a)` — element-wise predicate for complex values.
    Iscomplex,
    /// `numpy.iscomplexobj(a)` — true when the input has a complex dtype.
    Iscomplexobj,
    /// `numpy.isscalar(a)` — true for scalar values.
    Isscalar,
    /// `numpy.iterable(a)` — true for values accepted by Monty's iterator protocol.
    Iterable,

    // --- Phase 4: NaN-aware aggregations and statistics ---
    /// `numpy.nansum(a)` — sum ignoring NaN.
    Nansum,
    /// `numpy.nanmean(a)` — mean ignoring NaN.
    Nanmean,
    /// `numpy.nanmin(a)` — min ignoring NaN.
    Nanmin,
    /// `numpy.nanmax(a)` — max ignoring NaN.
    Nanmax,
    /// `numpy.nanstd(a)` — std ignoring NaN.
    Nanstd,
    /// `numpy.nanvar(a)` — var ignoring NaN.
    Nanvar,
    /// `numpy.nanprod(a)` — product ignoring NaN.
    Nanprod,
    /// `numpy.nanmedian(a)` — median ignoring NaN.
    Nanmedian,
    /// `numpy.nanargmin(a)` — argmin ignoring NaN.
    Nanargmin,
    /// `numpy.nanargmax(a)` — argmax ignoring NaN.
    Nanargmax,
    /// `numpy.average(a)` — weighted average (simple mean without weights).
    Average,
    /// `numpy.percentile(a, q)` — q-th percentile.
    Percentile,
    /// `numpy.quantile(a, q)` — q-th quantile (q in [0,1]).
    Quantile,
    /// `numpy.ptp(a)` — peak-to-peak (max - min).
    Ptp,
    /// `numpy.cumprod(a)` — cumulative product.
    Cumprod,
    /// `numpy.nancumsum(a)` — cumulative sum ignoring NaN.
    Nancumsum,
    /// `numpy.nancumprod(a)` — cumulative product ignoring NaN.
    Nancumprod,

    // --- Phase 5: Logical and testing functions ---
    /// `numpy.logical_and(a, b)` — element-wise logical AND.
    LogicalAnd,
    /// `numpy.logical_or(a, b)` — element-wise logical OR.
    LogicalOr,
    /// `numpy.logical_not(a)` — element-wise logical NOT.
    LogicalNot,
    /// `numpy.logical_xor(a, b)` — element-wise logical XOR.
    LogicalXor,
    /// `numpy.allclose(a, b)` — true if all elements are close.
    Allclose,
    /// `numpy.isclose(a, b)` — element-wise closeness test.
    Isclose,
    /// `numpy.isin(element, test_elements)` — element membership test.
    Isin,

    // --- Phase 6: Manipulation and shape ---
    /// `numpy.flip(a)` — reverse array elements.
    Flip,
    /// `numpy.fliplr(a)` — flip left-right (2D).
    Fliplr,
    /// `numpy.flipud(a)` — flip up-down (2D).
    Flipud,
    /// `numpy.roll(a, shift)` — roll elements along axis.
    Roll,
    /// `numpy.expand_dims(a, axis)` — add axis.
    ExpandDims,
    /// `numpy.squeeze(a)` — remove length-1 axes.
    Squeeze,
    /// `numpy.ravel(a)` — flatten to 1D (module-level).
    Ravel,
    /// `numpy.delete(arr, indices)` — delete elements.
    Delete,
    /// `numpy.insert(arr, index, values)` — insert values.
    Insert,
    /// `numpy.diag(v)` — extract diagonal or create diagonal matrix.
    Diag,
    /// `numpy.diagonal(a)` — return diagonal of array.
    Diagonal,
    /// `numpy.trace(a)` — sum of diagonal elements.
    Trace,
    /// `numpy.flatnonzero(a)` — non-zero indices in flattened array.
    Flatnonzero,
    /// `numpy.asarray(a)` — convert to array without copy if possible.
    Asarray,
    /// `numpy.column_stack(arrays)` — stack 1D arrays as columns.
    ColumnStack,
    /// `numpy.row_stack(arrays)` — alias for vstack.
    RowStack,
    /// `numpy.hsplit(a, n)` — horizontal split.
    Hsplit,
    /// `numpy.vsplit(a, n)` — vertical split.
    Vsplit,
    /// `numpy.array_split(a, n)` — split into possibly unequal parts.
    ArraySplit,
    /// `numpy.full_like(a, fill_value)` — array of same shape filled with value.
    FullLike,
    /// `numpy.empty_like(a)` — uninitialized array of same shape.
    EmptyLike,

    // --- Phase 7: Sorting, searching, set operations ---
    /// `numpy.argsort(a)` — module-level argsort.
    ArgsortMod,
    /// `numpy.searchsorted(a, v)` — find insertion points.
    Searchsorted,
    /// `numpy.extract(condition, arr)` — extract elements by condition.
    Extract,
    /// `numpy.intersect1d(a, b)` — sorted unique intersection.
    Intersect1d,
    /// `numpy.union1d(a, b)` — sorted unique union.
    Union1d,
    /// `numpy.setdiff1d(a, b)` — elements in a not in b.
    Setdiff1d,
    /// `numpy.setxor1d(a, b)` — elements in either but not both.
    Setxor1d,
    /// `numpy.bincount(a)` — count occurrences of each non-negative int.
    Bincount,
    /// `numpy.digitize(x, bins)` — indices of bins.
    Digitize,

    // --- Phase 8: Linear algebra ---
    /// `numpy.matmul(a, b)` — matrix multiplication.
    Matmul,
    /// `numpy.inner(a, b)` — inner product.
    Inner,
    /// `numpy.outer(a, b)` — outer product.
    Outer,
    /// `numpy.vdot(a, b)` — vector dot product (flattens first).
    Vdot,
    /// `numpy.cross(a, b)` — cross product (3-element vectors).
    Cross,

    // --- Phase 10: Additional creation functions ---
    /// `numpy.logspace(start, stop, num)` — log-spaced values.
    Logspace,
    /// `numpy.geomspace(start, stop, num)` — geometrically spaced values.
    Geomspace,
    /// `numpy.tri(N)` — triangular array.
    Tri,
    /// `numpy.tril(m)` — lower triangle.
    Tril,
    /// `numpy.triu(m)` — upper triangle.
    Triu,
    /// `numpy.identity(n)` — identity matrix (alias for eye).
    Identity,
    /// `numpy.meshgrid(*xi)` — coordinate matrices from vectors.
    Meshgrid,
    /// `numpy.gradient(f)` — numerical gradient.
    Gradient,
    /// `numpy.convolve(a, v)` — discrete linear convolution.
    Convolve,
    /// `numpy.correlate(a, v)` — cross-correlation.
    Correlate,
    /// `numpy.interp(x, xp, fp)` — 1D linear interpolation.
    Interp,
    /// `numpy.select(condlist, choicelist)` — conditional selection.
    Select,
}

/// Creates the `numpy` module and allocates it on the heap.
///
/// Registers all numpy functions as module attributes.
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Numpy);

    for (name, func) in NUMPY_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Numpy(*func)), vm);
    }

    // Module-level constants
    module.set_attr(StaticStrings::Pi, Value::Float(PI), vm);
    module.set_attr(StaticStrings::MathE, Value::Float(E), vm);
    module.set_attr(StaticStrings::MathInf, Value::Float(f64::INFINITY), vm);
    module.set_attr(StaticStrings::MathNan, Value::Float(f64::NAN), vm);
    module.set_attr(StaticStrings::Newaxis, Value::None, vm);

    // Dtype type objects — stored as interned strings that astype() recognizes.
    // These allow `arr.astype(np.float64)` to work alongside `arr.astype('float64')`.
    module.set_attr(
        StaticStrings::NpFloat64,
        Value::InternString(StaticStrings::NpFloat64.into()),
        vm,
    );
    module.set_attr(
        StaticStrings::NpInt64,
        Value::InternString(StaticStrings::NpInt64.into()),
        vm,
    );
    module.set_attr(
        StaticStrings::NpBool_,
        Value::InternString(StaticStrings::NpBool_.into()),
        vm,
    );
    module.set_attr(
        StaticStrings::NpFloat32,
        Value::InternString(StaticStrings::NpFloat32.into()),
        vm,
    );
    module.set_attr(
        StaticStrings::NpInt32,
        Value::InternString(StaticStrings::NpInt32.into()),
        vm,
    );

    vm.heap.allocate(HeapData::Module(module))
}

/// Static mapping of attribute names to numpy functions for module creation.
const NUMPY_FUNCTIONS: &[(StaticStrings, NumpyFunctions)] = &[
    (StaticStrings::NpArray, NumpyFunctions::Array),
    (StaticStrings::NpAsanyarray, NumpyFunctions::Asarray),
    (StaticStrings::NpZeros, NumpyFunctions::Zeros),
    (StaticStrings::NpOnes, NumpyFunctions::Ones),
    (StaticStrings::Add, NumpyFunctions::Add),
    (StaticStrings::NpSubtract, NumpyFunctions::Subtract),
    (StaticStrings::NpMultiply, NumpyFunctions::Multiply),
    (StaticStrings::NpDivide, NumpyFunctions::Divide),
    (StaticStrings::NpTrueDivide, NumpyFunctions::Divide), // alias
    (StaticStrings::NpFloorDivide, NumpyFunctions::FloorDivide),
    (StaticStrings::NpMod, NumpyFunctions::Mod),
    (StaticStrings::Remainder, NumpyFunctions::Mod), // alias
    (StaticStrings::NpEqual, NumpyFunctions::Equal),
    (StaticStrings::NpNotEqual, NumpyFunctions::NotEqual),
    (StaticStrings::NpGreater, NumpyFunctions::Greater),
    (StaticStrings::NpGreaterEqual, NumpyFunctions::GreaterEqual),
    (StaticStrings::NpLess, NumpyFunctions::Less),
    (StaticStrings::NpLessEqual, NumpyFunctions::LessEqual),
    (StaticStrings::NpArange, NumpyFunctions::Arange),
    (StaticStrings::NpLinspace, NumpyFunctions::Linspace),
    (StaticStrings::NpSum, NumpyFunctions::Sum),
    (StaticStrings::Mean, NumpyFunctions::Mean),
    (StaticStrings::NpMin, NumpyFunctions::Min),
    (StaticStrings::NpAmin, NumpyFunctions::Min), // alias
    (StaticStrings::NpMax, NumpyFunctions::Max),
    (StaticStrings::NpAmax, NumpyFunctions::Max), // alias
    (StaticStrings::Abs, NumpyFunctions::Abs),
    (StaticStrings::Absolute, NumpyFunctions::Abs), // alias
    (StaticStrings::Sqrt, NumpyFunctions::Sqrt),
    (StaticStrings::Log, NumpyFunctions::Log),
    (StaticStrings::Exp, NumpyFunctions::Exp),
    (StaticStrings::Round, NumpyFunctions::Round),
    (StaticStrings::NpAround, NumpyFunctions::Round), // alias
    (StaticStrings::Clip, NumpyFunctions::Clip),
    (StaticStrings::NpWhere, NumpyFunctions::Where),
    (StaticStrings::Maximum, NumpyFunctions::Maximum),
    (StaticStrings::Minimum, NumpyFunctions::Minimum),
    (StaticStrings::Sort, NumpyFunctions::Sort),
    (StaticStrings::Unique, NumpyFunctions::Unique),
    (StaticStrings::Concatenate, NumpyFunctions::Concatenate),
    (StaticStrings::NpConcat, NumpyFunctions::Concatenate), // alias
    (StaticStrings::Cumsum, NumpyFunctions::Cumsum),
    (StaticStrings::NpCumulativeSum, NumpyFunctions::Cumsum), // alias
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
    (StaticStrings::Pow, NumpyFunctions::Power), // alias
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
    (StaticStrings::NpShape, NumpyFunctions::Shape),
    (StaticStrings::NpSize, NumpyFunctions::Size),
    (StaticStrings::NpNdim, NumpyFunctions::Ndim),
    // Phase 3: Inverse trig, hyperbolic, remaining math
    (StaticStrings::NpArcsin, NumpyFunctions::Arcsin),
    (StaticStrings::Asin, NumpyFunctions::Arcsin), // alias
    (StaticStrings::NpArccos, NumpyFunctions::Arccos),
    (StaticStrings::Acos, NumpyFunctions::Arccos), // alias
    (StaticStrings::NpArctan, NumpyFunctions::Arctan),
    (StaticStrings::Atan, NumpyFunctions::Arctan), // alias
    (StaticStrings::NpArctan2, NumpyFunctions::Arctan2),
    (StaticStrings::Atan2, NumpyFunctions::Arctan2), // alias
    (StaticStrings::Sinh, NumpyFunctions::Sinh),
    (StaticStrings::Cosh, NumpyFunctions::Cosh),
    (StaticStrings::Tanh, NumpyFunctions::Tanh),
    (StaticStrings::NpArcsinh, NumpyFunctions::Arcsinh),
    (StaticStrings::Asinh, NumpyFunctions::Arcsinh), // alias
    (StaticStrings::NpArccosh, NumpyFunctions::Arccosh),
    (StaticStrings::Acosh, NumpyFunctions::Arccosh), // alias
    (StaticStrings::NpArctanh, NumpyFunctions::Arctanh),
    (StaticStrings::Atanh, NumpyFunctions::Arctanh), // alias
    (StaticStrings::NpSign, NumpyFunctions::Sign),
    (StaticStrings::NpSquare, NumpyFunctions::Square),
    (StaticStrings::Cbrt, NumpyFunctions::Cbrt),
    (StaticStrings::NpReciprocal, NumpyFunctions::Reciprocal),
    (StaticStrings::Log1p, NumpyFunctions::Log1p),
    (StaticStrings::Exp2, NumpyFunctions::Exp2),
    (StaticStrings::Expm1, NumpyFunctions::Expm1),
    (StaticStrings::NpDeg2rad, NumpyFunctions::Deg2rad),
    (StaticStrings::NpRad2deg, NumpyFunctions::Rad2deg),
    (StaticStrings::Degrees, NumpyFunctions::Rad2deg), // alias
    (StaticStrings::Radians, NumpyFunctions::Deg2rad), // alias
    (StaticStrings::NpHypot, NumpyFunctions::Hypot),
    (StaticStrings::NpNanToNum, NumpyFunctions::NanToNum),
    (StaticStrings::NpFmin, NumpyFunctions::Fmin),
    (StaticStrings::NpFmax, NumpyFunctions::Fmax),
    (StaticStrings::Fmod, NumpyFunctions::Fmod),
    (StaticStrings::NpRint, NumpyFunctions::Rint),
    (StaticStrings::Fabs, NumpyFunctions::Fabs),
    (StaticStrings::NpPositive, NumpyFunctions::Positive),
    (StaticStrings::NpNegative, NumpyFunctions::Negative),
    // Real-only aliases and introspection helpers
    (StaticStrings::NpConj, NumpyFunctions::Conj),
    (StaticStrings::NpConjugate, NumpyFunctions::Conj), // alias
    (StaticStrings::NpReal, NumpyFunctions::Real),
    (StaticStrings::NpImag, NumpyFunctions::Imag),
    (StaticStrings::NpIsreal, NumpyFunctions::Isreal),
    (StaticStrings::NpIsrealobj, NumpyFunctions::Isrealobj),
    (StaticStrings::NpIscomplex, NumpyFunctions::Iscomplex),
    (StaticStrings::NpIscomplexobj, NumpyFunctions::Iscomplexobj),
    (StaticStrings::NpIsscalar, NumpyFunctions::Isscalar),
    (StaticStrings::NpIterable, NumpyFunctions::Iterable),
    // Phase 4: NaN-aware aggregations and statistics
    (StaticStrings::NpNansum, NumpyFunctions::Nansum),
    (StaticStrings::NpNanmean, NumpyFunctions::Nanmean),
    (StaticStrings::NpNanmin, NumpyFunctions::Nanmin),
    (StaticStrings::NpNanmax, NumpyFunctions::Nanmax),
    (StaticStrings::NpNanstd, NumpyFunctions::Nanstd),
    (StaticStrings::NpNanvar, NumpyFunctions::Nanvar),
    (StaticStrings::NpNanprod, NumpyFunctions::Nanprod),
    (StaticStrings::NpNanmedian, NumpyFunctions::Nanmedian),
    (StaticStrings::NpNanargmin, NumpyFunctions::Nanargmin),
    (StaticStrings::NpNanargmax, NumpyFunctions::Nanargmax),
    (StaticStrings::NpAverage, NumpyFunctions::Average),
    (StaticStrings::NpPercentile, NumpyFunctions::Percentile),
    (StaticStrings::NpQuantile, NumpyFunctions::Quantile),
    (StaticStrings::NpPtp, NumpyFunctions::Ptp),
    (StaticStrings::NpCumprod, NumpyFunctions::Cumprod),
    (StaticStrings::NpCumulativeProd, NumpyFunctions::Cumprod), // alias
    (StaticStrings::NpNancumsum, NumpyFunctions::Nancumsum),
    (StaticStrings::NpNancumprod, NumpyFunctions::Nancumprod),
    // Phase 5: Logical and testing
    (StaticStrings::NpLogicalAnd, NumpyFunctions::LogicalAnd),
    (StaticStrings::NpLogicalOr, NumpyFunctions::LogicalOr),
    (StaticStrings::NpLogicalNot, NumpyFunctions::LogicalNot),
    (StaticStrings::NpLogicalXor, NumpyFunctions::LogicalXor),
    (StaticStrings::NpAllclose, NumpyFunctions::Allclose),
    (StaticStrings::Isclose, NumpyFunctions::Isclose),
    (StaticStrings::NpIsin, NumpyFunctions::Isin),
    // Phase 6: Manipulation and shape
    (StaticStrings::NpFlip, NumpyFunctions::Flip),
    (StaticStrings::NpFliplr, NumpyFunctions::Fliplr),
    (StaticStrings::NpFlipud, NumpyFunctions::Flipud),
    (StaticStrings::NpRoll, NumpyFunctions::Roll),
    (StaticStrings::NpExpandDims, NumpyFunctions::ExpandDims),
    (StaticStrings::NpSqueeze, NumpyFunctions::Squeeze),
    (StaticStrings::NpRavel, NumpyFunctions::Ravel),
    (StaticStrings::NpDelete, NumpyFunctions::Delete),
    (StaticStrings::Insert, NumpyFunctions::Insert),
    (StaticStrings::NpDiag, NumpyFunctions::Diag),
    (StaticStrings::NpDiagonal, NumpyFunctions::Diagonal),
    (StaticStrings::NpTrace, NumpyFunctions::Trace),
    (StaticStrings::NpFlatnonzero, NumpyFunctions::Flatnonzero),
    (StaticStrings::NpAsarray, NumpyFunctions::Asarray),
    (StaticStrings::NpColumnStack, NumpyFunctions::ColumnStack),
    (StaticStrings::NpRowStack, NumpyFunctions::RowStack),
    (StaticStrings::NpHsplit, NumpyFunctions::Hsplit),
    (StaticStrings::NpVsplit, NumpyFunctions::Vsplit),
    (StaticStrings::NpArraySplit, NumpyFunctions::ArraySplit),
    (StaticStrings::NpFullLike, NumpyFunctions::FullLike),
    (StaticStrings::NpEmptyLike, NumpyFunctions::EmptyLike),
    // Phase 7: Sorting, searching, set ops
    (StaticStrings::NpArgsort, NumpyFunctions::ArgsortMod),
    (StaticStrings::NpSearchsorted, NumpyFunctions::Searchsorted),
    (StaticStrings::NpExtract, NumpyFunctions::Extract),
    (StaticStrings::NpIntersect1d, NumpyFunctions::Intersect1d),
    (StaticStrings::NpUnion1d, NumpyFunctions::Union1d),
    (StaticStrings::NpSetdiff1d, NumpyFunctions::Setdiff1d),
    (StaticStrings::NpSetxor1d, NumpyFunctions::Setxor1d),
    (StaticStrings::NpBincount, NumpyFunctions::Bincount),
    (StaticStrings::NpDigitize, NumpyFunctions::Digitize),
    // Phase 8: Linear algebra
    (StaticStrings::NpMatmul, NumpyFunctions::Matmul),
    (StaticStrings::NpInner, NumpyFunctions::Inner),
    (StaticStrings::NpOuter, NumpyFunctions::Outer),
    (StaticStrings::NpVdot, NumpyFunctions::Vdot),
    (StaticStrings::NpCross, NumpyFunctions::Cross),
    // Phase 10: Additional creation and numerical
    (StaticStrings::NpLogspace, NumpyFunctions::Logspace),
    (StaticStrings::NpGeomspace, NumpyFunctions::Geomspace),
    (StaticStrings::NpTri, NumpyFunctions::Tri),
    (StaticStrings::NpTril, NumpyFunctions::Tril),
    (StaticStrings::NpTriu, NumpyFunctions::Triu),
    (StaticStrings::NpIdentity, NumpyFunctions::Identity),
    (StaticStrings::NpMeshgrid, NumpyFunctions::Meshgrid),
    (StaticStrings::NpGradient, NumpyFunctions::Gradient),
    (StaticStrings::NpConvolve, NumpyFunctions::Convolve),
    (StaticStrings::NpCorrelate, NumpyFunctions::Correlate),
    (StaticStrings::NpInterp, NumpyFunctions::Interp),
    (StaticStrings::NpSelect, NumpyFunctions::Select),
];

/// Dispatches a call to a `numpy` module function.
pub(super) fn call(
    vm: &mut VM<'_, impl ResourceTracker>,
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
        NumpyFunctions::Add => {
            call_numeric_binop(vm, args, |a, b| a + b, "numpy.add", BinopResult::Promoted).map(CallResult::Value)
        }
        NumpyFunctions::Subtract => {
            call_numeric_binop(vm, args, |a, b| a - b, "numpy.subtract", BinopResult::Promoted).map(CallResult::Value)
        }
        NumpyFunctions::Multiply => {
            call_numeric_binop(vm, args, |a, b| a * b, "numpy.multiply", BinopResult::Promoted).map(CallResult::Value)
        }
        NumpyFunctions::Divide => {
            call_numeric_binop(vm, args, |a, b| a / b, "numpy.divide", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::FloorDivide => call_numeric_binop(
            vm,
            args,
            |a, b| (a / b).floor(),
            "numpy.floor_divide",
            BinopResult::Promoted,
        )
        .map(CallResult::Value),
        NumpyFunctions::Mod => {
            call_numeric_binop(vm, args, py_mod, "numpy.mod", BinopResult::Promoted).map(CallResult::Value)
        }
        NumpyFunctions::Equal => {
            call_numeric_binop(vm, args, eq_to_f64, "numpy.equal", BinopResult::Bool).map(CallResult::Value)
        }
        NumpyFunctions::NotEqual => {
            call_numeric_binop(vm, args, ne_to_f64, "numpy.not_equal", BinopResult::Bool).map(CallResult::Value)
        }
        NumpyFunctions::Greater => call_numeric_binop(
            vm,
            args,
            |a, b| if a > b { 1.0 } else { 0.0 },
            "numpy.greater",
            BinopResult::Bool,
        )
        .map(CallResult::Value),
        NumpyFunctions::GreaterEqual => call_numeric_binop(
            vm,
            args,
            |a, b| if a >= b { 1.0 } else { 0.0 },
            "numpy.greater_equal",
            BinopResult::Bool,
        )
        .map(CallResult::Value),
        NumpyFunctions::Less => call_numeric_binop(
            vm,
            args,
            |a, b| if a < b { 1.0 } else { 0.0 },
            "numpy.less",
            BinopResult::Bool,
        )
        .map(CallResult::Value),
        NumpyFunctions::LessEqual => call_numeric_binop(
            vm,
            args,
            |a, b| if a <= b { 1.0 } else { 0.0 },
            "numpy.less_equal",
            BinopResult::Bool,
        )
        .map(CallResult::Value),
        NumpyFunctions::Shape => call_shape(vm, args).map(CallResult::Value),
        NumpyFunctions::Size => call_size(vm, args).map(CallResult::Value),
        NumpyFunctions::Ndim => call_ndim(vm, args).map(CallResult::Value),
        // Phase 3: Inverse trig, hyperbolic, remaining math
        NumpyFunctions::Arcsin => {
            call_elementwise(vm, args, f64::asin, "numpy.arcsin", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arccos => {
            call_elementwise(vm, args, f64::acos, "numpy.arccos", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arctan => {
            call_elementwise(vm, args, f64::atan, "numpy.arctan", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arctan2 => {
            call_numeric_binop(vm, args, f64::atan2, "numpy.arctan2", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Sinh => {
            call_elementwise(vm, args, f64::sinh, "numpy.sinh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Cosh => {
            call_elementwise(vm, args, f64::cosh, "numpy.cosh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Tanh => {
            call_elementwise(vm, args, f64::tanh, "numpy.tanh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arcsinh => {
            call_elementwise(vm, args, f64::asinh, "numpy.arcsinh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arccosh => {
            call_elementwise(vm, args, f64::acosh, "numpy.arccosh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Arctanh => {
            call_elementwise(vm, args, f64::atanh, "numpy.arctanh", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Sign => {
            // numpy.sign returns 0.0 for 0.0, unlike Rust's signum which returns 1.0
            call_elementwise(
                vm,
                args,
                |x| if x == 0.0 { 0.0 } else { x.signum() },
                "numpy.sign",
                None,
            )
            .map(CallResult::Value)
        }
        NumpyFunctions::Square => call_elementwise(vm, args, |x| x * x, "numpy.square", None).map(CallResult::Value),
        NumpyFunctions::Cbrt => {
            call_elementwise(vm, args, f64::cbrt, "numpy.cbrt", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Reciprocal => {
            call_elementwise(vm, args, |x| 1.0 / x, "numpy.reciprocal", Some(NdArrayDtype::Float64))
                .map(CallResult::Value)
        }
        NumpyFunctions::Log1p => {
            call_elementwise(vm, args, f64::ln_1p, "numpy.log1p", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Exp2 => {
            call_elementwise(vm, args, f64::exp2, "numpy.exp2", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Expm1 => {
            call_elementwise(vm, args, f64::exp_m1, "numpy.expm1", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Deg2rad => {
            call_elementwise(vm, args, f64::to_radians, "numpy.deg2rad", Some(NdArrayDtype::Float64))
                .map(CallResult::Value)
        }
        NumpyFunctions::Rad2deg => {
            call_elementwise(vm, args, f64::to_degrees, "numpy.rad2deg", Some(NdArrayDtype::Float64))
                .map(CallResult::Value)
        }
        NumpyFunctions::Hypot => call_pairwise(vm, args, f64::hypot, "numpy.hypot").map(CallResult::Value),
        NumpyFunctions::NanToNum => call_nan_to_num(vm, args).map(CallResult::Value),
        NumpyFunctions::Fmin => call_pairwise(
            vm,
            args,
            |a, b| {
                if a.is_nan() {
                    b
                } else if b.is_nan() {
                    a
                } else {
                    a.min(b)
                }
            },
            "numpy.fmin",
        )
        .map(CallResult::Value),
        NumpyFunctions::Fmax => call_pairwise(
            vm,
            args,
            |a, b| {
                if a.is_nan() {
                    b
                } else if b.is_nan() {
                    a
                } else {
                    a.max(b)
                }
            },
            "numpy.fmax",
        )
        .map(CallResult::Value),
        NumpyFunctions::Fmod => call_pairwise(vm, args, |a, b| a % b, "numpy.fmod").map(CallResult::Value),
        NumpyFunctions::Rint => call_elementwise(
            vm,
            args,
            f64::round_ties_even,
            "numpy.rint",
            Some(NdArrayDtype::Float64),
        )
        .map(CallResult::Value),
        NumpyFunctions::Fabs => {
            call_elementwise(vm, args, f64::abs, "numpy.fabs", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Positive => call_elementwise(vm, args, |x| x, "numpy.positive", None).map(CallResult::Value),
        NumpyFunctions::Negative => call_elementwise(vm, args, |x| -x, "numpy.negative", None).map(CallResult::Value),
        NumpyFunctions::Conj => call_real_identity(vm, args, "numpy.conj").map(CallResult::Value),
        NumpyFunctions::Real => call_real_identity(vm, args, "numpy.real").map(CallResult::Value),
        NumpyFunctions::Imag => call_imag(vm, args).map(CallResult::Value),
        NumpyFunctions::Isreal => call_realness_elementwise(vm, args, true, "numpy.isreal").map(CallResult::Value),
        NumpyFunctions::Isrealobj => call_realness_object(vm, args, true, "numpy.isrealobj").map(CallResult::Value),
        NumpyFunctions::Iscomplex => {
            call_realness_elementwise(vm, args, false, "numpy.iscomplex").map(CallResult::Value)
        }
        NumpyFunctions::Iscomplexobj => {
            call_realness_object(vm, args, false, "numpy.iscomplexobj").map(CallResult::Value)
        }
        NumpyFunctions::Isscalar => call_isscalar(vm, args).map(CallResult::Value),
        NumpyFunctions::Iterable => call_iterable(vm, args).map(CallResult::Value),
        // Phase 4: NaN-aware aggregations and statistics
        NumpyFunctions::Nansum => call_nan_aggregate(vm, args, nan_sum, "numpy.nansum").map(CallResult::Value),
        NumpyFunctions::Nanmean => call_nan_aggregate(vm, args, nan_mean, "numpy.nanmean").map(CallResult::Value),
        NumpyFunctions::Nanmin => call_nan_aggregate(vm, args, nan_min, "numpy.nanmin").map(CallResult::Value),
        NumpyFunctions::Nanmax => call_nan_aggregate(vm, args, nan_max, "numpy.nanmax").map(CallResult::Value),
        NumpyFunctions::Nanstd => call_nan_aggregate(vm, args, nan_std, "numpy.nanstd").map(CallResult::Value),
        NumpyFunctions::Nanvar => call_nan_aggregate(vm, args, nan_var, "numpy.nanvar").map(CallResult::Value),
        NumpyFunctions::Nanprod => call_nan_aggregate(vm, args, nan_prod, "numpy.nanprod").map(CallResult::Value),
        NumpyFunctions::Nanmedian => call_nan_aggregate(vm, args, nan_median, "numpy.nanmedian").map(CallResult::Value),
        NumpyFunctions::Nanargmin => call_nan_argmin(vm, args).map(CallResult::Value),
        NumpyFunctions::Nanargmax => call_nan_argmax(vm, args).map(CallResult::Value),
        NumpyFunctions::Average => call_aggregate(vm, args, NdArray::mean, "numpy.average").map(CallResult::Value),
        NumpyFunctions::Percentile => call_percentile(vm, args).map(CallResult::Value),
        NumpyFunctions::Quantile => call_quantile(vm, args).map(CallResult::Value),
        NumpyFunctions::Ptp => call_ptp(vm, args).map(CallResult::Value),
        NumpyFunctions::Cumprod => call_cumprod(vm, args).map(CallResult::Value),
        NumpyFunctions::Nancumsum => call_nancumop(vm, args, true, "numpy.nancumsum").map(CallResult::Value),
        NumpyFunctions::Nancumprod => call_nancumop(vm, args, false, "numpy.nancumprod").map(CallResult::Value),
        // Phase 5: Logical and testing
        NumpyFunctions::LogicalAnd => {
            call_logical_binop(vm, args, |a, b| a && b, "numpy.logical_and").map(CallResult::Value)
        }
        NumpyFunctions::LogicalOr => {
            call_logical_binop(vm, args, |a, b| a || b, "numpy.logical_or").map(CallResult::Value)
        }
        NumpyFunctions::LogicalNot => call_logical_not(vm, args).map(CallResult::Value),
        NumpyFunctions::LogicalXor => {
            call_logical_binop(vm, args, |a, b| a ^ b, "numpy.logical_xor").map(CallResult::Value)
        }
        NumpyFunctions::Allclose => call_allclose(vm, args).map(CallResult::Value),
        NumpyFunctions::Isclose => call_isclose(vm, args).map(CallResult::Value),
        NumpyFunctions::Isin => call_isin(vm, args).map(CallResult::Value),
        // Phase 6: Manipulation and shape
        NumpyFunctions::Flip => call_flip(vm, args).map(CallResult::Value),
        NumpyFunctions::Fliplr => call_fliplr(vm, args).map(CallResult::Value),
        NumpyFunctions::Flipud => call_flipud(vm, args).map(CallResult::Value),
        NumpyFunctions::Roll => call_roll(vm, args).map(CallResult::Value),
        NumpyFunctions::ExpandDims => call_expand_dims(vm, args).map(CallResult::Value),
        NumpyFunctions::Squeeze => call_squeeze(vm, args).map(CallResult::Value),
        NumpyFunctions::Ravel => call_ravel_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Delete => call_delete(vm, args).map(CallResult::Value),
        NumpyFunctions::Insert => call_insert(vm, args).map(CallResult::Value),
        NumpyFunctions::Diag => call_diag(vm, args).map(CallResult::Value),
        NumpyFunctions::Diagonal => call_diagonal(vm, args).map(CallResult::Value),
        NumpyFunctions::Trace => call_trace(vm, args).map(CallResult::Value),
        NumpyFunctions::Flatnonzero => call_flatnonzero(vm, args).map(CallResult::Value),
        NumpyFunctions::Asarray => call_asarray(vm, args).map(CallResult::Value),
        NumpyFunctions::ColumnStack => call_column_stack(vm, args).map(CallResult::Value),
        NumpyFunctions::RowStack => call_vstack(vm, args).map(CallResult::Value), // alias
        NumpyFunctions::Hsplit => call_hsplit(vm, args).map(CallResult::Value),
        NumpyFunctions::Vsplit => call_vsplit(vm, args).map(CallResult::Value),
        NumpyFunctions::ArraySplit => call_array_split(vm, args).map(CallResult::Value),
        NumpyFunctions::FullLike => call_full_like(vm, args).map(CallResult::Value),
        NumpyFunctions::EmptyLike => call_like(vm, args, 0.0, "numpy.empty_like").map(CallResult::Value),
        // Phase 7: Sorting, searching, set ops
        NumpyFunctions::ArgsortMod => call_argsort_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Searchsorted => call_searchsorted(vm, args).map(CallResult::Value),
        NumpyFunctions::Extract => call_extract(vm, args).map(CallResult::Value),
        NumpyFunctions::Intersect1d => {
            call_set_op(vm, args, SetOp::Intersect, "numpy.intersect1d").map(CallResult::Value)
        }
        NumpyFunctions::Union1d => call_set_op(vm, args, SetOp::Union, "numpy.union1d").map(CallResult::Value),
        NumpyFunctions::Setdiff1d => call_set_op(vm, args, SetOp::Diff, "numpy.setdiff1d").map(CallResult::Value),
        NumpyFunctions::Setxor1d => call_set_op(vm, args, SetOp::Xor, "numpy.setxor1d").map(CallResult::Value),
        NumpyFunctions::Bincount => call_bincount(vm, args).map(CallResult::Value),
        NumpyFunctions::Digitize => call_digitize(vm, args).map(CallResult::Value),
        // Phase 8: Linear algebra
        NumpyFunctions::Matmul => call_matmul(vm, args).map(CallResult::Value),
        NumpyFunctions::Inner => call_dot(vm, args).map(CallResult::Value), // For 1D, inner = dot
        NumpyFunctions::Outer => call_outer(vm, args).map(CallResult::Value),
        NumpyFunctions::Vdot => call_dot(vm, args).map(CallResult::Value), // vdot flattens first, same as dot for 1D
        NumpyFunctions::Cross => call_cross(vm, args).map(CallResult::Value),
        // Phase 10: Additional creation and numerical
        NumpyFunctions::Logspace => call_logspace(vm, args).map(CallResult::Value),
        NumpyFunctions::Geomspace => call_geomspace(vm, args).map(CallResult::Value),
        NumpyFunctions::Tri => call_tri(vm, args).map(CallResult::Value),
        NumpyFunctions::Tril => call_tril(vm, args).map(CallResult::Value),
        NumpyFunctions::Triu => call_triu(vm, args).map(CallResult::Value),
        NumpyFunctions::Identity => call_eye(vm, args).map(CallResult::Value), // alias
        NumpyFunctions::Meshgrid => call_meshgrid(vm, args).map(CallResult::Value),
        NumpyFunctions::Gradient => call_gradient(vm, args).map(CallResult::Value),
        NumpyFunctions::Convolve => call_convolve(vm, args).map(CallResult::Value),
        NumpyFunctions::Correlate => call_correlate(vm, args).map(CallResult::Value),
        NumpyFunctions::Interp => call_interp(vm, args).map(CallResult::Value),
        NumpyFunctions::Select => call_select(vm, args).map(CallResult::Value),
    }
}

// ===========================
// Array creation functions
// ===========================

/// `numpy.array(data)` — create an ndarray from a list or nested list.
fn call_array(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.array", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_list(arg, vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.zeros(shape)` — create an array of zeros with the given shape.
///
/// Accepts an integer for 1D or a tuple/list for multi-dimensional shapes.
fn call_zeros(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_ones(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_arange(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.arange", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let first = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.arange() requires at least 1 argument"))?;
    defer_drop!(first, vm);
    let second = pos.next();
    let third = pos.next();

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let (start, stop, step) = match (&second, &third) {
        (None, None) => (0.0, to_f64(first, vm)?, 1.0),
        (Some(stop_val), None) => (to_f64(first, vm)?, to_f64(stop_val, vm)?, 1.0),
        (Some(stop_val), Some(step_val)) => (to_f64(first, vm)?, to_f64(stop_val, vm)?, to_f64(step_val, vm)?),
        (None, Some(_)) => unreachable!("third arg without second"),
    };

    second.drop_with_heap(vm);
    third.drop_with_heap(vm);

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
fn call_linspace(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.linspace", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let start_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;
    defer_drop!(start_val, vm);
    let stop_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;
    defer_drop!(stop_val, vm);
    let num_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.linspace() requires 3 arguments"))?;
    defer_drop!(num_val, vm);

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let start = to_f64(start_val, vm)?;
    let stop = to_f64(stop_val, vm)?;
    let num = match num_val {
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
    vm: &mut VM<'_, impl ResourceTracker>,
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
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(&NdArray) -> RunResult<f64>,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return match arg {
            Value::Bool(_) | Value::Int(_) | Value::Float(_) => Ok(arg.clone_immediate()),
            _ => Err(ExcType::type_error(format!(
                "{name}() requires an array, list, or scalar argument"
            ))),
        };
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
fn list_to_ndarray(list: &List, name: &str) -> RunResult<NdArray> {
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
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64) -> f64,
    name: &str,
    result_dtype: Option<NdArrayDtype>,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        let (value, source_dtype) = numeric_scalar_info(arg, name, vm)?;
        let dtype = result_dtype.unwrap_or(source_dtype);
        return Ok(scalar_from_f64(f(value), dtype));
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
fn call_round(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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

    let factor = 10f64.powi(decimals);
    let Value::Ref(heap_id) = arr_val else {
        let (value, _) = numeric_scalar_info(arr_val, "numpy.round", vm)?;
        return Ok(Value::Float(round_to_decimals(value, factor)));
    };
    let (data, shape) = match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => (
            arr.data()
                .iter()
                .map(|&v| round_to_decimals(v, factor))
                .collect::<Vec<_>>(),
            arr.shape().to_vec(),
        ),
        HeapData::List(_) => {
            let arr = ndarray_from_list(arr_val, vm.heap)?;
            (
                arr.data()
                    .iter()
                    .map(|&v| round_to_decimals(v, factor))
                    .collect::<Vec<_>>(),
                arr.shape().to_vec(),
            )
        }
        _ => {
            return Err(ExcType::type_error(
                "numpy.round() requires an array, list, or scalar argument",
            ));
        }
    };

    let new_arr = NdArray::new(data, shape, NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.clip(a, a_min, a_max)` — clip (limit) array values to a range.
fn call_clip(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.clip", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;
    defer_drop!(arr_val, vm);
    let min_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;
    defer_drop!(min_val, vm);
    let max_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.clip() requires 3 arguments"))?;
    defer_drop!(max_val, vm);

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let a_min = to_f64(min_val, vm)?;
    let a_max = to_f64(max_val, vm)?;

    let Value::Ref(heap_id) = arr_val else {
        return Err(ExcType::type_error(
            "numpy.clip() requires an ndarray as the first argument",
        ));
    };
    let HeapData::NdArray(arr) = vm.heap.get(*heap_id) else {
        return Err(ExcType::type_error(
            "numpy.clip() requires an ndarray as the first argument",
        ));
    };

    let data: Vec<f64> = arr.data().iter().map(|&v| v.clamp(a_min, a_max)).collect();
    let dtype = arr.dtype();
    let shape = arr.shape().to_vec();

    let new_arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.where(condition, x, y)` — conditional element selection.
fn call_where(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.where", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let cond_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;
    defer_drop!(cond_val, vm);
    let x_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;
    defer_drop!(x_val, vm);
    let y_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.where() requires 3 arguments"))?;
    defer_drop!(y_val, vm);

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let Value::Ref(cond_id) = cond_val else {
        return Err(ExcType::type_error("numpy.where() condition must be an ndarray"));
    };
    let HeapData::NdArray(cond_arr) = vm.heap.get(*cond_id) else {
        return Err(ExcType::type_error("numpy.where() condition must be an ndarray"));
    };

    let cond_data: Vec<f64> = cond_arr.data().to_vec();
    let cond_shape = cond_arr.shape().to_vec();
    let len = cond_data.len();

    let x_data = extract_array_or_scalar(x_val, len, vm)?;
    let y_data = extract_array_or_scalar(y_val, len, vm)?;

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
fn extract_array_or_scalar(val: &Value, len: usize, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Vec<f64>> {
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
    vm: &mut VM<'_, impl ResourceTracker>,
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

/// Result dtype policy for NumPy binary ufunc-style helpers.
#[derive(Clone, Copy)]
enum BinopResult {
    /// Preserve NumPy-like int/float promotion for arithmetic operations.
    Promoted,
    /// Force float output, as true division does.
    Float,
    /// Force boolean output for comparison ufuncs.
    Bool,
}

/// Shared implementation for common binary NumPy ufuncs.
///
/// Supports ndarray, list, and scalar inputs. Full NumPy broadcasting is out of
/// scope for Monty's current ndarray model, but scalar broadcasting and equal
/// shaped arrays cover the common LLM-generated snippets these wrappers target.
fn call_numeric_binop(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64, f64) -> f64,
    name: &str,
    result: BinopResult,
) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_info = extract_ndarray_info(a_val, name, vm);
    let b_info = extract_ndarray_info(b_val, name, vm);

    match (a_info, b_info) {
        (Ok((a_data, a_shape, a_dtype)), Ok((b_data, b_shape, b_dtype))) => {
            if a_shape != b_shape {
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
                );
            }
            let data: Vec<f64> = a_data.iter().zip(b_data.iter()).map(|(&a, &b)| f(a, b)).collect();
            let dtype = binop_dtype(result, a_dtype, b_dtype);
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Ok((a_data, a_shape, a_dtype)), Err(_)) => {
            let (scalar, scalar_dtype) = numeric_scalar_info(b_val, name, vm)?;
            let data: Vec<f64> = a_data.iter().map(|&a| f(a, scalar)).collect();
            let dtype = binop_dtype(result, a_dtype, scalar_dtype);
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Ok((b_data, b_shape, b_dtype))) => {
            let (scalar, scalar_dtype) = numeric_scalar_info(a_val, name, vm)?;
            let data: Vec<f64> = b_data.iter().map(|&b| f(scalar, b)).collect();
            let dtype = binop_dtype(result, scalar_dtype, b_dtype);
            let arr = NdArray::new(data, b_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Err(_)) => {
            let (a, a_dtype) = numeric_scalar_info(a_val, name, vm)?;
            let (b, b_dtype) = numeric_scalar_info(b_val, name, vm)?;
            let dtype = binop_dtype(result, a_dtype, b_dtype);
            Ok(scalar_from_f64(f(a, b), dtype))
        }
    }
}

/// Computes the dtype for a binary ufunc result from the operation policy.
fn binop_dtype(result: BinopResult, a: NdArrayDtype, b: NdArrayDtype) -> NdArrayDtype {
    match result {
        BinopResult::Promoted => promote_dtype(a, b),
        BinopResult::Float => NdArrayDtype::Float64,
        BinopResult::Bool => NdArrayDtype::Bool,
    }
}

/// Python-compatible modulo: result has the same sign as the divisor.
fn py_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r > 0.0) != (b > 0.0)) { r + b } else { r }
}

/// Converts a boolean comparison result to the f64 backing value for bool arrays.
fn bool_to_f64(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

/// Equality comparison for NumPy-style numeric ufuncs.
///
/// `partial_cmp` preserves NumPy's NaN behavior without using direct float
/// equality: NaN does not compare equal to itself.
fn eq_to_f64(a: f64, b: f64) -> f64 {
    bool_to_f64(a.partial_cmp(&b) == Some(Ordering::Equal))
}

/// Inequality comparison for NumPy-style numeric ufuncs.
fn ne_to_f64(a: f64, b: f64) -> f64 {
    bool_to_f64(a.partial_cmp(&b) != Some(Ordering::Equal))
}

// ===========================
// Sorting and unique functions
// ===========================

/// `numpy.sort(a)` — return a sorted copy of the array.
fn call_sort(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
    data.sort_by(nan_last_cmp);
    let new_arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.unique(a)` — return the sorted unique elements of an array.
fn call_unique(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
    data.sort_by(nan_last_cmp);
    data.dedup();
    let len = data.len();
    let new_arr = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(new_arr))?))
}

/// `numpy.concatenate(arrays)` — join a sequence of arrays along the first axis.
fn call_concatenate(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_cumsum(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_dot(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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

/// `numpy.matmul(a, b)` — matrix multiplication (like `a @ b`).
///
/// Supports 1D-1D (dot product), 2D-2D (matrix multiply), 2D-1D and 1D-2D products.
fn call_matmul(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.matmul", vm.heap)?;
    defer_drop!(a_val, vm);

    let Value::Ref(a_id) = a_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.matmul() requires ndarray arguments"));
    };
    let Value::Ref(b_id) = &b_val else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.matmul() requires ndarray arguments"));
    };
    let b_id = *b_id;

    let HeapData::NdArray(a_arr) = vm.heap.get(*a_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.matmul() requires ndarray arguments"));
    };
    let HeapData::NdArray(b_arr) = vm.heap.get(b_id) else {
        b_val.drop_with_heap(vm);
        return Err(ExcType::type_error("numpy.matmul() requires ndarray arguments"));
    };

    let result = a_arr.matmul(b_arr, vm.heap);
    b_val.drop_with_heap(vm);
    result
}

// ===========================
// Element-wise math, array creation, testing, aggregation, manipulation,
// search, and utility functions
// ===========================

/// `numpy.power(a, b)` — element-wise power (like `a ** b`).
///
/// Supports array-array, array-scalar, and scalar-array combinations.
fn call_power(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
            let dtype = promote_dtype_with_scalar(a_dtype, is_float);
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        // First is scalar, second is array
        (Err(_), Ok((b_data, b_shape, b_dtype))) => {
            let scalar = to_f64(a_val, vm)?;
            let is_float = matches!(a_val, Value::Float(_));
            let data: Vec<f64> = b_data.iter().map(|&b| scalar.powf(b)).collect();
            let dtype = promote_dtype_with_scalar(b_dtype, is_float);
            let arr = NdArray::new(data, b_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        // Neither is array
        (Err(e), _) => Err(e),
    }
}

/// `numpy.diff(a)` — first-order discrete difference: `a[1:] - a[:-1]`.
fn call_diff(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_full(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_eye(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_copy(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.copy", vm.heap)?;
    defer_drop!(arg, vm);
    let Value::Ref(heap_id) = arg else {
        return Err(ExcType::type_error("numpy.copy() requires an array or list"));
    };
    let result = match vm.heap.get(*heap_id) {
        HeapData::NdArray(arr) => NdArray::new(arr.data().to_vec(), arr.shape().to_vec(), arr.dtype()),
        HeapData::List(_) => {
            // Use ndarray_from_list which handles proper dtype tracking
            ndarray_from_list(arg, vm.heap)?
        }
        _ => return Err(ExcType::type_error("numpy.copy() requires an array or list")),
    };
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.empty(shape)` — create an uninitialized array (returns zeros in Monty).
fn call_empty(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.empty", vm.heap)?;
    let shape = extract_shape(arg, "numpy.empty", vm)?;
    let total: usize = shape.iter().product();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let arr = NdArray::new(vec![0.0; total], shape, NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// Helper for `numpy.zeros_like(a)` and `numpy.ones_like(a)`.
fn call_like(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, fill: f64, name: &str) -> RunResult<Value> {
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
    vm: &mut VM<'_, impl ResourceTracker>,
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
fn call_array_equal(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.array_equal", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_arr = ndarray_from_value(a_val, "numpy.array_equal", vm)?;
    let b_arr = ndarray_from_value(b_val, "numpy.array_equal", vm)?;

    let equal = a_arr.shape() == b_arr.shape() && a_arr.data() == b_arr.data();
    Ok(Value::Bool(equal))
}

/// `numpy.count_nonzero(a)` — count non-zero elements.
fn call_count_nonzero(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.count_nonzero", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.count_nonzero", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "count won't exceed i64::MAX")]
    let count = arr.data().iter().filter(|&&v| v != 0.0).count() as i64;
    Ok(Value::Int(count))
}

/// `numpy.all(a)` — true if all elements are truthy (module-level wrapper).
fn call_all(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.all", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.all", vm)?;
    Ok(Value::Bool(arr.all()))
}

/// `numpy.any(a)` — true if any element is truthy (module-level wrapper).
fn call_any(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.any", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.any", vm)?;
    Ok(Value::Bool(arr.any()))
}

/// `numpy.prod(a)` — product of array elements.
fn call_prod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_median(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.median", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.median", vm)?;
    if arr.len() == 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "zero-size array has no median").into());
    }
    let mut sorted = arr.data().to_vec();
    sorted.sort_by(nan_last_cmp);
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 0 {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    };
    Ok(Value::Float(median))
}

/// `numpy.argmin(a)` — index of minimum element (module-level wrapper).
fn call_argmin_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argmin", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argmin", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
    Ok(Value::Int(arr.argmin()? as i64))
}

/// `numpy.argmax(a)` — index of maximum element (module-level wrapper).
fn call_argmax_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argmax", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argmax", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
    Ok(Value::Int(arr.argmax()? as i64))
}

/// `numpy.reshape(a, shape)` — reshape an array (module-level wrapper).
fn call_reshape_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.reshape", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.reshape() requires 2 arguments"))?;
    defer_drop!(arr_val, vm);
    let shape_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.reshape() requires 2 arguments"))?;
    defer_drop!(shape_val, vm);

    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let shape = extract_shape_from_value(shape_val, "numpy.reshape", vm)?;

    let arr = ndarray_from_value(arr_val, "numpy.reshape", vm)?;
    arr.reshape(shape, vm.heap)
}

/// `numpy.transpose(a)` — transpose an array (module-level wrapper).
fn call_transpose_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.transpose", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.transpose", vm)?;
    arr.transpose(vm.heap)
}

/// `numpy.append(a, values)` — append values to end of array (flattened).
fn call_append(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_vstack(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_hstack(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    call_concatenate(vm, args)
}

/// `numpy.nonzero(a)` — indices of non-zero elements, returned as a tuple of arrays.
fn call_nonzero(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_argwhere(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_tile(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_repeat(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
fn call_split(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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

    let list = List::new(parts);
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(list))?))
}

/// `numpy.shape(a)` — return the dimensions of an array-like value.
fn call_shape(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.shape", vm.heap)?;
    defer_drop!(arg, vm);
    let shape = array_like_shape(arg, "numpy.shape", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "shape dimensions won't exceed i64::MAX")]
    let values: SmallVec<[Value; 3]> = shape.iter().map(|&d| Value::Int(d as i64)).collect();
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// `numpy.size(a)` — return the total number of elements in an array-like value.
fn call_size(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.size", vm.heap)?;
    defer_drop!(arg, vm);
    let shape = array_like_shape(arg, "numpy.size", vm)?;
    let size = shape.iter().product::<usize>();
    #[expect(clippy::cast_possible_wrap, reason = "array sizes are resource-limited")]
    Ok(Value::Int(size as i64))
}

/// `numpy.ndim(a)` — return the number of dimensions in an array-like value.
fn call_ndim(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ndim", vm.heap)?;
    defer_drop!(arg, vm);
    let shape = array_like_shape(arg, "numpy.ndim", vm)?;
    #[expect(clippy::cast_possible_wrap, reason = "ndim is always small")]
    Ok(Value::Int(shape.len() as i64))
}

/// Returns the shape for ndarray/list inputs and the scalar shape for numbers.
fn array_like_shape(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Vec<usize>> {
    if let Ok((_, shape, _)) = extract_ndarray_info(value, name, vm) {
        Ok(shape)
    } else {
        numeric_scalar_info(value, name, vm)?;
        Ok(Vec::new())
    }
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
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<(Vec<f64>, Vec<usize>, NdArrayDtype)> {
    match value {
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::NdArray(arr) => Ok((arr.data().to_vec(), arr.shape().to_vec(), arr.dtype())),
            HeapData::List(_) => {
                let tmp = ndarray_from_list(value, vm.heap)?;
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
fn ndarray_from_value(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<NdArray> {
    let (data, shape, dtype) = extract_ndarray_info(value, name, vm)?;
    Ok(NdArray::new(data, shape, dtype))
}

/// Extracts a shape from a Value — supports int (1D), list, or tuple.
fn extract_shape(value: Value, func_name: &str, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Vec<usize>> {
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
    vm: &VM<'_, impl ResourceTracker>,
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
fn extract_size(value: Value, func_name: &str, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<usize> {
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
fn to_f64(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunResult<f64> {
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

/// Converts a Python numeric scalar to the internal f64 value plus NumPy dtype.
///
/// This is used by scalar-compatible ufunc-style helpers, where real NumPy
/// accepts both arrays and scalars. Non-numeric values still raise the same
/// Monty type error style as the array path.
fn numeric_scalar_info(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<(f64, NdArrayDtype)> {
    match value {
        Value::Int(n) => Ok((*n as f64, NdArrayDtype::Int64)),
        Value::Float(f) => Ok((*f, NdArrayDtype::Float64)),
        Value::Bool(b) => Ok((if *b { 1.0 } else { 0.0 }, NdArrayDtype::Bool)),
        _ => Err(ExcType::type_error(format!(
            "{name}() requires an array, list, or scalar argument, not '{}'",
            value.py_type(vm)
        ))),
    }
}

/// Converts an internal f64 result back to the best scalar value for a dtype.
///
/// Integer and boolean scalar results mirror Monty's existing ndarray display
/// conversion: the f64 backing value is truncated for integer dtypes and
/// non-zero values are truthy for boolean dtypes.
fn scalar_from_f64(value: f64, dtype: NdArrayDtype) -> Value {
    match dtype {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "scalar conversion follows ndarray integer element conversion"
        )]
        NdArrayDtype::Int64 => Value::Int(value as i64),
        NdArrayDtype::Float64 => Value::Float(value),
        NdArrayDtype::Bool => Value::Bool(value != 0.0),
    }
}

/// `numpy.conj(a)` / `numpy.real(a)` for Monty's real-valued ndarray subset.
///
/// Monty does not currently model complex numbers, so the conjugate and real
/// component are identical to the input. Lists are converted to ndarrays, while
/// numeric scalars keep their scalar shape and dtype.
fn call_real_identity(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, name: &str) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, dtype)) = extract_ndarray_info(arg, name, vm) {
        let arr = NdArray::new(data, shape, dtype);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let (value, dtype) = numeric_scalar_info(arg, name, vm)?;
        Ok(scalar_from_f64(value, dtype))
    }
}

/// `numpy.imag(a)` for Monty's real-valued ndarray subset.
///
/// Since complex dtypes are unsupported, every supported numeric input has a
/// zero imaginary component. The result preserves array shape and scalar-vs-array
/// form so common NumPy introspection snippets continue to work.
fn call_imag(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.imag", vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, dtype)) = extract_ndarray_info(arg, "numpy.imag", vm) {
        let arr = NdArray::new(vec![0.0; data.len()], shape, dtype);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let (_, dtype) = numeric_scalar_info(arg, "numpy.imag", vm)?;
        Ok(scalar_from_f64(0.0, dtype))
    }
}

/// Element-wise `numpy.isreal()` / `numpy.iscomplex()` over real-only inputs.
///
/// The safe ndarray model has no complex dtype, so every numeric element is real
/// and no numeric element is complex. Non-numeric object arrays remain outside
/// this module's supported surface.
fn call_realness_elementwise(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    is_real: bool,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, _)) = extract_ndarray_info(arg, name, vm) {
        let fill = bool_to_f64(is_real);
        let arr = NdArray::new(vec![fill; data.len()], shape, NdArrayDtype::Bool);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        numeric_scalar_info(arg, name, vm)?;
        Ok(Value::Bool(is_real))
    }
}

/// Object-level `numpy.isrealobj()` / `numpy.iscomplexobj()`.
///
/// Monty cannot construct complex arrays or scalars, so these predicates are
/// constant for the current runtime surface. The argument is still consumed and
/// dropped normally to preserve reference-count behavior.
fn call_realness_object(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    is_real: bool,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    arg.drop_with_heap(vm);
    Ok(Value::Bool(is_real))
}

/// `numpy.isscalar(a)` — report whether a value is scalar in Monty's runtime.
///
/// Numeric values, strings/bytes, dates, timedeltas, and long integers are
/// scalar-like; containers, arrays, modules, functions, and sentinel values are
/// not. This intentionally avoids invoking user-visible iteration or attribute
/// lookup, so it remains a pure shape/type predicate.
fn call_isscalar(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.isscalar", vm.heap)?;
    defer_drop!(arg, vm);
    Ok(Value::Bool(is_numpy_scalar(arg, vm)))
}

/// `numpy.iterable(a)` — report whether Monty's iterator protocol accepts a value.
fn call_iterable(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.iterable", vm.heap)?;
    defer_drop!(arg, vm);
    Ok(Value::Bool(is_numpy_iterable(arg, vm)))
}

/// Returns whether a value should be treated as scalar by `numpy.isscalar()`.
fn is_numpy_scalar(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::InternString(_)
        | Value::InternBytes(_)
        | Value::InternLongInt(_) => true,
        Value::Ref(heap_id) => matches!(
            vm.heap.get(*heap_id),
            HeapData::LongInt(_)
                | HeapData::Str(_)
                | HeapData::Bytes(_)
                | HeapData::Date(_)
                | HeapData::DateTime(_)
                | HeapData::TimeDelta(_)
                | HeapData::TimeZone(_)
        ),
        _ => false,
    }
}

/// Returns whether a value can be iterated by Monty's iterator protocol.
fn is_numpy_iterable(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::InternString(_) | Value::InternBytes(_) => true,
        Value::Ref(heap_id) => matches!(
            vm.heap.get(*heap_id),
            HeapData::List(_)
                | HeapData::Tuple(_)
                | HeapData::NamedTuple(_)
                | HeapData::Dict(_)
                | HeapData::DictKeysView(_)
                | HeapData::DictItemsView(_)
                | HeapData::DictValuesView(_)
                | HeapData::Set(_)
                | HeapData::FrozenSet(_)
                | HeapData::Range(_)
                | HeapData::Iter(_)
                | HeapData::Str(_)
                | HeapData::Bytes(_)
                | HeapData::NdArray(_)
        ),
        _ => false,
    }
}

/// Rounds a scalar using the factor computed from NumPy's `decimals` argument.
fn round_to_decimals(value: f64, factor: f64) -> f64 {
    (value * factor).round() / factor
}

// ===========================
// Phase 3+: Additional math, aggregation, logical, manipulation,
// sorting, set, linalg, and creation functions
// ===========================

/// `numpy.nan_to_num(a)` — replace NaN with 0, inf with large finite, -inf with -large finite.
fn call_nan_to_num(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.nan_to_num", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.nan_to_num", vm)?;
    let big = f64::MAX;
    let data: Vec<f64> = arr
        .data()
        .iter()
        .map(|&v| {
            if v.is_nan() {
                0.0
            } else if v == f64::INFINITY {
                big
            } else if v == f64::NEG_INFINITY {
                -big
            } else {
                v
            }
        })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- NaN-aware aggregation helpers ---

/// Filter NaN values from a slice, returning only finite values.
fn filter_nan(data: &[f64]) -> Vec<f64> {
    data.iter().copied().filter(|v| !v.is_nan()).collect()
}

fn nan_sum(data: &[f64]) -> f64 {
    filter_nan(data).iter().sum()
}
fn nan_prod(data: &[f64]) -> f64 {
    filter_nan(data).iter().fold(1.0, |a, &v| a * v)
}
fn nan_mean(data: &[f64]) -> f64 {
    let clean = filter_nan(data);
    if clean.is_empty() {
        f64::NAN
    } else {
        clean.iter().sum::<f64>() / clean.len() as f64
    }
}
fn nan_min(data: &[f64]) -> f64 {
    filter_nan(data).iter().copied().fold(f64::INFINITY, f64::min)
}
fn nan_max(data: &[f64]) -> f64 {
    filter_nan(data).iter().copied().fold(f64::NEG_INFINITY, f64::max)
}
fn nan_var(data: &[f64]) -> f64 {
    let clean = filter_nan(data);
    if clean.is_empty() {
        return f64::NAN;
    }
    let mean = clean.iter().sum::<f64>() / clean.len() as f64;
    clean.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / clean.len() as f64
}
fn nan_std(data: &[f64]) -> f64 {
    nan_var(data).sqrt()
}
fn nan_median(data: &[f64]) -> f64 {
    let mut clean = filter_nan(data);
    if clean.is_empty() {
        return f64::NAN;
    }
    clean.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = clean.len();
    if n % 2 == 1 {
        clean[n / 2]
    } else {
        f64::midpoint(clean[n / 2 - 1], clean[n / 2])
    }
}

/// Generic NaN-aware aggregation: extract array, filter NaN, apply function, return float.
fn call_nan_aggregate(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(&[f64]) -> f64,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, name, vm)?;
    Ok(Value::Float(f(arr.data())))
}

/// `numpy.nanargmin(a)` — index of minimum, ignoring NaN.
#[expect(
    clippy::cast_possible_wrap,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_nan_argmin(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.nanargmin", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.nanargmin", vm)?;
    let mut best_idx = 0usize;
    let mut best_val = f64::INFINITY;
    for (i, &v) in arr.data().iter().enumerate() {
        if !v.is_nan() && v < best_val {
            best_val = v;
            best_idx = i;
        }
    }
    Ok(Value::Int(best_idx as i64))
}

/// `numpy.nanargmax(a)` — index of maximum, ignoring NaN.
#[expect(
    clippy::cast_possible_wrap,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_nan_argmax(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.nanargmax", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.nanargmax", vm)?;
    let mut best_idx = 0usize;
    let mut best_val = f64::NEG_INFINITY;
    for (i, &v) in arr.data().iter().enumerate() {
        if !v.is_nan() && v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    Ok(Value::Int(best_idx as i64))
}

/// `numpy.percentile(a, q)` — q-th percentile (q in 0..100).
fn call_percentile(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, q_val) = args.get_two_args("numpy.percentile", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.percentile", vm)?;
    let q = to_f64(&q_val, vm)?;
    q_val.drop_with_heap(vm);
    Ok(Value::Float(percentile_impl(arr.data(), q / 100.0)))
}

/// `numpy.quantile(a, q)` — q-th quantile (q in 0..1).
fn call_quantile(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, q_val) = args.get_two_args("numpy.quantile", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.quantile", vm)?;
    let q = to_f64(&q_val, vm)?;
    q_val.drop_with_heap(vm);
    Ok(Value::Float(percentile_impl(arr.data(), q)))
}

/// Compute the q-th quantile (q in [0, 1]) using linear interpolation.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "quantile index is always within array bounds"
)]
fn percentile_impl(data: &[f64], q: f64) -> f64 {
    if data.is_empty() {
        return f64::NAN;
    }
    let mut sorted: Vec<f64> = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let idx = q * (n - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
    }
}

/// `numpy.ptp(a)` — peak-to-peak: max(a) - min(a).
fn call_ptp(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ptp", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.ptp", vm)?;
    let (min, max) = arr
        .data()
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), &v| {
            (mn.min(v), mx.max(v))
        });
    Ok(Value::Float(max - min))
}

/// `numpy.cumprod(a)` — cumulative product.
fn call_cumprod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.cumprod", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.cumprod", vm)?;
    let mut acc = 1.0;
    let data: Vec<f64> = arr
        .data()
        .iter()
        .map(|&v| {
            acc *= v;
            acc
        })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.nancumsum` / `numpy.nancumprod` — cumulative ops treating NaN as identity.
fn call_nancumop(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, is_sum: bool, name: &str) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, name, vm)?;
    let identity = if is_sum { 0.0 } else { 1.0 };
    let mut acc = identity;
    let data: Vec<f64> = arr
        .data()
        .iter()
        .map(|&v| {
            let clean = if v.is_nan() { identity } else { v };
            if is_sum {
                acc += clean;
            } else {
                acc *= clean;
            }
            acc
        })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- Phase 5: Logical and testing ---

/// Generic logical binary operation on two arrays → Bool result.
fn call_logical_binop(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    op: fn(bool, bool) -> bool,
    name: &str,
) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a = ndarray_from_value(a_val, name, vm)?;
    let b = ndarray_from_value(b_val, name, vm)?;
    let data: Vec<f64> = a
        .data()
        .iter()
        .zip(b.data().iter())
        .map(|(&x, &y)| if op(x != 0.0, y != 0.0) { 1.0 } else { 0.0 })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Bool);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.logical_not(a)` — element-wise logical NOT.
fn call_logical_not(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.logical_not", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.logical_not", vm)?;
    let data: Vec<f64> = arr.data().iter().map(|&v| if v == 0.0 { 1.0 } else { 0.0 }).collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Bool);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.allclose(a, b, rtol=1e-5, atol=1e-8)` — true if all elements are close.
fn call_allclose(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.allclose", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let a_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.allclose() requires at least 2 arguments"))?;
    defer_drop!(a_val, vm);
    let b_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.allclose() requires at least 2 arguments"))?;
    defer_drop!(b_val, vm);
    let rtol = pos
        .next()
        .map(|v| {
            let result = to_f64(&v, vm);
            v.drop_with_heap(vm);
            result
        })
        .transpose()?
        .unwrap_or(1e-5);
    let atol = pos
        .next()
        .map(|v| {
            let result = to_f64(&v, vm);
            v.drop_with_heap(vm);
            result
        })
        .transpose()?
        .unwrap_or(1e-8);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let a = ndarray_from_value(a_val, "numpy.allclose", vm)?;
    let b = ndarray_from_value(b_val, "numpy.allclose", vm)?;
    let close = a
        .data()
        .iter()
        .zip(b.data().iter())
        .all(|(&x, &y)| (x - y).abs() <= atol + rtol * y.abs());
    Ok(Value::Bool(close))
}

/// `numpy.isclose(a, b, rtol=1e-5, atol=1e-8)` — element-wise closeness test.
fn call_isclose(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.isclose", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let a_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.isclose() requires at least 2 arguments"))?;
    defer_drop!(a_val, vm);
    let b_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.isclose() requires at least 2 arguments"))?;
    defer_drop!(b_val, vm);
    let rtol = pos
        .next()
        .map(|v| {
            let result = to_f64(&v, vm);
            v.drop_with_heap(vm);
            result
        })
        .transpose()?
        .unwrap_or(1e-5);
    let atol = pos
        .next()
        .map(|v| {
            let result = to_f64(&v, vm);
            v.drop_with_heap(vm);
            result
        })
        .transpose()?
        .unwrap_or(1e-8);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let a = ndarray_from_value(a_val, "numpy.isclose", vm)?;
    let b = ndarray_from_value(b_val, "numpy.isclose", vm)?;
    let data: Vec<f64> = a
        .data()
        .iter()
        .zip(b.data().iter())
        .map(|(&x, &y)| {
            if (x - y).abs() <= atol + rtol * y.abs() {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Bool);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.isin(element, test_elements)` — test membership.
fn call_isin(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (elem_val, test_val) = args.get_two_args("numpy.isin", vm.heap)?;
    defer_drop!(elem_val, vm);
    defer_drop!(test_val, vm);
    let elems = ndarray_from_value(elem_val, "numpy.isin", vm)?;
    let tests = ndarray_from_value(test_val, "numpy.isin", vm)?;
    let test_set: Vec<f64> = tests.data().to_vec();
    let data: Vec<f64> = elems
        .data()
        .iter()
        .map(|&v| if test_set.contains(&v) { 1.0 } else { 0.0 })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Bool);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- Phase 6: Manipulation and shape ---

/// `numpy.flip(a)` — reverse array elements.
fn call_flip(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.flip", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.flip", vm)?;
    let mut data = arr.data().to_vec();
    data.reverse();
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.fliplr(a)` — flip left-right. For 2D: reverse each row.
fn call_fliplr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.fliplr", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.fliplr", vm)?;
    if arr.shape().len() < 2 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "Input must be >= 2-d.").into());
    }
    let cols = arr.shape()[1];
    let mut data = arr.data().to_vec();
    for row in data.chunks_mut(cols) {
        row.reverse();
    }
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.flipud(a)` — flip up-down. For 2D: reverse row order.
fn call_flipud(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.flipud", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.flipud", vm)?;
    if arr.shape().len() < 2 {
        // For 1D, flipud is just reverse
        let mut data = arr.data().to_vec();
        data.reverse();
        let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }
    let cols = arr.shape()[1];
    let mut rows: Vec<&[f64]> = arr.data().chunks(cols).collect();
    rows.reverse();
    let data: Vec<f64> = rows.into_iter().flatten().copied().collect();
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.roll(a, shift)` — roll elements by `shift` positions.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_roll(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, shift_val) = args.get_two_args("numpy.roll", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.roll", vm)?;
    let Value::Int(shift) = &shift_val else {
        shift_val.drop_with_heap(vm);
        return Err(ExcType::type_error("shift must be integer"));
    };
    let shift = *shift;
    shift_val.drop_with_heap(vm);
    let data = arr.data();
    let n = data.len();
    if n == 0 {
        let result = NdArray::new(Vec::new(), vec![0], arr.dtype());
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }
    let shift = ((shift % n as i64) + n as i64) as usize % n;
    let mut new_data = Vec::with_capacity(n);
    new_data.extend_from_slice(&data[n - shift..]);
    new_data.extend_from_slice(&data[..n - shift]);
    let result = NdArray::new(new_data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.expand_dims(a, axis)` — insert a new axis at `axis`.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_expand_dims(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, axis_val) = args.get_two_args("numpy.expand_dims", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.expand_dims", vm)?;
    let Value::Int(axis) = &axis_val else {
        axis_val.drop_with_heap(vm);
        return Err(ExcType::type_error("axis must be integer"));
    };
    let axis = *axis;
    axis_val.drop_with_heap(vm);
    let mut shape = arr.shape().to_vec();
    let ndim = shape.len() as i64 + 1;
    let axis = if axis < 0 {
        (axis + ndim).max(0) as usize
    } else {
        axis.min(ndim - 1) as usize
    };
    shape.insert(axis, 1);
    let result = NdArray::new(arr.data().to_vec(), shape, arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.squeeze(a)` — remove length-1 axes.
fn call_squeeze(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.squeeze", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.squeeze", vm)?;
    let shape: Vec<usize> = arr.shape().iter().copied().filter(|&s| s != 1).collect();
    let shape = if shape.is_empty() { vec![1] } else { shape };
    let result = NdArray::new(arr.data().to_vec(), shape, arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.ravel(a)` — module-level flatten.
fn call_ravel_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ravel", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.ravel", vm)?;
    let len = arr.data().len();
    let result = NdArray::new(arr.data().to_vec(), vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.delete(arr, indices)` — delete elements at given indices.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_delete(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, idx_val) = args.get_two_args("numpy.delete", vm.heap)?;
    defer_drop!(arr_val, vm);
    defer_drop!(idx_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.delete", vm)?;
    let n = arr.data().len();
    // Build set of indices to delete
    let del_indices: Vec<usize> = if let Value::Int(i) = idx_val {
        let i = if *i < 0 { (*i + n as i64) as usize } else { *i as usize };
        vec![i]
    } else {
        let idx_arr = ndarray_from_value(idx_val, "numpy.delete", vm)?;
        idx_arr
            .data()
            .iter()
            .map(|&v| if v < 0.0 { (v + n as f64) as usize } else { v as usize })
            .collect()
    };
    let data: Vec<f64> = arr
        .data()
        .iter()
        .enumerate()
        .filter(|(i, _)| !del_indices.contains(i))
        .map(|(_, &v)| v)
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.insert(arr, index, values)` — insert values before the given index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_insert(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.insert", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.insert() requires 3 arguments"))?;
    defer_drop!(arr_val, vm);
    let idx_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.insert() requires 3 arguments"))?;
    defer_drop!(idx_val, vm);
    let vals_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.insert() requires 3 arguments"))?;
    defer_drop!(vals_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let arr = ndarray_from_value(arr_val, "numpy.insert", vm)?;
    let Value::Int(idx) = idx_val else {
        return Err(ExcType::type_error("index must be integer"));
    };
    let idx = *idx as usize;
    // vals_val can be a scalar or an array
    let (vals_data, vals_dtype) = match vals_val {
        Value::Float(f) => (vec![*f], NdArrayDtype::Float64),
        Value::Int(n) => (vec![*n as f64], NdArrayDtype::Int64),
        _ => {
            let v = ndarray_from_value(vals_val, "numpy.insert", vm)?;
            (v.data().to_vec(), v.dtype())
        }
    };
    let mut data = arr.data().to_vec();
    let insert_at = idx.min(data.len());
    for (i, &v) in vals_data.iter().enumerate() {
        data.insert(insert_at + i, v);
    }
    let len = data.len();
    let dtype = promote_dtype(arr.dtype(), vals_dtype);
    let result = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.diag(v)` — for 1D input: create diagonal matrix. For 2D input: extract diagonal.
fn call_diag(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.diag", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.diag", vm)?;
    if arr.shape().len() == 1 {
        // Create diagonal matrix
        let n = arr.data().len();
        check_array_alloc_size(n * n, vm.heap.tracker())?;
        let mut data = vec![0.0; n * n];
        for (i, &v) in arr.data().iter().enumerate() {
            data[i * n + i] = v;
        }
        let result = NdArray::new(data, vec![n, n], arr.dtype());
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
    } else {
        // Extract diagonal from 2D
        let rows = arr.shape()[0];
        let cols = arr.shape()[1];
        let n = rows.min(cols);
        let data: Vec<f64> = (0..n).map(|i| arr.data()[i * cols + i]).collect();
        let result = NdArray::new(data, vec![n], arr.dtype());
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
    }
}

/// `numpy.diagonal(a)` — extract diagonal of 2D array.
fn call_diagonal(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // For our purposes, same as diag on 2D
    call_diag(vm, args)
}

/// `numpy.trace(a)` — sum of diagonal elements.
fn call_trace(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.trace", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.trace", vm)?;
    if arr.shape().len() < 2 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "trace requires 2-d array").into());
    }
    let cols = arr.shape()[1];
    let n = arr.shape()[0].min(cols);
    let sum: f64 = (0..n).map(|i| arr.data()[i * cols + i]).sum();
    Ok(Value::Float(sum))
}

/// `numpy.flatnonzero(a)` — indices of non-zero elements in flattened array.
fn call_flatnonzero(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.flatnonzero", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.flatnonzero", vm)?;
    let data: Vec<f64> = arr
        .data()
        .iter()
        .enumerate()
        .filter(|&(_, v)| *v != 0.0)
        .map(|(i, _)| i as f64)
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.asarray(a)` — convert a list or ndarray to an ndarray.
///
/// Monty does not currently model NumPy views, so ndarray input is copied rather
/// than returned as the identical object. The observable numeric contents, shape,
/// and dtype are preserved for the safe ndarray subset.
fn call_asarray(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.asarray", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.asarray", vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.column_stack(arrays)` — stack 1D arrays as columns into 2D.
fn call_column_stack(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let list_val = args.get_one_arg("numpy.column_stack", vm.heap)?;
    defer_drop!(list_val, vm);
    let list_items = match list_val {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => list
                .as_slice()
                .iter()
                .map(|v| v.clone_with_heap(vm))
                .collect::<Vec<_>>(),
            _ => return Err(ExcType::type_error("numpy.column_stack() requires a list")),
        },
        _ => return Err(ExcType::type_error("numpy.column_stack() requires a list")),
    };
    if list_items.is_empty() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "need at least one array to stack").into());
    }
    // Extract all arrays
    let mut arrays: Vec<NdArray> = Vec::new();
    for item in &list_items {
        arrays.push(ndarray_from_value(item, "numpy.column_stack", vm)?);
    }
    for item in list_items {
        item.drop_with_heap(vm);
    }
    let rows = arrays[0].data().len();
    let cols = arrays.len();
    check_array_alloc_size(rows * cols, vm.heap.tracker())?;
    let mut data = vec![0.0; rows * cols];
    for (c, arr) in arrays.iter().enumerate() {
        for (r, &v) in arr.data().iter().enumerate() {
            data[r * cols + c] = v;
        }
    }
    let dtype = arrays
        .iter()
        .fold(NdArrayDtype::Int64, |d, a| promote_dtype(d, a.dtype()));
    let result = NdArray::new(data, vec![rows, cols], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.hsplit(a, n)` — split horizontally (for 1D: split into n parts).
fn call_hsplit(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // For 1D, hsplit is same as split
    call_split(vm, args)
}

/// `numpy.vsplit(a, n)` — split vertically.
fn call_vsplit(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    call_split(vm, args)
}

/// `numpy.array_split(a, n)` — split into possibly unequal parts.
fn call_array_split(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, n_val) = args.get_two_args("numpy.array_split", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.array_split", vm)?;
    let Value::Int(n) = &n_val else {
        n_val.drop_with_heap(vm);
        return Err(ExcType::type_error("sections must be integer"));
    };
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "sections from user"
    )]
    let n = *n as usize;
    n_val.drop_with_heap(vm);
    if n == 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "number sections must be larger than 0").into());
    }
    let data = arr.data();
    let dtype = arr.dtype();
    let total = data.len();
    let base_size = total / n;
    let remainder = total % n;
    let mut parts = Vec::new();
    let mut offset = 0;
    for i in 0..n {
        let size = base_size + usize::from(i < remainder);
        let chunk = data[offset..offset + size].to_vec();
        let len = chunk.len();
        parts.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(NdArray::new(
            chunk,
            vec![len],
            dtype,
        )))?));
        offset += size;
    }
    let list = List::new(parts);
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(list))?))
}

/// `numpy.full_like(a, fill_value)` — array of same shape filled with value.
fn call_full_like(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, fill_val) = args.get_two_args("numpy.full_like", vm.heap)?;
    defer_drop!(arr_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.full_like", vm)?;
    let (fill, dtype) = match &fill_val {
        Value::Int(n) => (*n as f64, NdArrayDtype::Int64),
        Value::Float(f) => (*f, NdArrayDtype::Float64),
        Value::Bool(b) => (if *b { 1.0 } else { 0.0 }, NdArrayDtype::Bool),
        _ => {
            fill_val.drop_with_heap(vm);
            return Err(ExcType::type_error("fill_value must be a number"));
        }
    };
    fill_val.drop_with_heap(vm);
    let size = arr.data().len();
    let result = NdArray::new(vec![fill; size], arr.shape().to_vec(), dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- Phase 7: Sorting, searching, set ops ---

/// `numpy.argsort(a)` — module-level argsort.
fn call_argsort_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.argsort", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.argsort", vm)?;
    let data = arr.data();
    let mut indices: Vec<usize> = (0..data.len()).collect();
    indices.sort_by(|&a, &b| {
        let va = data[a];
        let vb = data[b];
        va.partial_cmp(&vb).unwrap_or_else(|| {
            if va.is_nan() && vb.is_nan() {
                Ordering::Equal
            } else if va.is_nan() {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        })
    });
    let result_data: Vec<f64> = indices.iter().map(|&i| i as f64).collect();
    let len = result_data.len();
    let result = NdArray::new(result_data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.searchsorted(a, v)` — find insertion points for `v` in sorted array `a`.
#[expect(
    clippy::cast_possible_wrap,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_searchsorted(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, v_val) = args.get_two_args("numpy.searchsorted", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(v_val, vm);
    let a = ndarray_from_value(a_val, "numpy.searchsorted", vm)?;
    let sorted = a.data();
    // v can be scalar or array
    match v_val {
        Value::Int(n) => {
            let v = *n as f64;
            let idx = sorted.partition_point(|&x| x < v);
            Ok(Value::Int(idx as i64))
        }
        Value::Float(f) => {
            let idx = sorted.partition_point(|&x| x < *f);
            Ok(Value::Int(idx as i64))
        }
        _ => {
            let v_arr = ndarray_from_value(v_val, "numpy.searchsorted", vm)?;
            let data: Vec<f64> = v_arr
                .data()
                .iter()
                .map(|&v| sorted.partition_point(|&x| x < v) as f64)
                .collect();
            let len = data.len();
            let result = NdArray::new(data, vec![len], NdArrayDtype::Int64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
        }
    }
}

/// `numpy.extract(condition, arr)` — extract elements where condition is True.
fn call_extract(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (cond_val, arr_val) = args.get_two_args("numpy.extract", vm.heap)?;
    defer_drop!(cond_val, vm);
    defer_drop!(arr_val, vm);
    let cond = ndarray_from_value(cond_val, "numpy.extract", vm)?;
    let arr = ndarray_from_value(arr_val, "numpy.extract", vm)?;
    let data: Vec<f64> = cond
        .data()
        .iter()
        .zip(arr.data().iter())
        .filter(|(c, _)| **c != 0.0)
        .map(|(_, v)| *v)
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Set operation type.
#[derive(Clone, Copy)]
enum SetOp {
    Intersect,
    Union,
    Diff,
    Xor,
}

/// Generic set operation on two sorted-unique arrays.
fn call_set_op(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, op: SetOp, name: &str) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a_arr = ndarray_from_value(a_val, name, vm)?;
    let b_arr = ndarray_from_value(b_val, name, vm)?;
    let mut a: Vec<f64> = a_arr.data().to_vec();
    let mut b: Vec<f64> = b_arr.data().to_vec();
    a.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
    a.dedup();
    b.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
    b.dedup();
    let data: Vec<f64> = match op {
        SetOp::Intersect => a.iter().filter(|v| b.contains(v)).copied().collect(),
        SetOp::Union => {
            let mut u = a.clone();
            for v in &b {
                if !u.contains(v) {
                    u.push(*v);
                }
            }
            u.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
            u
        }
        SetOp::Diff => a.iter().filter(|v| !b.contains(v)).copied().collect(),
        SetOp::Xor => {
            let mut r: Vec<f64> = a.iter().filter(|v| !b.contains(v)).copied().collect();
            r.extend(b.iter().filter(|v| !a.contains(v)));
            r.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
            r
        }
    };
    let len = data.len();
    let dtype = promote_dtype(a_arr.dtype(), b_arr.dtype());
    let result = NdArray::new(data, vec![len], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.bincount(a)` — count occurrences of each non-negative integer value.
fn call_bincount(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.bincount", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.bincount", vm)?;
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "index from user data"
    )]
    let max_val = arr.data().iter().fold(0usize, |m, &v| m.max(v as usize));
    let mut counts = vec![0.0; max_val + 1];
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "index from user data"
    )]
    for &v in arr.data() {
        counts[v as usize] += 1.0;
    }
    let len = counts.len();
    let result = NdArray::new(counts, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.digitize(x, bins)` — indices of bins to which each value belongs.
fn call_digitize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, bins_val) = args.get_two_args("numpy.digitize", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(bins_val, vm);
    let x = ndarray_from_value(x_val, "numpy.digitize", vm)?;
    let bins = ndarray_from_value(bins_val, "numpy.digitize", vm)?;
    let bins_data = bins.data();
    let data: Vec<f64> = x
        .data()
        .iter()
        .map(|&v| bins_data.partition_point(|&b| b <= v) as f64)
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- Phase 8: Linear algebra ---

/// `numpy.outer(a, b)` — outer product of two vectors.
fn call_outer(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.outer", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a = ndarray_from_value(a_val, "numpy.outer", vm)?;
    let b = ndarray_from_value(b_val, "numpy.outer", vm)?;
    let m = a.data().len();
    let n = b.data().len();
    check_array_alloc_size(m * n, vm.heap.tracker())?;
    let mut data = Vec::with_capacity(m * n);
    for &ai in a.data() {
        for &bj in b.data() {
            data.push(ai * bj);
        }
    }
    let dtype = promote_dtype(a.dtype(), b.dtype());
    let result = NdArray::new(data, vec![m, n], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.cross(a, b)` — cross product of 3-element vectors.
fn call_cross(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.cross", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a = ndarray_from_value(a_val, "numpy.cross", vm)?;
    let b = ndarray_from_value(b_val, "numpy.cross", vm)?;
    if a.data().len() != 3 || b.data().len() != 3 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "cross product requires 3-element vectors").into());
    }
    let (a0, a1, a2) = (a.data()[0], a.data()[1], a.data()[2]);
    let (b0, b1, b2) = (b.data()[0], b.data()[1], b.data()[2]);
    let data = vec![a1 * b2 - a2 * b1, a2 * b0 - a0 * b2, a0 * b1 - a1 * b0];
    let dtype = promote_dtype(a.dtype(), b.dtype());
    let result = NdArray::new(data, vec![3], dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

// --- Phase 10: Additional creation and numerical ---

/// `numpy.logspace(start, stop, num)` — log-spaced values.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_logspace(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.logspace", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let start_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.logspace() requires 3 arguments"))?;
    defer_drop!(start_val, vm);
    let stop_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.logspace() requires 3 arguments"))?;
    defer_drop!(stop_val, vm);
    let num_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.logspace() requires 3 arguments"))?;
    defer_drop!(num_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let start = to_f64(start_val, vm)?;
    let stop = to_f64(stop_val, vm)?;
    let Value::Int(num) = num_val else {
        return Err(ExcType::type_error("num must be integer"));
    };
    let num = *num as usize;
    check_array_alloc_size(num, vm.heap.tracker())?;
    // logspace: 10^linspace(start, stop, num)
    let data: Vec<f64> = if num == 0 {
        Vec::new()
    } else if num == 1 {
        vec![10.0f64.powf(start)]
    } else {
        let step = (stop - start) / (num - 1) as f64;
        (0..num).map(|i| 10.0f64.powf(start + step * i as f64)).collect()
    };
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.geomspace(start, stop, num)` — geometrically spaced values.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "array indices are small enough that these casts are safe"
)]
fn call_geomspace(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.geomspace", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let start_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.geomspace() requires 3 arguments"))?;
    defer_drop!(start_val, vm);
    let stop_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.geomspace() requires 3 arguments"))?;
    defer_drop!(stop_val, vm);
    let num_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.geomspace() requires 3 arguments"))?;
    defer_drop!(num_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let start = to_f64(start_val, vm)?;
    let stop = to_f64(stop_val, vm)?;
    let Value::Int(num) = num_val else {
        return Err(ExcType::type_error("num must be integer"));
    };
    let num = *num as usize;
    check_array_alloc_size(num, vm.heap.tracker())?;
    let data: Vec<f64> = if num == 0 {
        Vec::new()
    } else if num == 1 {
        vec![start]
    } else {
        let log_start = start.ln();
        let log_stop = stop.ln();
        let step = (log_stop - log_start) / (num - 1) as f64;
        (0..num).map(|i| (log_start + step * i as f64).exp()).collect()
    };
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.tri(N)` — NxN array with ones at and below diagonal.
fn call_tri(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.tri", vm.heap)?;
    let n = extract_size(arg, "numpy.tri", vm)?;
    check_array_alloc_size(n * n, vm.heap.tracker())?;
    let mut data = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            data[i * n + j] = 1.0;
        }
    }
    let result = NdArray::new(data, vec![n, n], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.tril(m)` — lower triangle of array.
fn call_tril(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.tril", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.tril", vm)?;
    if arr.shape().len() < 2 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "tril requires 2-d array").into());
    }
    let rows = arr.shape()[0];
    let cols = arr.shape()[1];
    let mut data = arr.data().to_vec();
    for i in 0..rows {
        for j in (i + 1)..cols {
            data[i * cols + j] = 0.0;
        }
    }
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.triu(m)` — upper triangle of array.
fn call_triu(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.triu", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.triu", vm)?;
    if arr.shape().len() < 2 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "triu requires 2-d array").into());
    }
    let rows = arr.shape()[0];
    let cols = arr.shape()[1];
    let mut data = arr.data().to_vec();
    for i in 0..rows {
        for j in 0..i.min(cols) {
            data[i * cols + j] = 0.0;
        }
    }
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.meshgrid(*xi)` — coordinate matrices from coordinate vectors.
fn call_meshgrid(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.meshgrid", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let mut arrays: Vec<NdArray> = Vec::new();
    for val in pos {
        let arr = ndarray_from_value(&val, "numpy.meshgrid", vm)?;
        val.drop_with_heap(vm);
        arrays.push(arr);
    }
    if arrays.len() != 2 {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "meshgrid currently supports exactly 2 arrays").into(),
        );
    }
    let x = &arrays[0];
    let y = &arrays[1];
    let nx = x.data().len();
    let ny = y.data().len();
    check_array_alloc_size(nx * ny * 2, vm.heap.tracker())?;
    // XX: repeat x for each row
    let mut xx_data = Vec::with_capacity(ny * nx);
    for _ in 0..ny {
        xx_data.extend_from_slice(x.data());
    }
    // YY: repeat each y value nx times
    let mut yy_data = Vec::with_capacity(ny * nx);
    for &yv in y.data() {
        for _ in 0..nx {
            yy_data.push(yv);
        }
    }
    let dtype = promote_dtype(x.dtype(), y.dtype());
    let xx = NdArray::new(xx_data, vec![ny, nx], dtype);
    let yy = NdArray::new(yy_data, vec![ny, nx], dtype);
    let xx_val = Value::Ref(vm.heap.allocate(HeapData::NdArray(xx))?);
    let yy_val = Value::Ref(vm.heap.allocate(HeapData::NdArray(yy))?);
    let values: SmallVec<[Value; 3]> = smallvec::smallvec![xx_val, yy_val];
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// `numpy.gradient(f)` — numerical gradient using central differences.
fn call_gradient(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.gradient", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.gradient", vm)?;
    let data = arr.data();
    let n = data.len();
    if n < 2 {
        let result = NdArray::new(vec![0.0; n], vec![n], NdArrayDtype::Float64);
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }
    let mut grad = Vec::with_capacity(n);
    // Forward difference for first element
    grad.push(data[1] - data[0]);
    // Central differences for interior
    for i in 1..n - 1 {
        grad.push((data[i + 1] - data[i - 1]) / 2.0);
    }
    // Backward difference for last element
    grad.push(data[n - 1] - data[n - 2]);
    let result = NdArray::new(grad, vec![n], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.convolve(a, v)` — discrete linear convolution (mode='full').
fn call_convolve(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, v_val) = args.get_two_args("numpy.convolve", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(v_val, vm);
    let a = ndarray_from_value(a_val, "numpy.convolve", vm)?;
    let v = ndarray_from_value(v_val, "numpy.convolve", vm)?;
    let na = a.data().len();
    let nv = v.data().len();
    let out_len = na + nv - 1;
    check_array_alloc_size(out_len, vm.heap.tracker())?;
    let mut result_data = vec![0.0; out_len];
    for i in 0..na {
        for j in 0..nv {
            result_data[i + j] += a.data()[i] * v.data()[j];
        }
    }
    let result = NdArray::new(result_data, vec![out_len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.correlate(a, v)` — cross-correlation (mode='valid').
fn call_correlate(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, v_val) = args.get_two_args("numpy.correlate", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(v_val, vm);
    let a = ndarray_from_value(a_val, "numpy.correlate", vm)?;
    let v = ndarray_from_value(v_val, "numpy.correlate", vm)?;
    let na = a.data().len();
    let nv = v.data().len();
    if na < nv {
        return Err(SimpleException::new_msg(ExcType::ValueError, "a must be at least as long as v").into());
    }
    let out_len = na - nv + 1;
    let mut result_data = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let sum: f64 = (0..nv).map(|j| a.data()[i + j] * v.data()[j]).sum();
        result_data.push(sum);
    }
    let result = NdArray::new(result_data, vec![out_len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.interp(x, xp, fp)` — 1D linear interpolation.
fn call_interp(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.interp", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let x_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.interp() requires 3 arguments"))?;
    defer_drop!(x_val, vm);
    let xp_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.interp() requires 3 arguments"))?;
    defer_drop!(xp_val, vm);
    let fp_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.interp() requires 3 arguments"))?;
    defer_drop!(fp_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    let x = ndarray_from_value(x_val, "numpy.interp", vm)?;
    let xp = ndarray_from_value(xp_val, "numpy.interp", vm)?;
    let fp = ndarray_from_value(fp_val, "numpy.interp", vm)?;
    let xp_data = xp.data();
    let fp_data = fp.data();
    let data: Vec<f64> = x
        .data()
        .iter()
        .map(|&xi| {
            if xi <= xp_data[0] {
                return fp_data[0];
            }
            if xi >= xp_data[xp_data.len() - 1] {
                return fp_data[fp_data.len() - 1];
            }
            let idx = xp_data.partition_point(|&xv| xv < xi);
            if idx == 0 {
                return fp_data[0];
            }
            let x0 = xp_data[idx - 1];
            let x1 = xp_data[idx];
            let f0 = fp_data[idx - 1];
            let f1 = fp_data[idx];
            f0 + (f1 - f0) * (xi - x0) / (x1 - x0)
        })
        .collect();
    let len = data.len();
    let result = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.select(condlist, choicelist, default=0)` — conditional selection.
fn call_select(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.select", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let condlist_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.select() requires 2-3 arguments"))?;
    defer_drop!(condlist_val, vm);
    let choicelist_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error("numpy.select() requires 2-3 arguments"))?;
    defer_drop!(choicelist_val, vm);
    let default_val = pos
        .next()
        .map(|v| {
            let result = to_f64(&v, vm);
            v.drop_with_heap(vm);
            result
        })
        .transpose()?
        .unwrap_or(0.0);
    for extra in pos {
        extra.drop_with_heap(vm);
    }
    // Extract conditions and choices from lists
    let conds: Vec<NdArray> = match condlist_val {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => {
                let items: Vec<Value> = list.as_slice().iter().map(|v| v.clone_with_heap(vm)).collect();
                let result: Vec<NdArray> = items
                    .iter()
                    .map(|v| ndarray_from_value(v, "numpy.select", vm))
                    .collect::<RunResult<Vec<_>>>()?;
                for item in items {
                    item.drop_with_heap(vm);
                }
                result
            }
            _ => return Err(ExcType::type_error("condlist must be a list")),
        },
        _ => return Err(ExcType::type_error("condlist must be a list")),
    };
    let choices: Vec<NdArray> = match choicelist_val {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => {
                let items: Vec<Value> = list.as_slice().iter().map(|v| v.clone_with_heap(vm)).collect();
                let result: Vec<NdArray> = items
                    .iter()
                    .map(|v| ndarray_from_value(v, "numpy.select", vm))
                    .collect::<RunResult<Vec<_>>>()?;
                for item in items {
                    item.drop_with_heap(vm);
                }
                result
            }
            _ => return Err(ExcType::type_error("choicelist must be a list")),
        },
        _ => return Err(ExcType::type_error("choicelist must be a list")),
    };
    if conds.is_empty() || conds.len() != choices.len() {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "condlist and choicelist must have same length").into(),
        );
    }
    let n = conds[0].data().len();
    let mut data = vec![default_val; n];
    // Process in reverse order so first matching condition wins
    for (cond, choice) in conds.iter().zip(choices.iter()).rev() {
        for (i, (&c, &v)) in cond.data().iter().zip(choice.data().iter()).enumerate() {
            if c != 0.0 {
                data[i] = v;
            }
        }
    }
    let result = NdArray::new(data, vec![n], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}
