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
//! - `numpy.copysign`, `numpy.frexp`, `numpy.modf`, `numpy.ldexp`, `numpy.gcd`, `numpy.lcm`
//! - `numpy.logaddexp`, `numpy.nextafter`, `numpy.spacing`, `numpy.signbit`, `numpy.sinc`
//! - `numpy.bitwise_and`, `numpy.invert`, `numpy.left_shift`, `numpy.bitwise_count`
//! - `numpy.packbits`, `numpy.unpackbits`
//! - `numpy.i0`, `numpy.bartlett`, `numpy.blackman`, `numpy.hamming`, `numpy.hanning`, `numpy.kaiser`
//! - `numpy.base_repr`, `numpy.binary_repr`
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
//! - `numpy.array2string(a)`, `numpy.array_repr(a)`, `numpy.array_str(a)`
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
//! - `numpy.take`, `numpy.compress`, `numpy.swapaxes`, `numpy.permute_dims`
//! - `numpy.matrix_transpose`, `numpy.moveaxis`, `numpy.rollaxis`, `numpy.rot90`
//! - `numpy.vecdot`, `numpy.matvec`, `numpy.vecmat`, `numpy.trapezoid`, `numpy.vander`
//!
//! ## Search & index
//! - `numpy.nonzero(a)`, `numpy.argwhere(a)`
//! - `numpy.diag_indices`, `numpy.tril_indices`, `numpy.triu_indices`
//! - `numpy.indices`, `numpy.unravel_index`, `numpy.ravel_multi_index`, `numpy.ix_`

use std::{
    cmp::Ordering,
    f64::consts::{E, PI},
};

use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapReadOutput},
    heap_traits::DropWithHeap,
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker, check_array_alloc_size},
    types::{
        Dict, List, Module, NamedTuple, NdArray, PyTrait, allocate_tuple,
        ndarray::{NdArrayDtype, nan_last_cmp, ndarray_from_list, promote_dtype, promote_dtype_with_scalar},
        str::{Str, allocate_string},
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
    /// `numpy.array2string(a)` — bare ndarray display string.
    Array2string,
    /// `numpy.array_repr(a)` — ndarray repr string.
    ArrayRepr,
    /// `numpy.array_str(a)` — bare ndarray string.
    ArrayStr,
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
    /// `numpy.unique_values(a)` — return unique values.
    UniqueValues,
    /// `numpy.unique_counts(a)` — return unique values and counts.
    UniqueCounts,
    /// `numpy.unique_inverse(a)` — return unique values and inverse indices.
    UniqueInverse,
    /// `numpy.unique_all(a)` — return unique values, first indices, inverse indices, and counts.
    UniqueAll,
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
    /// `numpy.ediff1d(a)` — flattened first-order discrete difference.
    Ediff1d,
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
    /// `numpy.isposinf(a)` — element-wise positive infinity test.
    Isposinf,
    /// `numpy.isneginf(a)` — element-wise negative infinity test.
    Isneginf,
    /// `numpy.isfinite(a)` — element-wise finiteness test.
    Isfinite,
    /// `numpy.array_equal(a, b)` — true if arrays are element-wise equal.
    ArrayEqual,
    /// `numpy.array_equiv(a, b)` — true if arrays are equal after scalar broadcasting.
    ArrayEquiv,
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
    /// `numpy.take(a, indices)` — gather flattened elements by index.
    Take,
    /// `numpy.take_along_axis(a, indices, axis)` — gather along an axis.
    TakeAlongAxis,
    /// `numpy.resize(a, new_shape)` — repeat flattened data into a new shape.
    Resize,
    /// `numpy.compress(condition, a)` — select flattened elements by condition.
    Compress,
    /// `numpy.swapaxes(a, axis1, axis2)` — swap two axes.
    Swapaxes,
    /// `numpy.permute_dims(a, axes=None)` — permute ndarray axes.
    PermuteDims,
    /// `numpy.matrix_transpose(a)` — swap the last two axes.
    MatrixTranspose,
    /// `numpy.moveaxis(a, source, destination)` — move axes to new positions.
    Moveaxis,
    /// `numpy.rollaxis(a, axis, start=0)` — roll one axis backward.
    Rollaxis,
    /// `numpy.rot90(a, k=1)` — rotate a 2-D array by quarter turns.
    Rot90,
    /// `numpy.choose(a, choices)` — select values from a sequence of choices.
    Choose,
    /// `numpy.append(a, values)` — append values to end of array.
    Append,
    /// `numpy.vstack(arrays)` — stack arrays vertically.
    Vstack,
    /// `numpy.hstack(arrays)` — stack arrays horizontally.
    Hstack,
    /// `numpy.dstack(arrays)` — stack arrays along depth after promoting to 3-D.
    Dstack,
    /// `numpy.stack(arrays)` — stack arrays along new axis.
    Stack,
    /// `numpy.unstack(a, axis=0)` — split an array into a tuple along an axis.
    Unstack,
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
    /// `numpy.angle(z, deg=False)` — real-valued phase angle.
    Angle,
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
    /// `numpy.copysign(a, b)` — element-wise magnitude/sign combination.
    Copysign,
    /// `numpy.frexp(a)` — element-wise mantissa/exponent decomposition.
    Frexp,
    /// `numpy.modf(a)` — element-wise fractional/integer decomposition.
    Modf,
    /// `numpy.ldexp(a, exp)` — element-wise multiply by powers of two.
    Ldexp,
    /// `numpy.gcd(a, b)` — element-wise greatest common divisor.
    Gcd,
    /// `numpy.lcm(a, b)` — element-wise least common multiple.
    Lcm,
    /// `numpy.logaddexp(a, b)` — element-wise log(exp(a) + exp(b)).
    Logaddexp,
    /// `numpy.logaddexp2(a, b)` — element-wise log2(2**a + 2**b).
    Logaddexp2,
    /// `numpy.nextafter(a, b)` — next floating point value from a toward b.
    Nextafter,
    /// `numpy.spacing(a)` — distance to the nearest adjacent floating value.
    Spacing,
    /// `numpy.signbit(a)` — element-wise sign-bit predicate.
    Signbit,
    /// `numpy.sinc(a)` — normalized sinc function.
    Sinc,
    /// `numpy.heaviside(a, h0)` — element-wise Heaviside step function.
    Heaviside,
    /// `numpy.trunc(a)` — truncate toward zero.
    Trunc,
    /// `numpy.fix(a)` — truncate toward zero.
    Fix,
    /// `numpy.float_power(a, b)` — element-wise floating-point exponentiation.
    FloatPower,
    /// `numpy.divmod(a, b)` — element-wise floor division and modulo pair.
    Divmod,
    /// `numpy.bitwise_and(a, b)` — element-wise integer/boolean bitwise AND.
    BitwiseAnd,
    /// `numpy.bitwise_or(a, b)` — element-wise integer/boolean bitwise OR.
    BitwiseOr,
    /// `numpy.bitwise_xor(a, b)` — element-wise integer/boolean bitwise XOR.
    BitwiseXor,
    /// `numpy.bitwise_not(a)` / aliases — element-wise integer/boolean inversion.
    BitwiseNot,
    /// `numpy.left_shift(a, b)` — element-wise integer left shift.
    LeftShift,
    /// `numpy.right_shift(a, b)` — element-wise integer right shift.
    RightShift,
    /// `numpy.bitwise_count(a)` — count set bits in each integer's absolute value.
    BitwiseCount,
    /// `numpy.packbits(a)` — pack non-zero values into byte-sized integers.
    Packbits,
    /// `numpy.unpackbits(a)` — unpack byte-sized integers into bit arrays.
    Unpackbits,
    /// `numpy.bartlett(M)` — Bartlett triangular window.
    Bartlett,
    /// `numpy.blackman(M)` — Blackman taper window.
    Blackman,
    /// `numpy.hamming(M)` — Hamming window.
    Hamming,
    /// `numpy.hanning(M)` — Hann window using NumPy's legacy spelling.
    Hanning,
    /// `numpy.kaiser(M, beta)` — Kaiser window.
    Kaiser,
    /// `numpy.i0(x)` — modified Bessel function of the first kind, order 0.
    I0,
    /// `numpy.base_repr(number, base=2, padding=0)` — integer base conversion string.
    BaseRepr,
    /// `numpy.binary_repr(num, width=None)` — integer binary conversion string.
    BinaryRepr,
    /// `numpy.conj(a)` — return the real-valued conjugate.
    Conj,
    /// `numpy.real(a)` — return the real component.
    Real,
    /// `numpy.real_if_close(a)` — identity for Monty's real-valued numeric subset.
    RealIfClose,
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
    /// `numpy.can_cast(from_, to)` — compact dtype cast predicate.
    CanCast,
    /// `numpy.promote_types(type1, type2)` — compact dtype promotion helper.
    PromoteTypes,
    /// `numpy.result_type(*arrays_and_dtypes)` — compact dtype result helper.
    ResultType,
    /// `numpy.common_type(*arrays)` — compact common dtype helper.
    CommonType,
    /// `numpy.min_scalar_type(a)` — compact scalar dtype helper.
    MinScalarType,
    /// `numpy.mintypecode(typechars)` — legacy dtype character helper.
    Mintypecode,
    /// `numpy.typename(char)` — legacy dtype character name helper.
    Typename,
    /// `numpy.geterr()` — floating-point error config snapshot.
    Geterr,
    /// `numpy.seterr(...)` — accepted no-op floating-point error config update.
    Seterr,
    /// `numpy.geterrcall()` — floating-point error callback query.
    Geterrcall,
    /// `numpy.seterrcall(callback)` — accepted no-op error callback update.
    Seterrcall,
    /// `numpy.errstate(...)` — lightweight floating-point error context placeholder.
    Errstate,
    /// `numpy.get_printoptions()` — print config snapshot.
    GetPrintoptions,
    /// `numpy.set_printoptions(...)` — accepted no-op print config update.
    SetPrintoptions,
    /// `numpy.printoptions(...)` — lightweight print config context placeholder.
    Printoptions,
    /// `numpy.getbufsize()` — legacy buffer size query.
    Getbufsize,
    /// `numpy.setbufsize(size)` — accepted no-op buffer size update.
    Setbufsize,
    /// `numpy.show_runtime()` — no-host runtime display placeholder.
    ShowRuntime,
    /// `numpy.test()` — no-op test-runner placeholder.
    Test,
    /// `numpy.atleast_1d(*arrays)` — view inputs as arrays with at least one dimension.
    Atleast1d,
    /// `numpy.atleast_2d(*arrays)` — view inputs as arrays with at least two dimensions.
    Atleast2d,
    /// `numpy.atleast_3d(*arrays)` — view inputs as arrays with at least three dimensions.
    Atleast3d,
    /// `numpy.diag_indices(n, ndim=2)` — indices for a diagonal in an `ndim` array.
    DiagIndices,
    /// `numpy.diag_indices_from(arr)` — diagonal indices matching a square array.
    DiagIndicesFrom,
    /// `numpy.tril_indices(n, k=0, m=None)` — lower-triangle indices.
    TrilIndices,
    /// `numpy.tril_indices_from(arr, k=0)` — lower-triangle indices for an array.
    TrilIndicesFrom,
    /// `numpy.triu_indices(n, k=0, m=None)` — upper-triangle indices.
    TriuIndices,
    /// `numpy.triu_indices_from(arr, k=0)` — upper-triangle indices for an array.
    TriuIndicesFrom,
    /// `numpy.indices(dimensions)` — dense coordinate grid arrays.
    Indices,
    /// `numpy.unravel_index(indices, shape)` — flat indices to coordinates.
    UnravelIndex,
    /// `numpy.ravel_multi_index(multi_index, dims)` — coordinates to flat indices.
    RavelMultiIndex,

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
    /// `numpy.diagflat(v, k=0)` — create a diagonal matrix from flattened input.
    Diagflat,
    /// `numpy.fill_diagonal(a, val)` — fill an array diagonal in place.
    FillDiagonal,
    /// `numpy.put(a, ind, v)` — assign flattened positions in place.
    Put,
    /// `numpy.put_along_axis(a, indices, values, axis)` — assign positions along an axis.
    PutAlongAxis,
    /// `numpy.copyto(dst, src)` — copy values into an array in place.
    Copyto,
    /// `numpy.putmask(a, mask, values)` — assign positions where a mask is true.
    Putmask,
    /// `numpy.place(a, mask, values)` — place values sequentially where a mask is true.
    Place,
    /// `numpy.diagonal(a)` — return diagonal of array.
    Diagonal,
    /// `numpy.trace(a)` — sum of diagonal elements.
    Trace,
    /// `numpy.flatnonzero(a)` — non-zero indices in flattened array.
    Flatnonzero,
    /// `numpy.asarray(a)` — convert to array without copy if possible.
    Asarray,
    /// `numpy.asarray_chkfinite(a)` — convert to array and reject NaN/Inf values.
    AsarrayChkfinite,
    /// `numpy.ascontiguousarray(a)` — Monty ndarray conversion with C-order semantics.
    Ascontiguousarray,
    /// `numpy.asfortranarray(a)` — Monty ndarray conversion with Fortran-order compatibility.
    Asfortranarray,
    /// `numpy.require(a)` — Monty ndarray conversion ignoring unsupported layout flags.
    Require,
    /// `numpy.ix_(*args)` — construct open mesh index arrays from 1-D sequences.
    Ix,
    /// `numpy.mask_indices(n, mask_func, k=0)` — indices selected by triangular masks.
    MaskIndices,
    /// `numpy.isfortran(a)` — true for Fortran-contiguous arrays.
    Isfortran,
    /// `numpy.may_share_memory(a, b)` — conservative overlap predicate.
    MayShareMemory,
    /// `numpy.shares_memory(a, b)` — exact overlap predicate for Monty's ndarray refs.
    SharesMemory,
    /// `numpy.column_stack(arrays)` — stack 1D arrays as columns.
    ColumnStack,
    /// `numpy.row_stack(arrays)` — alias for vstack.
    RowStack,
    /// `numpy.hsplit(a, n)` — horizontal split.
    Hsplit,
    /// `numpy.vsplit(a, n)` — vertical split.
    Vsplit,
    /// `numpy.dsplit(a, n)` — depth split.
    Dsplit,
    /// `numpy.array_split(a, n)` — split into possibly unequal parts.
    ArraySplit,
    /// `numpy.full_like(a, fill_value)` — array of same shape filled with value.
    FullLike,
    /// `numpy.empty_like(a)` — uninitialized array of same shape.
    EmptyLike,

    // --- Phase 7: Sorting, searching, set operations ---
    /// `numpy.argsort(a)` — module-level argsort.
    ArgsortMod,
    /// `numpy.argpartition(a, kth)` — indirect partition indices for 1-D arrays.
    Argpartition,
    /// `numpy.partition(a, kth)` — partition values for 1-D arrays.
    Partition,
    /// `numpy.lexsort(keys)` — indirect stable sort over 1-D key arrays.
    Lexsort,
    /// `numpy.cov(m)` — covariance matrix for 1-D or row-wise 2-D input.
    Cov,
    /// `numpy.corrcoef(x)` — correlation matrix for 1-D or row-wise 2-D input.
    Corrcoef,
    /// `numpy.searchsorted(a, v)` — find insertion points.
    Searchsorted,
    /// `numpy.extract(condition, arr)` — extract elements by condition.
    Extract,
    /// `numpy.trim_zeros(filt, trim='fb')` — trim leading and/or trailing zeros.
    TrimZeros,
    /// `numpy.unwrap(p, discont=None)` — unwrap phase jumps in a 1-D sequence.
    Unwrap,
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
    /// `numpy.vecdot(a, b)` — vector dot product.
    Vecdot,
    /// `numpy.matvec(a, x)` — matrix-vector multiplication.
    Matvec,
    /// `numpy.vecmat(x, a)` — vector-matrix multiplication.
    Vecmat,
    /// `numpy.cross(a, b)` — cross product (3-element vectors).
    Cross,
    /// `numpy.kron(a, b)` — Kronecker product.
    Kron,
    /// `numpy.trapezoid(y, x=None, dx=1.0)` — composite trapezoidal integral.
    Trapezoid,
    /// `numpy.vander(x, N=None, increasing=False)` — Vandermonde matrix.
    Vander,

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
    module.set_attr(
        StaticStrings::NpLittleEndian,
        Value::Bool(cfg!(target_endian = "little")),
        vm,
    );
    module.set_attr(StaticStrings::NpEulerGamma, Value::Float(0.577_215_664_901_532_9), vm);

    // Dtype type objects — stored as interned strings that astype() recognizes.
    // These allow `arr.astype(np.float64)` to work alongside `arr.astype('float64')`.
    for (name, target) in NUMPY_DTYPE_ALIASES {
        module.set_attr(*name, Value::InternString((*target).into()), vm);
    }
    module.set_attr(StaticStrings::NpTypecodes, numpy_typecodes_dict(vm)?, vm);

    vm.heap.allocate(HeapData::Module(module))
}

/// Builds NumPy's legacy `typecodes` dictionary for code that inspects dtype families.
fn numpy_typecodes_dict(vm: &mut VM<'_, impl ResourceTracker>) -> Result<Value, ResourceError> {
    let pairs = [
        ("Character", "c"),
        ("Integer", "bhilqnp"),
        ("UnsignedInteger", "BHILQNP"),
        ("Float", "efdg"),
        ("Complex", "FDG"),
        ("AllInteger", "bBhHiIlLqQnNpP"),
        ("AllFloat", "efdgFDG"),
        ("Datetime", "Mm"),
        ("All", "?bhilqnpBHILQNPefdgFDGSUVOMm"),
    ]
    .into_iter()
    .map(|(key, value)| {
        let key = Value::Ref(vm.heap.allocate(HeapData::Str(Str::new(key.to_string())))?);
        let value = Value::Ref(vm.heap.allocate(HeapData::Str(Str::new(value.to_string())))?);
        Ok((key, value))
    })
    .collect::<Result<Vec<_>, ResourceError>>()?;
    let dict = Dict::from_pairs(pairs, vm).expect("numpy.typecodes uses hashable string literal keys");
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))?))
}

/// NumPy dtype attributes supported by Monty's compact numeric ndarray model.
///
/// Many NumPy dtype names are aliases for platform-sized or narrower integer
/// and floating point types. Monty currently stores only bool, int64, and
/// float64 arrays, so aliases are mapped to the closest dtype marker that
/// existing `astype()` understands instead of introducing new storage formats.
const NUMPY_DTYPE_ALIASES: &[(StaticStrings, StaticStrings)] = &[
    (StaticStrings::NpFloat64, StaticStrings::NpFloat64),
    (StaticStrings::NpDouble, StaticStrings::NpFloat64),
    (StaticStrings::NpLongdouble, StaticStrings::NpFloat64),
    (StaticStrings::NpFloat32, StaticStrings::NpFloat32),
    (StaticStrings::NpFloat16, StaticStrings::NpFloat32),
    (StaticStrings::NpHalf, StaticStrings::NpFloat32),
    (StaticStrings::NpSingle, StaticStrings::NpFloat32),
    (StaticStrings::NpInt64, StaticStrings::NpInt64),
    (StaticStrings::NpInt_, StaticStrings::NpInt64),
    (StaticStrings::NpIntp, StaticStrings::NpInt64),
    (StaticStrings::NpLong, StaticStrings::NpInt64),
    (StaticStrings::NpLonglong, StaticStrings::NpInt64),
    (StaticStrings::NpByte, StaticStrings::NpInt64),
    (StaticStrings::NpShort, StaticStrings::NpInt64),
    (StaticStrings::NpInt8, StaticStrings::NpInt64),
    (StaticStrings::NpInt16, StaticStrings::NpInt64),
    (StaticStrings::NpUint, StaticStrings::NpInt64),
    (StaticStrings::NpUintp, StaticStrings::NpInt64),
    (StaticStrings::NpUbyte, StaticStrings::NpInt64),
    (StaticStrings::NpUshort, StaticStrings::NpInt64),
    (StaticStrings::NpUint8, StaticStrings::NpInt64),
    (StaticStrings::NpUint16, StaticStrings::NpInt64),
    (StaticStrings::NpUint32, StaticStrings::NpInt64),
    (StaticStrings::NpUint64, StaticStrings::NpInt64),
    (StaticStrings::NpUlong, StaticStrings::NpInt64),
    (StaticStrings::NpUlonglong, StaticStrings::NpInt64),
    (StaticStrings::NpInt32, StaticStrings::NpInt32),
    (StaticStrings::NpIntc, StaticStrings::NpInt32),
    (StaticStrings::NpUintc, StaticStrings::NpInt32),
    (StaticStrings::NpBool_, StaticStrings::NpBool_),
    (StaticStrings::NpBool, StaticStrings::NpBool_),
];

/// Static mapping of attribute names to numpy functions for module creation.
const NUMPY_FUNCTIONS: &[(StaticStrings, NumpyFunctions)] = &[
    (StaticStrings::NpArray, NumpyFunctions::Array),
    (StaticStrings::NpArray2string, NumpyFunctions::Array2string),
    (StaticStrings::NpArrayRepr, NumpyFunctions::ArrayRepr),
    (StaticStrings::NpArrayStr, NumpyFunctions::ArrayStr),
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
    (StaticStrings::NpUniqueValues, NumpyFunctions::UniqueValues),
    (StaticStrings::NpUniqueCounts, NumpyFunctions::UniqueCounts),
    (StaticStrings::NpUniqueInverse, NumpyFunctions::UniqueInverse),
    (StaticStrings::NpUniqueAll, NumpyFunctions::UniqueAll),
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
    (StaticStrings::NpEdiff1d, NumpyFunctions::Ediff1d),
    (StaticStrings::NpFull, NumpyFunctions::Full),
    (StaticStrings::NpEye, NumpyFunctions::Eye),
    (StaticStrings::Copy, NumpyFunctions::NpCopy),
    (StaticStrings::NpEmpty, NumpyFunctions::Empty),
    (StaticStrings::NpZerosLike, NumpyFunctions::ZerosLike),
    (StaticStrings::NpOnesLike, NumpyFunctions::OnesLike),
    (StaticStrings::Isnan, NumpyFunctions::Isnan),
    (StaticStrings::Isinf, NumpyFunctions::Isinf),
    (StaticStrings::NpIsposinf, NumpyFunctions::Isposinf),
    (StaticStrings::NpIsneginf, NumpyFunctions::Isneginf),
    (StaticStrings::Isfinite, NumpyFunctions::Isfinite),
    (StaticStrings::NpArrayEqual, NumpyFunctions::ArrayEqual),
    (StaticStrings::NpArrayEquiv, NumpyFunctions::ArrayEquiv),
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
    (StaticStrings::NpTake, NumpyFunctions::Take),
    (StaticStrings::NpTakeAlongAxis, NumpyFunctions::TakeAlongAxis),
    (StaticStrings::NpResize, NumpyFunctions::Resize),
    (StaticStrings::NpCompress, NumpyFunctions::Compress),
    (StaticStrings::NpSwapaxes, NumpyFunctions::Swapaxes),
    (StaticStrings::NpPermuteDims, NumpyFunctions::PermuteDims),
    (StaticStrings::NpMatrixTranspose, NumpyFunctions::MatrixTranspose),
    (StaticStrings::NpMoveaxis, NumpyFunctions::Moveaxis),
    (StaticStrings::NpRollaxis, NumpyFunctions::Rollaxis),
    (StaticStrings::NpRot90, NumpyFunctions::Rot90),
    (StaticStrings::NpChoose, NumpyFunctions::Choose),
    (StaticStrings::NpFillDiagonal, NumpyFunctions::FillDiagonal),
    (StaticStrings::NpPut, NumpyFunctions::Put),
    (StaticStrings::NpPutAlongAxis, NumpyFunctions::PutAlongAxis),
    (StaticStrings::NpCopyto, NumpyFunctions::Copyto),
    (StaticStrings::NpPutmask, NumpyFunctions::Putmask),
    (StaticStrings::NpPlace, NumpyFunctions::Place),
    (StaticStrings::Append, NumpyFunctions::Append),
    (StaticStrings::NpVstack, NumpyFunctions::Vstack),
    (StaticStrings::NpHstack, NumpyFunctions::Hstack),
    (StaticStrings::NpDstack, NumpyFunctions::Dstack),
    (StaticStrings::NpStack, NumpyFunctions::Stack),
    (StaticStrings::NpUnstack, NumpyFunctions::Unstack),
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
    (StaticStrings::NpAngle, NumpyFunctions::Angle),
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
    (StaticStrings::Copysign, NumpyFunctions::Copysign),
    (StaticStrings::Frexp, NumpyFunctions::Frexp),
    (StaticStrings::Modf, NumpyFunctions::Modf),
    (StaticStrings::Ldexp, NumpyFunctions::Ldexp),
    (StaticStrings::Gcd, NumpyFunctions::Gcd),
    (StaticStrings::Lcm, NumpyFunctions::Lcm),
    (StaticStrings::NpLogaddexp, NumpyFunctions::Logaddexp),
    (StaticStrings::NpLogaddexp2, NumpyFunctions::Logaddexp2),
    (StaticStrings::Nextafter, NumpyFunctions::Nextafter),
    (StaticStrings::NpSpacing, NumpyFunctions::Spacing),
    (StaticStrings::NpSignbit, NumpyFunctions::Signbit),
    (StaticStrings::NpSinc, NumpyFunctions::Sinc),
    (StaticStrings::NpHeaviside, NumpyFunctions::Heaviside),
    (StaticStrings::Trunc, NumpyFunctions::Trunc),
    (StaticStrings::NpFix, NumpyFunctions::Fix),
    (StaticStrings::NpFloatPower, NumpyFunctions::FloatPower),
    (StaticStrings::NpDivmod, NumpyFunctions::Divmod),
    (StaticStrings::NpBitwiseAnd, NumpyFunctions::BitwiseAnd),
    (StaticStrings::NpBitwiseOr, NumpyFunctions::BitwiseOr),
    (StaticStrings::NpBitwiseXor, NumpyFunctions::BitwiseXor),
    (StaticStrings::NpBitwiseNot, NumpyFunctions::BitwiseNot),
    (StaticStrings::NpBitwiseInvert, NumpyFunctions::BitwiseNot), // alias
    (StaticStrings::NpInvert, NumpyFunctions::BitwiseNot),        // alias
    (StaticStrings::NpLeftShift, NumpyFunctions::LeftShift),
    (StaticStrings::NpBitwiseLeftShift, NumpyFunctions::LeftShift), // alias
    (StaticStrings::NpRightShift, NumpyFunctions::RightShift),
    (StaticStrings::NpBitwiseRightShift, NumpyFunctions::RightShift), // alias
    (StaticStrings::NpBitwiseCount, NumpyFunctions::BitwiseCount),
    (StaticStrings::NpPackbits, NumpyFunctions::Packbits),
    (StaticStrings::NpUnpackbits, NumpyFunctions::Unpackbits),
    (StaticStrings::NpBartlett, NumpyFunctions::Bartlett),
    (StaticStrings::NpBlackman, NumpyFunctions::Blackman),
    (StaticStrings::NpHamming, NumpyFunctions::Hamming),
    (StaticStrings::NpHanning, NumpyFunctions::Hanning),
    (StaticStrings::NpKaiser, NumpyFunctions::Kaiser),
    (StaticStrings::NpI0, NumpyFunctions::I0),
    (StaticStrings::NpBaseRepr, NumpyFunctions::BaseRepr),
    (StaticStrings::NpBinaryRepr, NumpyFunctions::BinaryRepr),
    // Real-only aliases and introspection helpers
    (StaticStrings::NpConj, NumpyFunctions::Conj),
    (StaticStrings::NpConjugate, NumpyFunctions::Conj), // alias
    (StaticStrings::NpReal, NumpyFunctions::Real),
    (StaticStrings::NpRealIfClose, NumpyFunctions::RealIfClose),
    (StaticStrings::NpImag, NumpyFunctions::Imag),
    (StaticStrings::NpIsreal, NumpyFunctions::Isreal),
    (StaticStrings::NpIsrealobj, NumpyFunctions::Isrealobj),
    (StaticStrings::NpIscomplex, NumpyFunctions::Iscomplex),
    (StaticStrings::NpIscomplexobj, NumpyFunctions::Iscomplexobj),
    (StaticStrings::NpIsscalar, NumpyFunctions::Isscalar),
    (StaticStrings::NpIterable, NumpyFunctions::Iterable),
    (StaticStrings::NpCanCast, NumpyFunctions::CanCast),
    (StaticStrings::NpPromoteTypes, NumpyFunctions::PromoteTypes),
    (StaticStrings::NpResultType, NumpyFunctions::ResultType),
    (StaticStrings::NpCommonType, NumpyFunctions::CommonType),
    (StaticStrings::NpMinScalarType, NumpyFunctions::MinScalarType),
    (StaticStrings::NpMintypecode, NumpyFunctions::Mintypecode),
    (StaticStrings::NpTypename, NumpyFunctions::Typename),
    (StaticStrings::NpGeterr, NumpyFunctions::Geterr),
    (StaticStrings::NpSeterr, NumpyFunctions::Seterr),
    (StaticStrings::NpGeterrcall, NumpyFunctions::Geterrcall),
    (StaticStrings::NpSeterrcall, NumpyFunctions::Seterrcall),
    (StaticStrings::NpErrstate, NumpyFunctions::Errstate),
    (StaticStrings::NpGetPrintoptions, NumpyFunctions::GetPrintoptions),
    (StaticStrings::NpSetPrintoptions, NumpyFunctions::SetPrintoptions),
    (StaticStrings::NpPrintoptions, NumpyFunctions::Printoptions),
    (StaticStrings::NpGetbufsize, NumpyFunctions::Getbufsize),
    (StaticStrings::NpSetbufsize, NumpyFunctions::Setbufsize),
    (StaticStrings::NpShowRuntime, NumpyFunctions::ShowRuntime),
    (StaticStrings::NpTest, NumpyFunctions::Test),
    (StaticStrings::NpAtleast1d, NumpyFunctions::Atleast1d),
    (StaticStrings::NpAtleast2d, NumpyFunctions::Atleast2d),
    (StaticStrings::NpAtleast3d, NumpyFunctions::Atleast3d),
    (StaticStrings::NpDiagIndices, NumpyFunctions::DiagIndices),
    (StaticStrings::NpDiagIndicesFrom, NumpyFunctions::DiagIndicesFrom),
    (StaticStrings::NpTrilIndices, NumpyFunctions::TrilIndices),
    (StaticStrings::NpTrilIndicesFrom, NumpyFunctions::TrilIndicesFrom),
    (StaticStrings::NpTriuIndices, NumpyFunctions::TriuIndices),
    (StaticStrings::NpTriuIndicesFrom, NumpyFunctions::TriuIndicesFrom),
    (StaticStrings::NpIndices, NumpyFunctions::Indices),
    (StaticStrings::NpUnravelIndex, NumpyFunctions::UnravelIndex),
    (StaticStrings::NpRavelMultiIndex, NumpyFunctions::RavelMultiIndex),
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
    (StaticStrings::NpDiagflat, NumpyFunctions::Diagflat),
    (StaticStrings::NpDiagonal, NumpyFunctions::Diagonal),
    (StaticStrings::NpTrace, NumpyFunctions::Trace),
    (StaticStrings::NpFlatnonzero, NumpyFunctions::Flatnonzero),
    (StaticStrings::NpAsarray, NumpyFunctions::Asarray),
    (StaticStrings::NpAsarrayChkfinite, NumpyFunctions::AsarrayChkfinite),
    (StaticStrings::NpAscontiguousarray, NumpyFunctions::Ascontiguousarray),
    (StaticStrings::NpAsfortranarray, NumpyFunctions::Asfortranarray),
    (StaticStrings::NpRequire, NumpyFunctions::Require),
    (StaticStrings::NpIx_, NumpyFunctions::Ix),
    (StaticStrings::NpMaskIndices, NumpyFunctions::MaskIndices),
    (StaticStrings::NpIsfortran, NumpyFunctions::Isfortran),
    (StaticStrings::NpMayShareMemory, NumpyFunctions::MayShareMemory),
    (StaticStrings::NpSharesMemory, NumpyFunctions::SharesMemory),
    (StaticStrings::NpColumnStack, NumpyFunctions::ColumnStack),
    (StaticStrings::NpRowStack, NumpyFunctions::RowStack),
    (StaticStrings::NpHsplit, NumpyFunctions::Hsplit),
    (StaticStrings::NpVsplit, NumpyFunctions::Vsplit),
    (StaticStrings::NpDsplit, NumpyFunctions::Dsplit),
    (StaticStrings::NpArraySplit, NumpyFunctions::ArraySplit),
    (StaticStrings::NpFullLike, NumpyFunctions::FullLike),
    (StaticStrings::NpEmptyLike, NumpyFunctions::EmptyLike),
    // Phase 7: Sorting, searching, set ops
    (StaticStrings::NpArgsort, NumpyFunctions::ArgsortMod),
    (StaticStrings::NpArgpartition, NumpyFunctions::Argpartition),
    (StaticStrings::Partition, NumpyFunctions::Partition),
    (StaticStrings::NpLexsort, NumpyFunctions::Lexsort),
    (StaticStrings::NpCov, NumpyFunctions::Cov),
    (StaticStrings::NpCorrcoef, NumpyFunctions::Corrcoef),
    (StaticStrings::NpSearchsorted, NumpyFunctions::Searchsorted),
    (StaticStrings::NpExtract, NumpyFunctions::Extract),
    (StaticStrings::NpTrimZeros, NumpyFunctions::TrimZeros),
    (StaticStrings::NpUnwrap, NumpyFunctions::Unwrap),
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
    (StaticStrings::NpVecdot, NumpyFunctions::Vecdot),
    (StaticStrings::NpMatvec, NumpyFunctions::Matvec),
    (StaticStrings::NpVecmat, NumpyFunctions::Vecmat),
    (StaticStrings::NpCross, NumpyFunctions::Cross),
    (StaticStrings::NpKron, NumpyFunctions::Kron),
    (StaticStrings::NpTrapezoid, NumpyFunctions::Trapezoid),
    (StaticStrings::NpVander, NumpyFunctions::Vander),
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
        NumpyFunctions::Array2string => call_array2string(vm, args).map(CallResult::Value),
        NumpyFunctions::ArrayRepr => call_array_repr(vm, args).map(CallResult::Value),
        NumpyFunctions::ArrayStr => call_array_str(vm, args).map(CallResult::Value),
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
        NumpyFunctions::UniqueValues => call_unique_result(vm, args, UniqueResultKind::Values).map(CallResult::Value),
        NumpyFunctions::UniqueCounts => call_unique_result(vm, args, UniqueResultKind::Counts).map(CallResult::Value),
        NumpyFunctions::UniqueInverse => call_unique_result(vm, args, UniqueResultKind::Inverse).map(CallResult::Value),
        NumpyFunctions::UniqueAll => call_unique_result(vm, args, UniqueResultKind::All).map(CallResult::Value),
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
        NumpyFunctions::Ediff1d => call_ediff1d(vm, args).map(CallResult::Value),
        NumpyFunctions::Full => call_full(vm, args).map(CallResult::Value),
        NumpyFunctions::Eye => call_eye(vm, args).map(CallResult::Value),
        NumpyFunctions::NpCopy => call_copy(vm, args).map(CallResult::Value),
        NumpyFunctions::Empty => call_empty(vm, args).map(CallResult::Value),
        NumpyFunctions::ZerosLike => call_like(vm, args, 0.0, "numpy.zeros_like").map(CallResult::Value),
        NumpyFunctions::OnesLike => call_like(vm, args, 1.0, "numpy.ones_like").map(CallResult::Value),
        NumpyFunctions::Isnan => call_bool_test(vm, args, f64::is_nan, "numpy.isnan").map(CallResult::Value),
        NumpyFunctions::Isinf => call_bool_test(vm, args, f64::is_infinite, "numpy.isinf").map(CallResult::Value),
        NumpyFunctions::Isposinf => call_bool_test(vm, args, f64_is_pos_inf, "numpy.isposinf").map(CallResult::Value),
        NumpyFunctions::Isneginf => call_bool_test(vm, args, f64_is_neg_inf, "numpy.isneginf").map(CallResult::Value),
        NumpyFunctions::Isfinite => call_bool_test(vm, args, f64::is_finite, "numpy.isfinite").map(CallResult::Value),
        NumpyFunctions::ArrayEqual => call_array_equal(vm, args).map(CallResult::Value),
        NumpyFunctions::ArrayEquiv => call_array_equiv(vm, args).map(CallResult::Value),
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
        NumpyFunctions::Take => call_take_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::TakeAlongAxis => call_take_along_axis(vm, args).map(CallResult::Value),
        NumpyFunctions::Resize => call_resize(vm, args).map(CallResult::Value),
        NumpyFunctions::Compress => call_compress_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Swapaxes => call_swapaxes_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::PermuteDims => call_permute_dims(vm, args).map(CallResult::Value),
        NumpyFunctions::MatrixTranspose => call_matrix_transpose(vm, args).map(CallResult::Value),
        NumpyFunctions::Moveaxis => call_moveaxis(vm, args).map(CallResult::Value),
        NumpyFunctions::Rollaxis => call_rollaxis(vm, args).map(CallResult::Value),
        NumpyFunctions::Rot90 => call_rot90(vm, args).map(CallResult::Value),
        NumpyFunctions::Choose => call_choose(vm, args).map(CallResult::Value),
        NumpyFunctions::FillDiagonal => call_fill_diagonal(vm, args).map(CallResult::Value),
        NumpyFunctions::Put => call_put(vm, args).map(CallResult::Value),
        NumpyFunctions::PutAlongAxis => call_put_along_axis(vm, args).map(CallResult::Value),
        NumpyFunctions::Copyto => call_copyto(vm, args).map(CallResult::Value),
        NumpyFunctions::Putmask => call_putmask(vm, args).map(CallResult::Value),
        NumpyFunctions::Place => call_place(vm, args).map(CallResult::Value),
        NumpyFunctions::Append => call_append(vm, args).map(CallResult::Value),
        NumpyFunctions::Vstack => call_vstack(vm, args).map(CallResult::Value),
        NumpyFunctions::Hstack => call_hstack(vm, args).map(CallResult::Value),
        NumpyFunctions::Dstack => call_dstack(vm, args).map(CallResult::Value),
        // Note: np.stack with axis=0 is equivalent to np.vstack for 1D inputs.
        // For 2D+ inputs, np.stack creates a new axis, which differs from vstack.
        // We only support the 1D case which is the LLM-common pattern.
        NumpyFunctions::Stack => call_vstack(vm, args).map(CallResult::Value),
        NumpyFunctions::Unstack => call_unstack(vm, args).map(CallResult::Value),
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
        NumpyFunctions::Angle => call_angle(vm, args).map(CallResult::Value),
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
        NumpyFunctions::Copysign => {
            call_numeric_binop(vm, args, f64::copysign, "numpy.copysign", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Frexp => call_unary_tuple_func(
            vm,
            args,
            numpy_frexp,
            "numpy.frexp",
            NdArrayDtype::Float64,
            NdArrayDtype::Int64,
        )
        .map(CallResult::Value),
        NumpyFunctions::Modf => call_unary_tuple_func(
            vm,
            args,
            numpy_modf,
            "numpy.modf",
            NdArrayDtype::Float64,
            NdArrayDtype::Float64,
        )
        .map(CallResult::Value),
        NumpyFunctions::Ldexp => call_ldexp(vm, args).map(CallResult::Value),
        NumpyFunctions::Gcd => call_integer_binop(vm, args, numpy_gcd, "numpy.gcd").map(CallResult::Value),
        NumpyFunctions::Lcm => call_integer_binop(vm, args, numpy_lcm, "numpy.lcm").map(CallResult::Value),
        NumpyFunctions::Logaddexp => {
            call_numeric_binop(vm, args, numpy_logaddexp, "numpy.logaddexp", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Logaddexp2 => {
            call_numeric_binop(vm, args, numpy_logaddexp2, "numpy.logaddexp2", BinopResult::Float)
                .map(CallResult::Value)
        }
        NumpyFunctions::Nextafter => {
            call_numeric_binop(vm, args, libm::nextafter, "numpy.nextafter", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Spacing => {
            call_elementwise(vm, args, numpy_spacing, "numpy.spacing", Some(NdArrayDtype::Float64))
                .map(CallResult::Value)
        }
        NumpyFunctions::Signbit => {
            call_elementwise(vm, args, signbit_as_f64, "numpy.signbit", Some(NdArrayDtype::Bool)).map(CallResult::Value)
        }
        NumpyFunctions::Sinc => {
            call_elementwise(vm, args, numpy_sinc, "numpy.sinc", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Heaviside => {
            call_numeric_binop(vm, args, numpy_heaviside, "numpy.heaviside", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Trunc => {
            call_elementwise(vm, args, f64::trunc, "numpy.trunc", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::Fix => {
            call_elementwise(vm, args, f64::trunc, "numpy.fix", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::FloatPower => {
            call_numeric_binop(vm, args, f64::powf, "numpy.float_power", BinopResult::Float).map(CallResult::Value)
        }
        NumpyFunctions::Divmod => call_numeric_tuple_binop(
            vm,
            args,
            numpy_divmod,
            "numpy.divmod",
            BinopResult::Promoted,
            BinopResult::Promoted,
        )
        .map(CallResult::Value),
        NumpyFunctions::BitwiseAnd => {
            call_bitwise_binop(vm, args, IntegerBitwiseOp::And, "numpy.bitwise_and").map(CallResult::Value)
        }
        NumpyFunctions::BitwiseOr => {
            call_bitwise_binop(vm, args, IntegerBitwiseOp::Or, "numpy.bitwise_or").map(CallResult::Value)
        }
        NumpyFunctions::BitwiseXor => {
            call_bitwise_binop(vm, args, IntegerBitwiseOp::Xor, "numpy.bitwise_xor").map(CallResult::Value)
        }
        NumpyFunctions::BitwiseNot => call_bitwise_not(vm, args).map(CallResult::Value),
        NumpyFunctions::LeftShift => {
            call_bitwise_binop(vm, args, IntegerBitwiseOp::LeftShift, "numpy.left_shift").map(CallResult::Value)
        }
        NumpyFunctions::RightShift => {
            call_bitwise_binop(vm, args, IntegerBitwiseOp::RightShift, "numpy.right_shift").map(CallResult::Value)
        }
        NumpyFunctions::BitwiseCount => call_bitwise_count(vm, args).map(CallResult::Value),
        NumpyFunctions::Packbits => call_packbits(vm, args).map(CallResult::Value),
        NumpyFunctions::Unpackbits => call_unpackbits(vm, args).map(CallResult::Value),
        NumpyFunctions::Bartlett => {
            call_window(vm, args, WindowKind::Bartlett, "numpy.bartlett").map(CallResult::Value)
        }
        NumpyFunctions::Blackman => {
            call_window(vm, args, WindowKind::Blackman, "numpy.blackman").map(CallResult::Value)
        }
        NumpyFunctions::Hamming => call_window(vm, args, WindowKind::Hamming, "numpy.hamming").map(CallResult::Value),
        NumpyFunctions::Hanning => call_window(vm, args, WindowKind::Hanning, "numpy.hanning").map(CallResult::Value),
        NumpyFunctions::Kaiser => call_kaiser(vm, args).map(CallResult::Value),
        NumpyFunctions::I0 => {
            call_elementwise(vm, args, numpy_i0, "numpy.i0", Some(NdArrayDtype::Float64)).map(CallResult::Value)
        }
        NumpyFunctions::BaseRepr => call_base_repr(vm, args).map(CallResult::Value),
        NumpyFunctions::BinaryRepr => call_binary_repr(vm, args).map(CallResult::Value),
        NumpyFunctions::Conj => call_real_identity(vm, args, "numpy.conj").map(CallResult::Value),
        NumpyFunctions::Real => call_real_identity(vm, args, "numpy.real").map(CallResult::Value),
        NumpyFunctions::RealIfClose => call_real_if_close(vm, args).map(CallResult::Value),
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
        NumpyFunctions::CanCast => call_can_cast(vm, args).map(CallResult::Value),
        NumpyFunctions::PromoteTypes => call_promote_types(vm, args).map(CallResult::Value),
        NumpyFunctions::ResultType => call_result_type(vm, args).map(CallResult::Value),
        NumpyFunctions::CommonType => call_common_type(vm, args).map(CallResult::Value),
        NumpyFunctions::MinScalarType => call_min_scalar_type(vm, args).map(CallResult::Value),
        NumpyFunctions::Mintypecode => call_mintypecode(vm, args).map(CallResult::Value),
        NumpyFunctions::Typename => call_typename(vm, args).map(CallResult::Value),
        NumpyFunctions::Geterr => call_geterr(vm, args).map(CallResult::Value),
        NumpyFunctions::Seterr => call_seterr(vm, args).map(CallResult::Value),
        NumpyFunctions::Geterrcall => call_geterrcall(vm, args).map(CallResult::Value),
        NumpyFunctions::Seterrcall => call_seterrcall(vm, args).map(CallResult::Value),
        NumpyFunctions::Errstate => call_errstate(vm, args).map(CallResult::Value),
        NumpyFunctions::GetPrintoptions => call_get_printoptions(vm, args).map(CallResult::Value),
        NumpyFunctions::SetPrintoptions => Ok(CallResult::Value(call_set_printoptions(vm, args))),
        NumpyFunctions::Printoptions => call_printoptions(vm, args).map(CallResult::Value),
        NumpyFunctions::Getbufsize => call_getbufsize(vm, args).map(CallResult::Value),
        NumpyFunctions::Setbufsize => call_setbufsize(vm, args).map(CallResult::Value),
        NumpyFunctions::ShowRuntime => Ok(CallResult::Value(call_show_runtime(vm, args))),
        NumpyFunctions::Test => Ok(CallResult::Value(call_test(vm, args))),
        NumpyFunctions::Atleast1d => call_atleast_nd(vm, args, 1, "numpy.atleast_1d").map(CallResult::Value),
        NumpyFunctions::Atleast2d => call_atleast_nd(vm, args, 2, "numpy.atleast_2d").map(CallResult::Value),
        NumpyFunctions::Atleast3d => call_atleast_nd(vm, args, 3, "numpy.atleast_3d").map(CallResult::Value),
        NumpyFunctions::DiagIndices => call_diag_indices(vm, args).map(CallResult::Value),
        NumpyFunctions::DiagIndicesFrom => call_diag_indices_from(vm, args).map(CallResult::Value),
        NumpyFunctions::TrilIndices => {
            call_triangle_indices(vm, args, TriangleKind::Lower, "numpy.tril_indices").map(CallResult::Value)
        }
        NumpyFunctions::TrilIndicesFrom => {
            call_triangle_indices_from(vm, args, TriangleKind::Lower, "numpy.tril_indices_from").map(CallResult::Value)
        }
        NumpyFunctions::TriuIndices => {
            call_triangle_indices(vm, args, TriangleKind::Upper, "numpy.triu_indices").map(CallResult::Value)
        }
        NumpyFunctions::TriuIndicesFrom => {
            call_triangle_indices_from(vm, args, TriangleKind::Upper, "numpy.triu_indices_from").map(CallResult::Value)
        }
        NumpyFunctions::Indices => call_indices(vm, args).map(CallResult::Value),
        NumpyFunctions::UnravelIndex => call_unravel_index(vm, args).map(CallResult::Value),
        NumpyFunctions::RavelMultiIndex => call_ravel_multi_index(vm, args).map(CallResult::Value),
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
        NumpyFunctions::Diagflat => call_diagflat(vm, args).map(CallResult::Value),
        NumpyFunctions::Diagonal => call_diagonal(vm, args).map(CallResult::Value),
        NumpyFunctions::Trace => call_trace(vm, args).map(CallResult::Value),
        NumpyFunctions::Flatnonzero => call_flatnonzero(vm, args).map(CallResult::Value),
        NumpyFunctions::Asarray => call_asarray(vm, args).map(CallResult::Value),
        NumpyFunctions::AsarrayChkfinite => call_asarray_chkfinite(vm, args).map(CallResult::Value),
        NumpyFunctions::Ascontiguousarray | NumpyFunctions::Asfortranarray | NumpyFunctions::Require => {
            call_asarray_compat(vm, args).map(CallResult::Value)
        }
        NumpyFunctions::Ix => call_ix(vm, args).map(CallResult::Value),
        NumpyFunctions::MaskIndices => call_mask_indices(vm, args).map(CallResult::Value),
        NumpyFunctions::Isfortran => call_isfortran(vm, args).map(CallResult::Value),
        NumpyFunctions::MayShareMemory => {
            call_memory_overlap(vm, args, "numpy.may_share_memory").map(CallResult::Value)
        }
        NumpyFunctions::SharesMemory => call_memory_overlap(vm, args, "numpy.shares_memory").map(CallResult::Value),
        NumpyFunctions::ColumnStack => call_column_stack(vm, args).map(CallResult::Value),
        NumpyFunctions::RowStack => call_vstack(vm, args).map(CallResult::Value), // alias
        NumpyFunctions::Hsplit => call_hsplit(vm, args).map(CallResult::Value),
        NumpyFunctions::Vsplit => call_vsplit(vm, args).map(CallResult::Value),
        NumpyFunctions::Dsplit => call_dsplit(vm, args).map(CallResult::Value),
        NumpyFunctions::ArraySplit => call_array_split(vm, args).map(CallResult::Value),
        NumpyFunctions::FullLike => call_full_like(vm, args).map(CallResult::Value),
        NumpyFunctions::EmptyLike => call_like(vm, args, 0.0, "numpy.empty_like").map(CallResult::Value),
        // Phase 7: Sorting, searching, set ops
        NumpyFunctions::ArgsortMod => call_argsort_mod(vm, args).map(CallResult::Value),
        NumpyFunctions::Argpartition => call_argpartition(vm, args).map(CallResult::Value),
        NumpyFunctions::Partition => call_partition(vm, args).map(CallResult::Value),
        NumpyFunctions::Lexsort => call_lexsort(vm, args).map(CallResult::Value),
        NumpyFunctions::Cov => call_cov(vm, args).map(CallResult::Value),
        NumpyFunctions::Corrcoef => call_corrcoef(vm, args).map(CallResult::Value),
        NumpyFunctions::Searchsorted => call_searchsorted(vm, args).map(CallResult::Value),
        NumpyFunctions::Extract => call_extract(vm, args).map(CallResult::Value),
        NumpyFunctions::TrimZeros => call_trim_zeros(vm, args).map(CallResult::Value),
        NumpyFunctions::Unwrap => call_unwrap(vm, args).map(CallResult::Value),
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
        NumpyFunctions::Vecdot => call_dot(vm, args).map(CallResult::Value), // 1D vector subset
        NumpyFunctions::Matvec | NumpyFunctions::Vecmat => call_matmul(vm, args).map(CallResult::Value),
        NumpyFunctions::Cross => call_cross(vm, args).map(CallResult::Value),
        NumpyFunctions::Kron => call_kron(vm, args).map(CallResult::Value),
        NumpyFunctions::Trapezoid => call_trapezoid(vm, args).map(CallResult::Value),
        NumpyFunctions::Vander => call_vander(vm, args).map(CallResult::Value),
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

/// `numpy.array2string(a)` — format an ndarray without the `array(...)` wrapper.
fn call_array2string(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arr = array_display_arg(vm, args, "numpy.array2string")?;
    let mut output = String::new();
    arr.array_str_fmt_inner(&mut output)?;
    allocate_string(output, vm.heap)
}

/// `numpy.array_repr(a)` — return the ndarray repr string.
fn call_array_repr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arr = array_display_arg(vm, args, "numpy.array_repr")?;
    let mut output = String::new();
    arr.py_repr_fmt_inner(&mut output)?;
    allocate_string(output, vm.heap)
}

/// `numpy.array_str(a)` — return NumPy's bare ndarray string.
fn call_array_str(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arr = array_display_arg(vm, args, "numpy.array_str")?;
    let mut output = String::new();
    arr.array_str_fmt_inner(&mut output)?;
    allocate_string(output, vm.heap)
}

/// Extracts the ndarray argument for display helpers and ignores optional print-only arguments.
fn array_display_arg(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, name: &str) -> RunResult<NdArray> {
    let (mut pos, kwargs) = args.into_parts();
    let Some(arg) = pos.next() else {
        pos.drop_with_heap(vm);
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_at_least(name, 1, 0));
    };
    defer_drop!(arg, vm);
    pos.drop_with_heap(vm);
    kwargs.drop_with_heap(vm);
    ndarray_from_value(arg, name, vm)
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

/// Return shape for the Array API style `unique_*` helpers.
#[derive(Clone, Copy)]
enum UniqueResultKind {
    /// `unique_values(x)` returns only the unique values ndarray.
    Values,
    /// `unique_counts(x)` returns values plus occurrence counts.
    Counts,
    /// `unique_inverse(x)` returns values plus inverse indices.
    Inverse,
    /// `unique_all(x)` returns values, first indices, inverse indices, and counts.
    All,
}

/// Precomputed unique-result arrays shared by the `unique_*` wrappers.
struct UniqueAnalysis {
    /// Sorted unique values.
    values: Vec<f64>,
    /// First original index for each unique value.
    first_indices: Vec<usize>,
    /// Inverse index for each input element.
    inverse_indices: Vec<usize>,
    /// Occurrence count for each unique value.
    counts: Vec<usize>,
}

/// Shared implementation for NumPy's Array API `unique_*` helpers.
fn call_unique_result(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    kind: UniqueResultKind,
) -> RunResult<Value> {
    let name = match kind {
        UniqueResultKind::Values => "numpy.unique_values",
        UniqueResultKind::Counts => "numpy.unique_counts",
        UniqueResultKind::Inverse => "numpy.unique_inverse",
        UniqueResultKind::All => "numpy.unique_all",
    };
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, name, vm)?;
    let analysis = unique_analysis(&arr);

    match kind {
        UniqueResultKind::Values => allocate_unique_values_array(&analysis, arr.dtype(), vm.heap),
        UniqueResultKind::Counts => {
            let values = allocate_unique_values_array(&analysis, arr.dtype(), vm.heap)?;
            let counts = allocate_usize_array(&analysis.counts, vec![analysis.counts.len()], vm.heap)?;
            allocate_namedtuple_result("UniqueCountsResult", &["values", "counts"], vec![values, counts], vm)
        }
        UniqueResultKind::Inverse => {
            let values = allocate_unique_values_array(&analysis, arr.dtype(), vm.heap)?;
            let inverse_indices = allocate_usize_array(&analysis.inverse_indices, arr.shape().to_vec(), vm.heap)?;
            allocate_namedtuple_result(
                "UniqueInverseResult",
                &["values", "inverse_indices"],
                vec![values, inverse_indices],
                vm,
            )
        }
        UniqueResultKind::All => {
            let values = allocate_unique_values_array(&analysis, arr.dtype(), vm.heap)?;
            let indices = allocate_usize_array(&analysis.first_indices, vec![analysis.first_indices.len()], vm.heap)?;
            let inverse_indices = allocate_usize_array(&analysis.inverse_indices, arr.shape().to_vec(), vm.heap)?;
            let counts = allocate_usize_array(&analysis.counts, vec![analysis.counts.len()], vm.heap)?;
            allocate_namedtuple_result(
                "UniqueAllResult",
                &["values", "indices", "inverse_indices", "counts"],
                vec![values, indices, inverse_indices, counts],
                vm,
            )
        }
    }
}

/// Computes sorted unique values, first indices, inverse indices, and counts.
fn unique_analysis(arr: &NdArray) -> UniqueAnalysis {
    let mut pairs: Vec<(f64, usize)> = arr
        .data()
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect();
    pairs.sort_by(|(left, _), (right, _)| nan_last_cmp(left, right));

    let mut values = Vec::new();
    let mut first_indices = Vec::new();
    let mut inverse_indices = vec![0; arr.len()];
    let mut counts = Vec::new();

    for (value, original_index) in pairs {
        let group = if values.last().is_some_and(|last| f64_exact_equal(*last, value)) {
            values.len() - 1
        } else {
            values.push(value);
            first_indices.push(original_index);
            counts.push(0);
            values.len() - 1
        };
        first_indices[group] = first_indices[group].min(original_index);
        counts[group] += 1;
        inverse_indices[original_index] = group;
    }

    UniqueAnalysis {
        values,
        first_indices,
        inverse_indices,
        counts,
    }
}

/// Allocates the unique values ndarray.
fn allocate_unique_values_array(
    analysis: &UniqueAnalysis,
    dtype: NdArrayDtype,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    let values = analysis.values.clone();
    let result = NdArray::new(values, vec![analysis.values.len()], dtype);
    Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
}

/// Allocates an int64 ndarray from usize values.
fn allocate_usize_array(values: &[usize], shape: Vec<usize>, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    let data = values.iter().copied().map(usize_to_f64).collect();
    let result = NdArray::new(data, shape, NdArrayDtype::Int64);
    Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
}

/// Allocates a namedtuple-style result object for `unique_*` helpers.
fn allocate_namedtuple_result(
    type_name: &str,
    fields: &[&str],
    values: Vec<Value>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let field_names = fields.iter().map(|field| (*field).to_owned().into()).collect();
    let result = NamedTuple::new(type_name.to_owned(), field_names, values);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(result))?))
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

/// `numpy.ediff1d(a)` — flattened first-order difference.
fn call_ediff1d(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.ediff1d", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.ediff1d", vm)?;
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

/// Predicate for positive infinity.
fn f64_is_pos_inf(value: f64) -> bool {
    value.is_infinite() && value.is_sign_positive()
}

/// Predicate for negative infinity.
fn f64_is_neg_inf(value: f64) -> bool {
    value.is_infinite() && value.is_sign_negative()
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

/// Numeric input normalized for `numpy.array_equiv`.
enum ArrayEquivInput {
    /// Array-like input with copied data and shape.
    Array { data: Vec<f64>, shape: Vec<usize> },
    /// Numeric scalar input.
    Scalar(f64),
}

/// `numpy.array_equiv(a, b)` — equality with scalar broadcasting.
fn call_array_equiv(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.array_equiv", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a = array_equiv_input(a_val, "numpy.array_equiv", vm)?;
    let b = array_equiv_input(b_val, "numpy.array_equiv", vm)?;
    Ok(Value::Bool(array_equiv_inputs(&a, &b)))
}

/// Converts a value into the scalar-or-array form used by `array_equiv`.
fn array_equiv_input(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<ArrayEquivInput> {
    if let Ok((data, shape, _)) = extract_ndarray_info(value, name, vm) {
        Ok(ArrayEquivInput::Array { data, shape })
    } else {
        let (value, _) = numeric_scalar_info(value, name, vm)?;
        Ok(ArrayEquivInput::Scalar(value))
    }
}

/// Compares `array_equiv` inputs with scalar broadcasting.
fn array_equiv_inputs(a: &ArrayEquivInput, b: &ArrayEquivInput) -> bool {
    match (a, b) {
        (
            ArrayEquivInput::Array {
                data: a_data,
                shape: a_shape,
            },
            ArrayEquivInput::Array {
                data: b_data,
                shape: b_shape,
            },
        ) => a_shape == b_shape && a_data == b_data,
        (ArrayEquivInput::Array { data, .. }, ArrayEquivInput::Scalar(value))
        | (ArrayEquivInput::Scalar(value), ArrayEquivInput::Array { data, .. }) => {
            data.iter().all(|item| f64_exact_equal(*item, *value))
        }
        (ArrayEquivInput::Scalar(a), ArrayEquivInput::Scalar(b)) => f64_exact_equal(*a, *b),
    }
}

/// Exact float equality with NumPy's NaN-is-not-equal behavior and clippy-friendly spelling.
fn f64_exact_equal(a: f64, b: f64) -> bool {
    a.partial_cmp(&b) == Some(Ordering::Equal)
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

/// `numpy.resize(a, new_shape)` — repeat flattened input data into a new shape.
fn call_resize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.resize", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.resize", 2, 0))?;
    defer_drop!(arr_val, vm);
    let shape_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.resize", 2, 1))?;
    defer_drop!(shape_val, vm);
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.resize", 2, 3));
    }

    let arr = ndarray_from_value(arr_val, "numpy.resize", vm)?;
    let shape = extract_shape_from_value(shape_val, "numpy.resize", vm)?;
    let total = shape.iter().product::<usize>();
    check_array_alloc_size(total, vm.heap.tracker())?;
    let data = if total == 0 {
        Vec::new()
    } else if arr.data().is_empty() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "cannot resize an empty array").into());
    } else {
        (0..total)
            .map(|index| arr.data()[index % arr.len()])
            .collect::<Vec<_>>()
    };
    let result = NdArray::new(data, shape, arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.transpose(a, axes=None)` — transpose an array (module-level wrapper).
fn call_transpose_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    call_permute_dims_named(vm, args, "numpy.transpose")
}

/// `numpy.take(a, indices)` — gather flattened elements at integer indices.
///
/// Monty supports the default flattened mode. The optional `axis`, `out`, and
/// `mode` arguments are outside the current ndarray subset and must be omitted
/// or passed as `None` for `axis`.
fn call_take_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.take", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.take", 2, 0))?;
    defer_drop!(arr_val, vm);
    let indices_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.take", 2, 1))?;
    defer_drop!(indices_val, vm);

    if let Some(axis_val) = pos.next() {
        defer_drop!(axis_val, vm);
        if !matches!(axis_val, Value::None) {
            return Err(ExcType::type_error("numpy.take() axis is not supported yet"));
        }
    }
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, "numpy.take", vm)?;
    if let Value::Int(index) = indices_val {
        let resolved = resolve_flat_index(*index, arr.len())?;
        Ok(ndarray_element_to_value(&arr, arr.data()[resolved]))
    } else {
        let indices = ndarray_from_value(indices_val, "numpy.take", vm)?;
        take_flat_indices(&arr, &indices, vm.heap)
    }
}

/// `numpy.take_along_axis(a, indices, axis)` — gather per-axis positions from an array.
fn call_take_along_axis(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, vm);
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.take_along_axis", 3, 0))?;
    defer_drop!(arr_val, vm);
    let indices_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.take_along_axis", 3, 1))?;
    defer_drop!(indices_val, vm);
    let axis_value = pos.next();
    defer_drop_mut!(axis_value, vm);
    if pos.len() != 0 {
        return Err(ExcType::type_error_at_most("numpy.take_along_axis", 3, 3 + pos.len()));
    }
    parse_axis_keyword(kwargs_iter, axis_value, "numpy.take_along_axis", vm)?;
    let Some(axis_value) = axis_value.as_ref() else {
        return Err(ExcType::type_error_at_least("numpy.take_along_axis", 3, 2));
    };

    let arr = ndarray_from_value(arr_val, "numpy.take_along_axis", vm)?;
    let indices = ndarray_from_value(indices_val, "numpy.take_along_axis", vm)?;
    let axis = normalize_axis(
        value_to_i64_arg(axis_value, "numpy.take_along_axis", "axis")?,
        arr.ndim(),
        "numpy.take_along_axis",
    )?;
    let result = take_along_axis_array(&arr, &indices, axis, "numpy.take_along_axis")?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.compress(condition, a)` — select flattened elements where condition is true.
///
/// The optional `axis` and `out` arguments are not modeled yet. Omitting `axis`
/// matches the flattened behavior of NumPy and the existing ndarray method.
fn call_compress_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.compress", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let condition_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.compress", 2, 0))?;
    defer_drop!(condition_val, vm);
    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.compress", 2, 1))?;
    defer_drop!(arr_val, vm);

    if let Some(axis_val) = pos.next() {
        defer_drop!(axis_val, vm);
        if !matches!(axis_val, Value::None) {
            return Err(ExcType::type_error("numpy.compress() axis is not supported yet"));
        }
    }
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let condition = ndarray_from_value(condition_val, "numpy.compress", vm)?;
    let arr = ndarray_from_value(arr_val, "numpy.compress", vm)?;
    arr.compress(&condition, vm.heap)
}

/// `numpy.swapaxes(a, axis1, axis2)` — swap two axes of an array.
fn call_swapaxes_mod(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.swapaxes", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.swapaxes", 3, 0))?;
    defer_drop!(arr_val, vm);
    let axis1_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.swapaxes", 3, 1))?;
    defer_drop!(axis1_val, vm);
    let axis2_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.swapaxes", 3, 2))?;
    defer_drop!(axis2_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, "numpy.swapaxes", vm)?;
    let axis1 = normalize_axis(
        value_to_i64_arg(axis1_val, "numpy.swapaxes", "axis1")?,
        arr.ndim(),
        "numpy.swapaxes",
    )?;
    let axis2 = normalize_axis(
        value_to_i64_arg(axis2_val, "numpy.swapaxes", "axis2")?,
        arr.ndim(),
        "numpy.swapaxes",
    )?;
    let mut axes: Vec<usize> = (0..arr.ndim()).collect();
    axes.swap(axis1, axis2);
    permute_ndarray_axes(&arr, &axes, vm.heap, "numpy.swapaxes")
}

/// `numpy.permute_dims(a, axes=None)` — permute ndarray axes.
fn call_permute_dims(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    call_permute_dims_named(vm, args, "numpy.permute_dims")
}

/// Shared implementation for `transpose` and `permute_dims`.
fn call_permute_dims_named(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    name: &'static str,
) -> RunResult<Value> {
    let pos = args.into_pos_only(name, vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos.next().ok_or_else(|| ExcType::type_error_at_least(name, 1, 0))?;
    defer_drop!(arr_val, vm);
    let axes_val = pos.next();
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, name, vm)?;
    let axes = if let Some(axes_val) = axes_val {
        defer_drop!(axes_val, vm);
        axes_permutation_from_value(axes_val, arr.ndim(), name, vm)?
    } else {
        default_transpose_axes(arr.ndim())
    };
    permute_ndarray_axes(&arr, &axes, vm.heap, name)
}

/// `numpy.matrix_transpose(a)` — swap the last two axes of an array.
fn call_matrix_transpose(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.matrix_transpose", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.matrix_transpose", vm)?;
    let ndim = arr.ndim();
    if ndim < 2 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Input array must be at least 2-dimensional, but it is {ndim}"),
        )
        .into())
    } else {
        let mut axes: Vec<usize> = (0..ndim).collect();
        axes.swap(ndim - 2, ndim - 1);
        permute_ndarray_axes(&arr, &axes, vm.heap, "numpy.matrix_transpose")
    }
}

/// `numpy.moveaxis(a, source, destination)` — move axes to new positions.
fn call_moveaxis(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.moveaxis", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.moveaxis", 3, 0))?;
    defer_drop!(arr_val, vm);
    let source_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.moveaxis", 3, 1))?;
    defer_drop!(source_val, vm);
    let destination_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.moveaxis", 3, 2))?;
    defer_drop!(destination_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, "numpy.moveaxis", vm)?;
    let source = axis_list_from_value(source_val, arr.ndim(), "numpy.moveaxis", "source", vm)?;
    let destination = axis_list_from_value(destination_val, arr.ndim(), "numpy.moveaxis", "destination", vm)?;
    let axes = moveaxis_permutation(arr.ndim(), &source, &destination)?;
    permute_ndarray_axes(&arr, &axes, vm.heap, "numpy.moveaxis")
}

/// `numpy.rollaxis(a, axis, start=0)` — roll an axis backward to a target position.
fn call_rollaxis(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.rollaxis", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.rollaxis", 2, 0))?;
    defer_drop!(arr_val, vm);
    let axis_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.rollaxis", 2, 1))?;
    defer_drop!(axis_val, vm);
    let start_val = pos.next();
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, "numpy.rollaxis", vm)?;
    let axis = normalize_axis(
        value_to_i64_arg(axis_val, "numpy.rollaxis", "axis")?,
        arr.ndim(),
        "numpy.rollaxis",
    )?;
    let start = if let Some(start_val) = start_val {
        defer_drop!(start_val, vm);
        normalize_rollaxis_start(value_to_i64_arg(start_val, "numpy.rollaxis", "start")?, arr.ndim())?
    } else {
        0
    };
    let axes = rollaxis_permutation(arr.ndim(), axis, start);
    permute_ndarray_axes(&arr, &axes, vm.heap, "numpy.rollaxis")
}

/// `numpy.rot90(a, k=1)` — rotate a 2-D array by 90-degree increments.
fn call_rot90(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.rot90", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.rot90", 1, 0))?;
    defer_drop!(arr_val, vm);
    let k_val = pos.next();
    let axes_val = pos.next();
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let arr = ndarray_from_value(arr_val, "numpy.rot90", vm)?;
    let k = if let Some(k_val) = k_val {
        defer_drop!(k_val, vm);
        value_to_i64_arg(k_val, "numpy.rot90", "k")?
    } else {
        1
    };
    let axes = if let Some(axes_val) = axes_val {
        defer_drop!(axes_val, vm);
        axis_pair_from_value(axes_val, arr.ndim(), "numpy.rot90", "axes", vm)?
    } else {
        default_axis_pair(arr.ndim(), "numpy.rot90")?
    };
    rot90_ndarray(&arr, k, axes, vm.heap)
}

/// `numpy.choose(a, choices)` — choose values from a sequence by integer index array.
fn call_choose(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.choose", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let index_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.choose", 2, 0))?;
    defer_drop!(index_val, vm);
    let choices_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.choose", 2, 1))?;
    defer_drop!(choices_val, vm);
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let indices = ndarray_from_value(index_val, "numpy.choose", vm)?;
    let choice_items = sequence_items(choices_val, "numpy.choose", vm)?;
    defer_drop!(choice_items, vm);
    let result = choose_from_arrays(&indices, choice_items, "numpy.choose", vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Choice buffer used by `numpy.choose`.
struct ChoiceData {
    /// Flat scalar or array values for one choice branch.
    values: Vec<f64>,
    /// Compact dtype for the choice branch.
    dtype: NdArrayDtype,
}

/// Builds the output array for `numpy.choose` from validated choice branches.
fn choose_from_arrays(
    indices: &NdArray,
    choice_items: &[Value],
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<NdArray> {
    if choice_items.is_empty() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "invalid entry in choice array").into());
    }

    let mut choices = Vec::with_capacity(choice_items.len());
    let mut dtype = NdArrayDtype::Bool;
    for choice in choice_items {
        let choice_data = choice_data_from_value(choice, indices.len(), name, vm)?;
        dtype = if choices.is_empty() {
            choice_data.dtype
        } else {
            promote_dtype(dtype, choice_data.dtype)
        };
        choices.push(choice_data);
    }

    let mut data = Vec::with_capacity(indices.len());
    for (offset, raw_index) in indices.data().iter().copied().enumerate() {
        let choice_index = choice_index_from_f64(raw_index, choices.len())?;
        data.push(broadcast_value_at(&choices[choice_index].values, offset));
    }
    Ok(NdArray::new(data, indices.shape().to_vec(), dtype))
}

/// Converts one `choose` branch into a scalar or index-shaped value buffer.
fn choice_data_from_value(
    value: &Value,
    output_len: usize,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<ChoiceData> {
    if let Ok((scalar, dtype)) = numeric_scalar_info(value, name, vm) {
        Ok(ChoiceData {
            values: vec![scalar],
            dtype,
        })
    } else {
        let arr = ndarray_from_value(value, name, vm)?;
        validate_broadcast_values(arr.data(), output_len, name)?;
        Ok(ChoiceData {
            values: arr.data().to_vec(),
            dtype: arr.dtype(),
        })
    }
}

/// Converts a numeric `choose` selector into a branch index.
fn choice_index_from_f64(value: f64, choice_count: usize) -> RunResult<usize> {
    #[expect(clippy::cast_possible_truncation, reason = "choice index from numeric ndarray")]
    let index = value as i64;
    if index < 0 || usize::try_from(index).map_or(true, |index| index >= choice_count) {
        Err(SimpleException::new_msg(ExcType::ValueError, "invalid entry in choice array").into())
    } else {
        usize::try_from(index)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "choice index is too large").into())
    }
}

/// Returns the default transpose permutation, which reverses axis order.
fn default_transpose_axes(ndim: usize) -> Vec<usize> {
    (0..ndim).rev().collect()
}

/// Parses a full axis permutation for `transpose`-style calls.
fn axes_permutation_from_value(
    value: &Value,
    ndim: usize,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    if matches!(value, Value::None) {
        Ok(default_transpose_axes(ndim))
    } else {
        let axes = axis_sequence_from_value(value, ndim, name, "axes", vm)?;
        ensure_axes_are_permutation(&axes, ndim, name)?;
        Ok(axes)
    }
}

/// Parses a list or tuple of axes without accepting scalar shorthand.
fn axis_sequence_from_value(
    value: &Value,
    ndim: usize,
    name: &str,
    arg_name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    match value {
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::List(list) => axis_sequence_from_items(list.as_slice(), ndim, name, arg_name),
            HeapData::Tuple(tuple) => axis_sequence_from_items(tuple.as_slice(), ndim, name, arg_name),
            _ => Err(ExcType::type_error(format!(
                "{name}() {arg_name} must be a tuple or list of integers"
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{name}() {arg_name} must be a tuple or list of integers"
        ))),
    }
}

/// Parses either a scalar axis or a list/tuple of axes.
fn axis_list_from_value(
    value: &Value,
    ndim: usize,
    name: &str,
    arg_name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    match value {
        Value::Int(axis) => Ok(vec![normalize_axis(*axis, ndim, name)?]),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::List(list) => {
                let axes = axis_sequence_from_items(list.as_slice(), ndim, name, arg_name)?;
                ensure_unique_axes(&axes, name)?;
                Ok(axes)
            }
            HeapData::Tuple(tuple) => {
                let axes = axis_sequence_from_items(tuple.as_slice(), ndim, name, arg_name)?;
                ensure_unique_axes(&axes, name)?;
                Ok(axes)
            }
            _ => Err(ExcType::type_error(format!(
                "{name}() {arg_name} must be an integer or tuple of integers"
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{name}() {arg_name} must be an integer or tuple of integers"
        ))),
    }
}

/// Converts a sequence of axis values into normalized axis indices.
fn axis_sequence_from_items(items: &[Value], ndim: usize, name: &str, arg_name: &str) -> RunResult<Vec<usize>> {
    items
        .iter()
        .map(|item| value_to_i64_arg(item, name, arg_name).and_then(|axis| normalize_axis(axis, ndim, name)))
        .collect()
}

/// Parses the two-axis tuple used by `rot90`.
fn axis_pair_from_value(
    value: &Value,
    ndim: usize,
    name: &str,
    arg_name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<[usize; 2]> {
    let axes = axis_sequence_from_value(value, ndim, name, arg_name, vm)?;
    if axes.len() != 2 {
        Err(ExcType::type_error(format!(
            "{name}() {arg_name} must contain exactly two axes"
        )))
    } else if axes[0] == axes[1] {
        Err(SimpleException::new_msg(ExcType::ValueError, "Axes must be different.").into())
    } else {
        Ok([axes[0], axes[1]])
    }
}

/// Returns the default `rot90` axes, validating that the array is at least 2-D.
fn default_axis_pair(ndim: usize, name: &str) -> RunResult<[usize; 2]> {
    if ndim < 2 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{name}() requires an array of at least two dimensions"),
        )
        .into())
    } else {
        Ok([0, 1])
    }
}

/// Normalizes a possibly negative axis into a valid dimension index.
fn normalize_axis(axis: i64, ndim: usize, name: &str) -> RunResult<usize> {
    let ndim_i64 = i64::try_from(ndim)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, format!("{name}() ndim is too large")))?;
    let normalized = if axis < 0 { axis + ndim_i64 } else { axis };
    if normalized < 0 || normalized >= ndim_i64 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("bad axis for array with {ndim} dimensions"),
        )
        .into())
    } else {
        usize::try_from(normalized)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, format!("{name}() axis is too large")).into())
    }
}

/// Normalizes `rollaxis(start)`, whose insertion point may be equal to `ndim`.
fn normalize_rollaxis_start(start: i64, ndim: usize) -> RunResult<usize> {
    let ndim_i64 = i64::try_from(ndim)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "numpy.rollaxis() ndim is too large"))?;
    let normalized = if start < 0 { start + ndim_i64 } else { start };
    if normalized < 0 || normalized > ndim_i64 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("bad axis for array with {ndim} dimensions"),
        )
        .into())
    } else {
        usize::try_from(normalized)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "numpy.rollaxis() start is too large").into())
    }
}

/// Validates that an axis list has no duplicates.
fn ensure_unique_axes(axes: &[usize], name: &str) -> RunResult<()> {
    for (index, axis) in axes.iter().enumerate() {
        if axes[..index].contains(axis) {
            return Err(SimpleException::new_msg(ExcType::ValueError, format!("{name}() repeated axis")).into());
        }
    }
    Ok(())
}

/// Validates that an axis list contains each axis exactly once.
fn ensure_axes_are_permutation(axes: &[usize], ndim: usize, name: &str) -> RunResult<()> {
    if axes.len() == ndim {
        ensure_unique_axes(axes, name)
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("{name}() axes don't match array")).into())
    }
}

/// Builds the axis order used by `moveaxis`.
fn moveaxis_permutation(ndim: usize, source: &[usize], destination: &[usize]) -> RunResult<Vec<usize>> {
    if source.len() == destination.len() {
        let mut axes: Vec<usize> = (0..ndim).filter(|axis| !source.contains(axis)).collect();
        let mut moves: Vec<(usize, usize)> = destination.iter().copied().zip(source.iter().copied()).collect();
        moves.sort_by_key(|(dest, _)| *dest);
        for (dest, src) in moves {
            axes.insert(dest, src);
        }
        Ok(axes)
    } else {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            "numpy.moveaxis() source and destination arguments must have the same number of elements",
        )
        .into())
    }
}

/// Builds the axis order used by `rollaxis`.
fn rollaxis_permutation(ndim: usize, axis: usize, start: usize) -> Vec<usize> {
    let mut insert_at = start;
    if axis < insert_at {
        insert_at -= 1;
    }
    let mut axes: Vec<usize> = (0..ndim).collect();
    axes.remove(axis);
    axes.insert(insert_at, axis);
    axes
}

/// Allocates an ndarray with axes permuted according to NumPy row-major order.
fn permute_ndarray_axes(
    arr: &NdArray,
    axes: &[usize],
    heap: &Heap<impl ResourceTracker>,
    name: &str,
) -> RunResult<Value> {
    ensure_axes_are_permutation(axes, arr.ndim(), name)?;
    let new_shape: Vec<usize> = axes.iter().map(|&axis| arr.shape()[axis]).collect();
    if axes.iter().copied().eq(0..arr.ndim()) || arr.ndim() <= 1 {
        let result = NdArray::new(arr.data().to_vec(), new_shape, arr.dtype());
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    } else {
        let old_strides = row_major_strides(arr.shape());
        let new_strides = row_major_strides(&new_shape);
        let mut data = vec![0.0; arr.len()];
        for (old_flat, value) in arr.data().iter().copied().enumerate() {
            let old_coords = coords_from_flat_index(old_flat, arr.shape(), &old_strides);
            let new_flat = axes
                .iter()
                .enumerate()
                .map(|(new_axis, &old_axis)| old_coords[old_axis] * new_strides[new_axis])
                .sum::<usize>();
            data[new_flat] = value;
        }
        let result = NdArray::new(data, new_shape, arr.dtype());
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    }
}

/// Computes row-major strides for a shape.
fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1; shape.len()];
    let mut stride = 1usize;
    for axis in (0..shape.len()).rev() {
        strides[axis] = stride;
        stride = stride.saturating_mul(shape[axis]);
    }
    strides
}

/// Converts a flat row-major index into coordinate components.
fn coords_from_flat_index(flat: usize, shape: &[usize], strides: &[usize]) -> Vec<usize> {
    shape
        .iter()
        .zip(strides.iter())
        .map(|(&dim, &stride)| if dim == 0 { 0 } else { (flat / stride) % dim })
        .collect()
}

/// Parses an optional `axis` keyword into the shared axis value slot.
fn parse_axis_keyword(
    kwargs_iter: &mut impl Iterator<Item = (Value, Value)>,
    axis_value: &mut Option<Value>,
    name: &str,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    for (key, value) in kwargs_iter {
        defer_drop!(key, vm);
        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            value.drop_with_heap(vm);
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let key_str = keyword_name.as_str(vm.interns);
        if key_str == "axis" {
            if axis_value.is_some() {
                value.drop_with_heap(vm);
                return Err(ExcType::type_error_duplicate_arg(name, key_str));
            }
            *axis_value = Some(value);
        } else {
            value.drop_with_heap(vm);
            return Err(ExcType::type_error_unexpected_keyword(name, key_str));
        }
    }
    Ok(())
}

/// Implements flattened `take` while preserving the shape of the indices array.
fn take_flat_indices(arr: &NdArray, indices: &NdArray, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut data = Vec::with_capacity(indices.len());
    for index in indices.data().iter().copied() {
        #[expect(clippy::cast_possible_truncation, reason = "index from numeric ndarray")]
        let resolved = resolve_flat_index(index as i64, arr.len())?;
        data.push(arr.data()[resolved]);
    }
    let result = NdArray::new(data, indices.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
}

/// Implements `take_along_axis` by resolving every indexed output coordinate.
fn take_along_axis_array(arr: &NdArray, indices: &NdArray, axis: usize, name: &str) -> RunResult<NdArray> {
    let targets = along_axis_flat_indices(arr.shape(), indices, axis, name)?;
    let data = targets.into_iter().map(|target| arr.data()[target]).collect::<Vec<_>>();
    Ok(NdArray::new(data, indices.shape().to_vec(), arr.dtype()))
}

/// Resolves every `indices` entry into a flat row-major index for an array shape.
fn along_axis_flat_indices(arr_shape: &[usize], indices: &NdArray, axis: usize, name: &str) -> RunResult<Vec<usize>> {
    validate_along_axis_shapes(arr_shape, indices.shape(), axis, name)?;
    let arr_strides = row_major_strides(arr_shape);
    let index_strides = row_major_strides(indices.shape());
    let mut targets = Vec::with_capacity(indices.len());
    for (flat, raw_index) in indices.data().iter().copied().enumerate() {
        let mut coords = coords_from_flat_index(flat, indices.shape(), &index_strides);
        #[expect(clippy::cast_possible_truncation, reason = "axis index from numeric ndarray")]
        {
            coords[axis] = resolve_flat_index(raw_index as i64, arr_shape[axis])?;
        }
        let target = coords
            .iter()
            .zip(arr_strides.iter())
            .map(|(coord, stride)| coord * stride)
            .sum::<usize>();
        targets.push(target);
    }
    Ok(targets)
}

/// Validates the shared-dimensional shape rule used by NumPy's along-axis helpers.
fn validate_along_axis_shapes(arr_shape: &[usize], index_shape: &[usize], axis: usize, name: &str) -> RunResult<()> {
    if arr_shape.len() != index_shape.len() {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{name}() indices and arr must have the same number of dimensions"),
        )
        .into())
    } else if arr_shape
        .iter()
        .zip(index_shape.iter())
        .enumerate()
        .any(|(dim, (arr_dim, index_dim))| dim != axis && arr_dim != index_dim)
    {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{name}() shape mismatch outside the indexed axis"),
        )
        .into())
    } else {
        Ok(())
    }
}

/// Resolves a possibly negative flattened index.
fn resolve_flat_index(index: i64, len: usize) -> RunResult<usize> {
    let len_i64 =
        i64::try_from(len).map_err(|_| SimpleException::new_msg(ExcType::ValueError, "array is too large"))?;
    let resolved = if index < 0 { index + len_i64 } else { index };
    if resolved < 0 || resolved >= len_i64 {
        Err(SimpleException::new_msg(ExcType::IndexError, "index out of range").into())
    } else {
        usize::try_from(resolved)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "index is too large").into())
    }
}

/// Converts a raw ndarray element back to the public scalar Value for its dtype.
fn ndarray_element_to_value(arr: &NdArray, value: f64) -> Value {
    match arr.dtype() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64 to i64 truncation is the intended int conversion"
        )]
        NdArrayDtype::Int64 => Value::Int(value as i64),
        NdArrayDtype::Float64 => Value::Float(value),
        NdArrayDtype::Bool => Value::Bool(value != 0.0),
    }
}

/// Rotates a 2-D ndarray by `k` quarter turns across the requested axis pair.
fn rot90_ndarray(arr: &NdArray, k: i64, axes: [usize; 2], heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    if arr.ndim() != 2 {
        Err(SimpleException::new_msg(ExcType::ValueError, "numpy.rot90() only supports 2-D arrays").into())
    } else if axes != [0, 1] && axes != [1, 0] {
        Err(SimpleException::new_msg(ExcType::ValueError, "numpy.rot90() only supports axes (0, 1)").into())
    } else {
        let adjusted_k = if axes == [1, 0] { -k } else { k };
        let k = adjusted_k.rem_euclid(4);
        let rows = arr.shape()[0];
        let cols = arr.shape()[1];
        let (data, shape) = match k {
            0 => (arr.data().to_vec(), arr.shape().to_vec()),
            1 => {
                let mut data = Vec::with_capacity(arr.len());
                for col in (0..cols).rev() {
                    for row in 0..rows {
                        data.push(arr.data()[row * cols + col]);
                    }
                }
                (data, vec![cols, rows])
            }
            2 => {
                let mut data = arr.data().to_vec();
                data.reverse();
                (data, arr.shape().to_vec())
            }
            _ => {
                let mut data = Vec::with_capacity(arr.len());
                for col in 0..cols {
                    for row in (0..rows).rev() {
                        data.push(arr.data()[row * cols + col]);
                    }
                }
                (data, vec![cols, rows])
            }
        };
        let result = NdArray::new(data, shape, arr.dtype());
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    }
}

/// `numpy.fill_diagonal(a, val)` — fill an ndarray diagonal in place.
fn call_fill_diagonal(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.fill_diagonal", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.fill_diagonal", 2, 0))?;
    defer_drop!(arr_val, vm);
    let fill_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.fill_diagonal", 2, 1))?;
    defer_drop!(fill_val, vm);
    let wrap = if let Some(wrap_val) = pos.next() {
        defer_drop!(wrap_val, vm);
        bool_scalar_from_value(wrap_val, "numpy.fill_diagonal", "wrap")?
    } else {
        false
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.fill_diagonal", 3, 4));
    }

    let arr_id = mutable_ndarray_id(arr_val, "numpy.fill_diagonal", vm)?;
    let values = mutation_values_from_value(fill_val, "numpy.fill_diagonal", vm)?;
    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    let arr = arr_read.get_mut(vm.heap);
    let indices = fill_diagonal_flat_indices(arr.shape(), wrap)?;
    assign_cycled_values(&mut arr.data, &indices, &values)?;
    drop(arr_read);
    Ok(Value::None)
}

/// `numpy.put(a, ind, v)` — assign flattened positions in place.
fn call_put(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.put", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put", 3, 0))?;
    defer_drop!(arr_val, vm);
    let indices_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put", 3, 1))?;
    defer_drop!(indices_val, vm);
    let values_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put", 3, 2))?;
    defer_drop!(values_val, vm);
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.put", 3, 4));
    }

    let arr_id = mutable_ndarray_id(arr_val, "numpy.put", vm)?;
    let len = ndarray_len_by_id(arr_id, "numpy.put", vm)?;
    let indices = flat_indices_from_value(indices_val, len, "numpy.put", vm)?;
    let values = mutation_values_from_value(values_val, "numpy.put", vm)?;
    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    assign_cycled_values(&mut arr_read.get_mut(vm.heap).data, &indices, &values)?;
    drop(arr_read);
    Ok(Value::None)
}

/// `numpy.put_along_axis(a, indices, values, axis)` — assign values along an axis in place.
fn call_put_along_axis(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, vm);
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put_along_axis", 4, 0))?;
    defer_drop!(arr_val, vm);
    let indices_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put_along_axis", 4, 1))?;
    defer_drop!(indices_val, vm);
    let values_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.put_along_axis", 4, 2))?;
    defer_drop!(values_val, vm);
    let axis_value = pos.next();
    defer_drop_mut!(axis_value, vm);
    if pos.len() != 0 {
        return Err(ExcType::type_error_at_most("numpy.put_along_axis", 4, 4 + pos.len()));
    }
    parse_axis_keyword(kwargs_iter, axis_value, "numpy.put_along_axis", vm)?;
    let Some(axis_value) = axis_value.as_ref() else {
        return Err(ExcType::type_error_at_least("numpy.put_along_axis", 4, 3));
    };

    let arr_id = mutable_ndarray_id(arr_val, "numpy.put_along_axis", vm)?;
    let indices = ndarray_from_value(indices_val, "numpy.put_along_axis", vm)?;
    let values = mutation_values_from_value(values_val, "numpy.put_along_axis", vm)?;
    validate_broadcast_values(&values, indices.len(), "numpy.put_along_axis")?;
    let targets = {
        let HeapData::NdArray(arr) = vm.heap.get(arr_id) else {
            unreachable!()
        };
        let axis = normalize_axis(
            value_to_i64_arg(axis_value, "numpy.put_along_axis", "axis")?,
            arr.ndim(),
            "numpy.put_along_axis",
        )?;
        along_axis_flat_indices(arr.shape(), &indices, axis, "numpy.put_along_axis")?
    };
    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    let arr = arr_read.get_mut(vm.heap);
    for (index, target) in targets.into_iter().enumerate() {
        arr.data[target] = broadcast_value_at(&values, index);
    }
    drop(arr_read);
    Ok(Value::None)
}

/// `numpy.copyto(dst, src, where=True)` — copy values into an ndarray in place.
fn call_copyto(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, vm);
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);

    let dst_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.copyto", 2, 0))?;
    defer_drop!(dst_val, vm);
    let src_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.copyto", 2, 1))?;
    defer_drop!(src_val, vm);
    if pos.len() != 0 {
        return Err(ExcType::type_error_at_most("numpy.copyto", 2, 3));
    }

    let where_value = None;
    defer_drop_mut!(where_value, vm);
    for (key, value) in kwargs_iter {
        defer_drop!(key, vm);
        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            value.drop_with_heap(vm);
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        if let Some(StaticStrings::NpWhere) = keyword_name.static_string() {
            if where_value.is_some() {
                value.drop_with_heap(vm);
                return Err(ExcType::type_error_duplicate_arg(
                    "copyto",
                    keyword_name.as_str(vm.interns),
                ));
            }
            *where_value = Some(value);
        } else {
            value.drop_with_heap(vm);
            return Err(ExcType::type_error_unexpected_keyword(
                "copyto",
                keyword_name.as_str(vm.interns),
            ));
        }
    }

    let arr_id = mutable_ndarray_id(dst_val, "numpy.copyto", vm)?;
    let len = ndarray_len_by_id(arr_id, "numpy.copyto", vm)?;
    let source = mutation_values_from_value(src_val, "numpy.copyto", vm)?;
    validate_broadcast_values(&source, len, "numpy.copyto")?;
    let where_mask = if let Some(value) = where_value.as_ref() {
        Some(bool_mask_from_value(value, len, "numpy.copyto", true, vm)?)
    } else {
        None
    };

    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    let arr = arr_read.get_mut(vm.heap);
    for (index, slot) in arr.data.iter_mut().enumerate() {
        if where_mask.as_ref().is_none_or(|mask| mask[index]) {
            *slot = broadcast_value_at(&source, index);
        }
    }
    drop(arr_read);
    Ok(Value::None)
}

/// `numpy.putmask(a, mask, values)` — assign by flat mask positions in place.
fn call_putmask(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_id, mask, values) = masked_mutation_args(args, "numpy.putmask", false, vm)?;
    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    let arr = arr_read.get_mut(vm.heap);
    for (index, slot) in arr.data.iter_mut().enumerate() {
        if mask[index] {
            *slot = values[index % values.len()];
        }
    }
    drop(arr_read);
    Ok(Value::None)
}

/// `numpy.place(a, mask, values)` — place values sequentially where a mask is true.
fn call_place(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_id, mask, values) = masked_mutation_args(args, "numpy.place", false, vm)?;
    let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(arr_id) else {
        unreachable!()
    };
    let arr = arr_read.get_mut(vm.heap);
    let mut value_index = 0usize;
    for (index, slot) in arr.data.iter_mut().enumerate() {
        if mask[index] {
            *slot = values[value_index % values.len()];
            value_index += 1;
        }
    }
    drop(arr_read);
    Ok(Value::None)
}

/// Parses the shared `(a, mask, values)` arguments for masked mutation helpers.
fn masked_mutation_args(
    args: ArgValues,
    name: &str,
    allow_scalar_mask: bool,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(HeapId, Vec<bool>, Vec<f64>)> {
    let pos = args.into_pos_only(name, vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos.next().ok_or_else(|| ExcType::type_error_at_least(name, 3, 0))?;
    defer_drop!(arr_val, vm);
    let mask_val = pos.next().ok_or_else(|| ExcType::type_error_at_least(name, 3, 1))?;
    defer_drop!(mask_val, vm);
    let values_val = pos.next().ok_or_else(|| ExcType::type_error_at_least(name, 3, 2))?;
    defer_drop!(values_val, vm);
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most(name, 3, 4));
    }

    let arr_id = mutable_ndarray_id(arr_val, name, vm)?;
    let len = ndarray_len_by_id(arr_id, name, vm)?;
    let mask = bool_mask_from_value(mask_val, len, name, allow_scalar_mask, vm)?;
    let values = mutation_values_from_value(values_val, name, vm)?;
    ensure_nonempty_values(&values, name)?;
    Ok((arr_id, mask, values))
}

/// Returns a mutable ndarray heap id after validating that the target is an ndarray.
fn mutable_ndarray_id(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<HeapId> {
    match value {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::NdArray(_)) => Ok(*id),
        _ => Err(ExcType::type_error(format!("{name}() target must be an ndarray"))),
    }
}

/// Returns the flat length for a validated ndarray heap id.
fn ndarray_len_by_id(id: HeapId, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<usize> {
    match vm.heap.get(id) {
        HeapData::NdArray(arr) => Ok(arr.len()),
        _ => Err(ExcType::type_error(format!("{name}() target must be an ndarray"))),
    }
}

/// Converts one scalar or array-like mutation value into flat f64 storage.
fn mutation_values_from_value(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Vec<f64>> {
    if let Ok((scalar, _)) = numeric_scalar_info(value, name, vm) {
        Ok(vec![scalar])
    } else {
        let arr = ndarray_from_value(value, name, vm)?;
        ensure_nonempty_values(arr.data(), name)?;
        Ok(arr.data().to_vec())
    }
}

/// Rejects empty mutation value arrays, which cannot supply cycled assignments.
fn ensure_nonempty_values(values: &[f64], name: &str) -> RunResult<()> {
    if values.is_empty() {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("{name}() values must not be empty")).into())
    } else {
        Ok(())
    }
}

/// Converts an integer or array-like value into resolved flattened indices.
fn flat_indices_from_value(
    value: &Value,
    len: usize,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    if let Value::Int(index) = value {
        Ok(vec![resolve_flat_index(*index, len)?])
    } else {
        let indices = ndarray_from_value(value, name, vm)?;
        indices
            .data()
            .iter()
            .map(|&index| {
                #[expect(clippy::cast_possible_truncation, reason = "index from numeric ndarray")]
                {
                    resolve_flat_index(index as i64, len)
                }
            })
            .collect()
    }
}

/// Converts a bool-like scalar or array-like value into a flat mask.
fn bool_mask_from_value(
    value: &Value,
    len: usize,
    name: &str,
    allow_scalar: bool,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<bool>> {
    match value {
        Value::Bool(value) if allow_scalar => Ok(vec![*value; len]),
        Value::Int(value) if allow_scalar => Ok(vec![*value != 0; len]),
        Value::Float(value) if allow_scalar => Ok(vec![*value != 0.0; len]),
        _ => {
            let mask = ndarray_from_value(value, name, vm)?;
            if mask.len() == len {
                Ok(mask.data().iter().map(|&value| value != 0.0).collect())
            } else {
                Err(
                    SimpleException::new_msg(ExcType::ValueError, format!("{name}() mask must match array size"))
                        .into(),
                )
            }
        }
    }
}

/// Converts a bool-like scalar argument.
fn bool_scalar_from_value(value: &Value, name: &str, arg_name: &str) -> RunResult<bool> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Int(value) => Ok(*value != 0),
        Value::Float(value) => Ok(*value != 0.0),
        _ => Err(ExcType::type_error(format!("{name}() {arg_name} must be a boolean"))),
    }
}

/// Computes the flat row-major offsets that participate in `fill_diagonal`.
fn fill_diagonal_flat_indices(shape: &[usize], wrap: bool) -> RunResult<Vec<usize>> {
    if shape.len() < 2 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "array must be at least 2-d").into());
    }
    if shape.len() == 2 {
        let rows = shape[0];
        let cols = shape[1];
        if wrap {
            let total = rows.saturating_mul(cols);
            let step = cols.saturating_add(1);
            Ok((0..total).step_by(step.max(1)).collect())
        } else {
            Ok((0..rows.min(cols)).map(|index| index * cols + index).collect())
        }
    } else if shape.iter().all(|&dim| dim == shape[0]) {
        let diagonal_stride = row_major_strides(shape).iter().sum::<usize>();
        Ok((0..shape[0]).map(|index| index * diagonal_stride).collect())
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "All dimensions of input must be of equal length").into())
    }
}

/// Assigns cycled values into pre-resolved flat positions.
fn assign_cycled_values(target: &mut [f64], indices: &[usize], values: &[f64]) -> RunResult<()> {
    ensure_nonempty_values(values, "numpy assignment")?;
    for (value_index, &target_index) in indices.iter().enumerate() {
        target[target_index] = values[value_index % values.len()];
    }
    Ok(())
}

/// Validates source values for `copyto` scalar or equal-size broadcasting.
fn validate_broadcast_values(values: &[f64], len: usize, name: &str) -> RunResult<()> {
    ensure_nonempty_values(values, name)?;
    if values.len() == 1 || values.len() == len {
        Ok(())
    } else {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{name}() source must be scalar or same size"),
        )
        .into())
    }
}

/// Returns the value for a scalar-broadcast or same-size source buffer.
fn broadcast_value_at(values: &[f64], index: usize) -> f64 {
    values[if values.len() == 1 { 0 } else { index }]
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

/// `numpy.dstack(arrays)` — stack arrays along the third axis.
fn call_dstack(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.dstack", vm.heap)?;
    defer_drop!(arg, vm);
    let items = sequence_items(arg, "numpy.dstack", vm)?;
    defer_drop_mut!(items, vm);
    if items.is_empty() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "need at least one array to stack").into());
    }

    let mut arrays = Vec::with_capacity(items.len());
    for item in items.iter() {
        let arr = ndarray_from_value(item, "numpy.dstack", vm)?;
        arrays.push(dstack_promoted_array(arr));
    }
    let result = concatenate_ndarrays_along_axis(&arrays, 2, "numpy.dstack", vm.heap.tracker())?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.unstack(a, axis=0)` — split an array into a tuple with one axis removed.
fn call_unstack(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.unstack", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arr_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.unstack", 1, 0))?;
    defer_drop!(arr_val, vm);
    let axis = if let Some(axis_val) = pos.next() {
        defer_drop!(axis_val, vm);
        value_to_i64_arg(axis_val, "numpy.unstack", "axis")?
    } else {
        0
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.unstack", 2, 3));
    }

    let arr = ndarray_from_value(arr_val, "numpy.unstack", vm)?;
    let axis = normalize_axis(axis, arr.ndim(), "numpy.unstack")?;
    let result_shape = shape_without_axis(arr.shape(), axis);
    let mut values: SmallVec<[Value; 3]> = SmallVec::new();
    for index in 0..arr.shape()[axis] {
        let data = slice_ndarray_along_axis(&arr, axis, index, index + 1);
        if result_shape.is_empty() {
            values.push(scalar_from_f64(data[0], arr.dtype()));
        } else {
            let result = NdArray::new(data, result_shape.clone(), arr.dtype());
            values.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
        }
    }
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// Reshapes one dstack input according to NumPy's `atleast_3d` promotion rules.
fn dstack_promoted_array(arr: NdArray) -> NdArray {
    let NdArray { data, shape, dtype } = arr;
    let shape = match shape.as_slice() {
        [len] => vec![1, *len, 1],
        [rows, cols] => vec![*rows, *cols, 1],
        _ => shape,
    };
    NdArray::new(data, shape, dtype)
}

/// Concatenates arrays along one axis, preserving row-major layout.
fn concatenate_ndarrays_along_axis(
    arrays: &[NdArray],
    axis: usize,
    name: &str,
    tracker: &impl ResourceTracker,
) -> RunResult<NdArray> {
    let first = arrays
        .first()
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("{name}() needs at least one array")))?;
    let mut output_shape = first.shape().to_vec();
    if axis >= output_shape.len() {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("bad axis for array with {} dimensions", first.ndim()),
        )
        .into());
    }

    output_shape[axis] = 0;
    let mut dtype = first.dtype();
    for arr in arrays {
        if arr.ndim() != first.ndim() || !same_shape_except_axis(arr.shape(), first.shape(), axis) {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("{name}() input arrays must have matching dimensions except along the concatenation axis"),
            )
            .into());
        }
        output_shape[axis] = output_shape[axis]
            .checked_add(arr.shape()[axis])
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("{name}() dimensions overflow")))?;
        dtype = promote_dtype(dtype, arr.dtype());
    }

    let output_len = checked_shape_product(&output_shape, name)?;
    check_array_alloc_size(output_len, tracker)?;
    let inner = shape_product(&first.shape()[axis + 1..]);
    let outer = shape_product(&first.shape()[..axis]);
    let mut data = Vec::with_capacity(output_len);
    for outer_index in 0..outer {
        for arr in arrays {
            let axis_len = arr.shape()[axis];
            let start = outer_index * axis_len * inner;
            let end = start + axis_len * inner;
            data.extend_from_slice(&arr.data()[start..end]);
        }
    }

    Ok(NdArray::new(data, output_shape, dtype))
}

/// Returns true when two shapes are equal outside one concatenation axis.
fn same_shape_except_axis(lhs: &[usize], rhs: &[usize], axis: usize) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .enumerate()
            .all(|(index, (&left, &right))| index == axis || left == right)
}

/// Removes one axis from a shape, preserving the order of the remaining axes.
fn shape_without_axis(shape: &[usize], axis: usize) -> Vec<usize> {
    shape
        .iter()
        .enumerate()
        .filter_map(|(index, &dim)| (index != axis).then_some(dim))
        .collect()
}

/// Copies one half-open slice along an axis out of a row-major ndarray.
fn slice_ndarray_along_axis(arr: &NdArray, axis: usize, start_axis: usize, end_axis: usize) -> Vec<f64> {
    let axis_len = arr.shape()[axis];
    let inner = shape_product(&arr.shape()[axis + 1..]);
    let outer = shape_product(&arr.shape()[..axis]);
    let chunk_axis_len = end_axis.saturating_sub(start_axis);
    let mut data = Vec::with_capacity(outer * chunk_axis_len * inner);
    for outer_index in 0..outer {
        let block_start = outer_index * axis_len * inner + start_axis * inner;
        let block_end = block_start + chunk_axis_len * inner;
        data.extend_from_slice(&arr.data()[block_start..block_end]);
    }
    data
}

/// Computes a small shape product for already-validated ndarray dimensions.
fn shape_product(shape: &[usize]) -> usize {
    shape.iter().product()
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

/// `numpy.angle(z, deg=False)` for Monty's real-valued numeric subset.
fn call_angle(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, deg_val) = args.get_one_two_args("numpy.angle", vm.heap)?;
    defer_drop!(arg, vm);
    let deg = if let Some(deg_val) = deg_val {
        defer_drop!(deg_val, vm);
        value_to_bool_arg(deg_val, "numpy.angle", "deg")?
    } else {
        false
    };

    if let Ok((data, shape, _)) = extract_ndarray_info(arg, "numpy.angle", vm) {
        let data = data.into_iter().map(|value| real_phase_angle(value, deg)).collect();
        let arr = NdArray::new(data, shape, NdArrayDtype::Float64);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let (value, _) = numeric_scalar_info(arg, "numpy.angle", vm)?;
        Ok(Value::Float(real_phase_angle(value, deg)))
    }
}

/// Computes the phase angle of a real number, preserving NumPy's `-0.0 -> pi` behavior.
fn real_phase_angle(value: f64, deg: bool) -> f64 {
    let angle = if value.is_sign_negative() { PI } else { 0.0 };
    if deg { angle.to_degrees() } else { angle }
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

/// `numpy.real_if_close(a, tol=100)` — identity for Monty's real-valued subset.
fn call_real_if_close(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, tol) = args.get_one_two_args("numpy.real_if_close", vm.heap)?;
    defer_drop!(arg, vm);
    if let Some(tol) = tol {
        tol.drop_with_heap(vm);
    }

    if let Ok((data, shape, dtype)) = extract_ndarray_info(arg, "numpy.real_if_close", vm) {
        let arr = NdArray::new(data, shape, dtype);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let (value, dtype) = numeric_scalar_info(arg, "numpy.real_if_close", vm)?;
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

/// `numpy.can_cast(from_, to)` — safe cast predicate for Monty's compact dtype set.
fn call_can_cast(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (from_val, to_val) = args.get_two_args("numpy.can_cast", vm.heap)?;
    defer_drop!(from_val, vm);
    defer_drop!(to_val, vm);
    let from = dtype_meta_from_dtype_value(from_val, "numpy.can_cast", vm)?;
    let to = dtype_meta_from_dtype_value(to_val, "numpy.can_cast", vm)?;
    Ok(Value::Bool(can_cast_dtype_meta(from, to)))
}

/// `numpy.promote_types(type1, type2)` — promoted dtype marker.
fn call_promote_types(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (first_val, second_val) = args.get_two_args("numpy.promote_types", vm.heap)?;
    defer_drop!(first_val, vm);
    defer_drop!(second_val, vm);
    let first = dtype_meta_from_dtype_value(first_val, "numpy.promote_types", vm)?;
    let second = dtype_meta_from_dtype_value(second_val, "numpy.promote_types", vm)?;
    Ok(dtype_meta_value(promote_dtype_meta(first, second)))
}

/// `numpy.result_type(*arrays_and_dtypes)` — result dtype marker for real numeric inputs.
fn call_result_type(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.result_type", vm.heap)?;
    defer_drop_mut!(pos, vm);
    if pos.len() == 0 {
        return Err(ExcType::type_error_at_least("numpy.result_type", 1, 0));
    }

    let mut result = CompactDtype::Bool;
    for arg in pos.by_ref() {
        defer_drop!(arg, vm);
        result = promote_dtype_meta(result, dtype_meta_from_value(arg, "numpy.result_type", vm)?);
    }
    Ok(dtype_meta_value(result))
}

/// `numpy.common_type(*arrays)` — common real dtype marker, with float64 as minimum.
fn call_common_type(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.common_type", vm.heap)?;
    defer_drop_mut!(pos, vm);
    if pos.len() == 0 {
        return Err(ExcType::type_error_at_least("numpy.common_type", 1, 0));
    }

    for arg in pos.by_ref() {
        defer_drop!(arg, vm);
        dtype_meta_from_value(arg, "numpy.common_type", vm)?;
    }
    Ok(dtype_meta_value(CompactDtype::Float64))
}

/// `numpy.min_scalar_type(a)` — smallest compatible marker in Monty's compact dtype set.
fn call_min_scalar_type(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.min_scalar_type", vm.heap)?;
    defer_drop!(arg, vm);
    Ok(dtype_meta_value(dtype_meta_from_value(
        arg,
        "numpy.min_scalar_type",
        vm,
    )?))
}

/// `numpy.mintypecode(typechars, typeset='GDFgdf', default='d')` — legacy dtype code helper.
fn call_mintypecode(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.mintypecode", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let typechars_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.mintypecode", 1, 0))?;
    defer_drop!(typechars_val, vm);
    let typeset = if let Some(typeset_val) = pos.next() {
        defer_drop!(typeset_val, vm);
        string_from_value(typeset_val, "numpy.mintypecode", vm)?
    } else {
        "GDFgdf".to_string()
    };
    let default = if let Some(default_val) = pos.next() {
        defer_drop!(default_val, vm);
        string_from_value(default_val, "numpy.mintypecode", vm)?
    } else {
        "d".to_string()
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.mintypecode", 3, 4));
    }

    let chars = typechars_from_value(typechars_val, "numpy.mintypecode", vm)?;
    let result = mintypecode_result(&chars, &typeset, &default);
    allocate_string(result.to_string(), vm.heap)
}

/// `numpy.typename(char)` — human-readable legacy dtype character name.
fn call_typename(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.typename", vm.heap)?;
    defer_drop!(arg, vm);
    let text = string_from_value(arg, "numpy.typename", vm)?;
    let name = match text.as_str() {
        "?" => "bool",
        "b" => "signed char",
        "B" => "unsigned char",
        "h" => "short",
        "H" => "unsigned short",
        "i" => "integer",
        "I" => "unsigned integer",
        "l" => "long integer",
        "L" => "unsigned long integer",
        "q" => "long integer",
        "Q" => "unsigned long integer",
        "e" => "half precision",
        "f" => "single precision",
        "d" => "double precision",
        "g" => "long precision",
        "F" => "complex single precision",
        "D" => "complex double precision",
        "G" => "complex long double precision",
        "c" => "character",
        _ => {
            return Err(SimpleException::new_msg(ExcType::KeyError, format!("'{}'", text.escape_debug())).into());
        }
    };
    allocate_string(name.to_string(), vm.heap)
}

/// `numpy.geterr()` — return Monty's fixed floating-point error policy.
fn call_geterr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("numpy.geterr", vm.heap)?;
    numpy_error_policy_dict(vm)
}

/// `numpy.seterr(...)` — accept error-policy options and return the previous policy.
fn call_seterr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.drop_with_heap(vm);
    numpy_error_policy_dict(vm)
}

/// `numpy.geterrcall()` — return the fixed absence of an error callback.
fn call_geterrcall(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("numpy.geterrcall", vm.heap)?;
    Ok(Value::None)
}

/// `numpy.seterrcall(callback)` — accept callback configuration as a no-op.
fn call_seterrcall(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let callback = args.get_one_arg("numpy.seterrcall", vm.heap)?;
    callback.drop_with_heap(vm);
    Ok(Value::None)
}

/// `numpy.errstate(...)` — lightweight placeholder for context-manager-style code.
fn call_errstate(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.drop_with_heap(vm);
    numpy_error_policy_dict(vm)
}

/// `numpy.get_printoptions()` — return Monty's fixed print-option snapshot.
fn call_get_printoptions(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("numpy.get_printoptions", vm.heap)?;
    numpy_print_options_dict(vm)
}

/// `numpy.set_printoptions(...)` — accept print options as a no-op.
fn call_set_printoptions(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> Value {
    args.drop_with_heap(vm);
    Value::None
}

/// `numpy.printoptions(...)` — lightweight placeholder for context-manager-style code.
fn call_printoptions(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.drop_with_heap(vm);
    numpy_print_options_dict(vm)
}

/// `numpy.getbufsize()` — return NumPy's legacy default buffer size.
fn call_getbufsize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("numpy.getbufsize", vm.heap)?;
    Ok(Value::Int(8192))
}

/// `numpy.setbufsize(size)` — accept a buffer size as a no-op and return the previous size.
fn call_setbufsize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.setbufsize", vm.heap)?;
    arg.drop_with_heap(vm);
    Ok(Value::Int(8192))
}

/// `numpy.show_runtime()` — no-op placeholder that avoids host runtime introspection.
fn call_show_runtime(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> Value {
    args.drop_with_heap(vm);
    Value::None
}

/// `numpy.test()` — no-op placeholder that avoids launching NumPy's external test suite.
fn call_test(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> Value {
    args.drop_with_heap(vm);
    Value::None
}

/// Builds the fixed floating-point error-policy dictionary.
fn numpy_error_policy_dict(vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    string_dict_from_pairs(
        &[
            ("divide", "warn"),
            ("over", "warn"),
            ("under", "ignore"),
            ("invalid", "warn"),
        ],
        vm,
    )
}

/// Builds the fixed print-options dictionary for Monty's ndarray representation.
fn numpy_print_options_dict(vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let pairs = vec![
        (allocate_string("edgeitems".to_string(), vm.heap)?, Value::Int(3)),
        (allocate_string("threshold".to_string(), vm.heap)?, Value::Int(1000)),
        (allocate_string("linewidth".to_string(), vm.heap)?, Value::Int(75)),
        (allocate_string("precision".to_string(), vm.heap)?, Value::Int(8)),
        (allocate_string("suppress".to_string(), vm.heap)?, Value::Bool(false)),
        (
            allocate_string("nanstr".to_string(), vm.heap)?,
            allocate_string("nan".to_string(), vm.heap)?,
        ),
        (
            allocate_string("infstr".to_string(), vm.heap)?,
            allocate_string("inf".to_string(), vm.heap)?,
        ),
        (
            allocate_string("sign".to_string(), vm.heap)?,
            allocate_string("-".to_string(), vm.heap)?,
        ),
        (
            allocate_string("floatmode".to_string(), vm.heap)?,
            allocate_string("maxprec".to_string(), vm.heap)?,
        ),
        (allocate_string("legacy".to_string(), vm.heap)?, Value::None),
    ];
    dict_from_pairs(pairs, vm)
}

/// Allocates a Python dict from string key/value pairs.
fn string_dict_from_pairs(pairs: &[(&str, &str)], vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let mut values = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        values.push((
            allocate_string((*key).to_string(), vm.heap)?,
            allocate_string((*value).to_string(), vm.heap)?,
        ));
    }
    dict_from_pairs(values, vm)
}

/// Allocates a Python dict from already-owned key/value pairs.
fn dict_from_pairs(pairs: Vec<(Value, Value)>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))?))
}

/// Compact dtype categories that fit Monty's bool/int/float ndarray storage model.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CompactDtype {
    /// Boolean arrays and scalar markers.
    Bool,
    /// Integer arrays and scalar markers.
    Int,
    /// Single-precision dtype marker accepted as a float storage alias.
    Float32,
    /// Double-precision dtype marker used by Monty's float arrays and scalars.
    Float64,
}

/// Parses a dtype argument such as `np.float64` or `'int64'`.
fn dtype_meta_from_dtype_value(
    value: &Value,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<CompactDtype> {
    let text = string_from_value(value, name, vm)?;
    dtype_meta_from_str(&text, name)
}

/// Infers the compact dtype for a dtype marker, scalar, ndarray, or list.
fn dtype_meta_from_value(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<CompactDtype> {
    if let Ok(text) = string_from_value(value, name, vm) {
        dtype_meta_from_str(&text, name)
    } else if let Ok((_, dtype)) = numeric_scalar_info(value, name, vm) {
        Ok(dtype_meta_from_ndarray_dtype(dtype))
    } else {
        let arr = ndarray_from_value(value, name, vm)?;
        Ok(dtype_meta_from_ndarray_dtype(arr.dtype()))
    }
}

/// Maps dtype text onto Monty's compact dtype categories.
fn dtype_meta_from_str(text: &str, name: &str) -> RunResult<CompactDtype> {
    match text {
        "bool" | "bool_" | "?" => Ok(CompactDtype::Bool),
        "int8" | "int16" | "int32" | "int64" | "int_" | "intc" | "intp" | "long" | "longlong" | "byte" | "short"
        | "uint8" | "uint16" | "uint32" | "uint64" | "uint" | "uintc" | "uintp" | "ubyte" | "ushort" | "ulong"
        | "ulonglong" | "i" | "l" | "q" | "b" | "h" | "B" | "H" | "I" | "L" | "Q" => Ok(CompactDtype::Int),
        "float16" | "float32" | "half" | "single" | "f" | "e" => Ok(CompactDtype::Float32),
        "float64" | "double" | "longdouble" | "float" | "d" | "g" => Ok(CompactDtype::Float64),
        _ => Err(ExcType::type_error(format!("{name}() unsupported dtype: {text}"))),
    }
}

/// Converts an ndarray dtype into a compact dtype category.
fn dtype_meta_from_ndarray_dtype(dtype: NdArrayDtype) -> CompactDtype {
    match dtype {
        NdArrayDtype::Bool => CompactDtype::Bool,
        NdArrayDtype::Int64 => CompactDtype::Int,
        NdArrayDtype::Float64 => CompactDtype::Float64,
    }
}

/// Returns an interned dtype marker for a compact dtype category.
fn dtype_meta_value(dtype: CompactDtype) -> Value {
    let marker = match dtype {
        CompactDtype::Bool => StaticStrings::NpBool_,
        CompactDtype::Int => StaticStrings::NpInt64,
        CompactDtype::Float32 => StaticStrings::NpFloat32,
        CompactDtype::Float64 => StaticStrings::NpFloat64,
    };
    Value::InternString(marker.into())
}

/// Promotes two compact dtype categories using NumPy's real numeric ordering.
fn promote_dtype_meta(first: CompactDtype, second: CompactDtype) -> CompactDtype {
    first.max(second)
}

/// Returns whether a cast is safe in the compact bool -> int -> float ordering.
fn can_cast_dtype_meta(from: CompactDtype, to: CompactDtype) -> bool {
    from <= to
}

/// Extracts an owned Python string from interned or heap string values.
fn string_from_value(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<String> {
    match value {
        Value::InternString(id) => Ok(vm.interns.get_str(*id).to_string()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(text) => Ok(text.as_str().to_string()),
            _ => Err(ExcType::type_error(format!("{name}() expected a string"))),
        },
        _ => Err(ExcType::type_error(format!("{name}() expected a string"))),
    }
}

/// Extracts legacy dtype character codes from a string or sequence of strings.
fn typechars_from_value(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Vec<char>> {
    if let Ok(text) = string_from_value(value, name, vm) {
        Ok(text.chars().collect())
    } else {
        let items = match value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::List(list) => list.as_slice(),
                HeapData::Tuple(tuple) => tuple.as_slice(),
                _ => return Err(ExcType::type_error(format!("{name}() expected a string or sequence"))),
            },
            _ => return Err(ExcType::type_error(format!("{name}() expected a string or sequence"))),
        };
        let mut chars = Vec::new();
        for item in items {
            let text = string_from_value(item, name, vm)?;
            chars.extend(text.chars());
        }
        Ok(chars)
    }
}

/// Chooses the minimal type code present in `typeset`, falling back to `default`.
fn mintypecode_result(chars: &[char], typeset: &str, default: &str) -> char {
    let priority = ['G', 'D', 'F', 'g', 'd', 'f'];
    priority
        .iter()
        .copied()
        .find(|code| chars.contains(code) && typeset.contains(*code))
        .or_else(|| default.chars().next())
        .unwrap_or('d')
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

/// Triangle side selected by `tril_indices*` and `triu_indices*`.
#[derive(Clone, Copy)]
enum TriangleKind {
    /// Include coordinates on and below the selected diagonal.
    Lower,
    /// Include coordinates on and above the selected diagonal.
    Upper,
}

/// Integer index input for ravel/unravel helpers.
///
/// NumPy returns scalar coordinates for scalar index inputs and arrays for
/// vector inputs. This enum carries the copied integer data plus the shape
/// needed to rebuild that same result form.
enum IndexInput {
    /// A single scalar index.
    Scalar(i64),
    /// A vector/array of indices and the shape to preserve for the output.
    Array { data: Vec<i64>, shape: Vec<usize> },
}

/// Shared implementation for `numpy.atleast_1d`, `numpy.atleast_2d`, and `numpy.atleast_3d`.
///
/// Each input is converted into Monty's numeric ndarray representation and then
/// reshaped by adding length-1 axes according to NumPy's common cases. Multiple
/// inputs return a tuple of arrays, matching NumPy's variadic API.
fn call_atleast_nd(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    min_ndim: usize,
    name: &str,
) -> RunResult<Value> {
    let pos = args.into_pos_only(name, vm.heap)?;
    defer_drop_mut!(pos, vm);

    let mut outputs: SmallVec<[Value; 3]> = SmallVec::new();
    for arg in pos.by_ref() {
        defer_drop!(arg, vm);
        outputs.push(atleast_nd_value(arg, min_ndim, name, vm)?);
    }

    if outputs.len() == 1 {
        Ok(outputs.pop().expect("one output exists"))
    } else {
        allocate_tuple(outputs, vm.heap).map_err(Into::into)
    }
}

/// Converts one value for the `atleast_*d` family.
fn atleast_nd_value(
    value: &Value,
    min_ndim: usize,
    name: &str,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let (data, shape, dtype) = if let Ok((data, shape, dtype)) = extract_ndarray_info(value, name, vm) {
        (data, shape, dtype)
    } else {
        let (scalar, dtype) = numeric_scalar_info(value, name, vm)?;
        (vec![scalar], Vec::new(), dtype)
    };
    let shape = atleast_shape(shape, min_ndim);
    let arr = NdArray::new(data, shape, dtype);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// Computes NumPy's shape expansion for the supported `atleast_*d` cases.
fn atleast_shape(shape: Vec<usize>, min_ndim: usize) -> Vec<usize> {
    match (min_ndim, shape.as_slice()) {
        (1, []) => vec![1],
        (1, _) => shape,
        (2, []) => vec![1, 1],
        (2, [n]) => vec![1, *n],
        (2, _) => shape,
        (3, []) => vec![1, 1, 1],
        (3, [n]) => vec![1, *n, 1],
        (3, [rows, cols]) => vec![*rows, *cols, 1],
        (3, _) => shape,
        _ => shape,
    }
}

/// `numpy.diag_indices(n, ndim=2)` — return repeated diagonal index arrays.
fn call_diag_indices(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (n_val, ndim_val) = args.get_one_two_args("numpy.diag_indices", vm.heap)?;
    defer_drop!(n_val, vm);
    let n = value_to_nonnegative_usize(n_val, "numpy.diag_indices", "n")?;
    let ndim = if let Some(ndim_val) = ndim_val {
        defer_drop!(ndim_val, vm);
        value_to_nonnegative_usize(ndim_val, "numpy.diag_indices", "ndim")?
    } else {
        2
    };
    diag_indices_tuple(n, ndim, vm)
}

/// `numpy.diag_indices_from(arr)` — diagonal index arrays for a square input.
fn call_diag_indices_from(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.diag_indices_from", vm.heap)?;
    defer_drop!(arg, vm);
    let (_, shape, _) = extract_ndarray_info(arg, "numpy.diag_indices_from", vm)?;
    if shape.len() < 2 {
        Err(SimpleException::new_msg(ExcType::ValueError, "input array must be at least 2-d").into())
    } else if !shape.iter().all(|&dim| dim == shape[0]) {
        Err(SimpleException::new_msg(ExcType::ValueError, "all dimensions of input must be of equal length").into())
    } else {
        diag_indices_tuple(shape[0], shape.len(), vm)
    }
}

/// Builds a tuple containing `ndim` copies of the diagonal range `0..n`.
fn diag_indices_tuple(n: usize, ndim: usize, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let data: Vec<f64> = (0..n).map(usize_to_f64).collect();
    let vectors = (0..ndim).map(|_| data.clone()).collect::<Vec<_>>();
    tuple_from_index_vectors(vm, vectors, &[n])
}

/// `numpy.tril_indices()` / `numpy.triu_indices()` over the supported integer arguments.
fn call_triangle_indices(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    kind: TriangleKind,
    name: &str,
) -> RunResult<Value> {
    let (n, k, m) = triangle_args(args, name, vm)?;
    triangle_indices_tuple(n, k, m, kind, vm)
}

/// `numpy.tril_indices_from()` / `numpy.triu_indices_from()` for 2-D arrays.
fn call_triangle_indices_from(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    kind: TriangleKind,
    name: &str,
) -> RunResult<Value> {
    let (arr_val, k_val) = args.get_one_two_args(name, vm.heap)?;
    defer_drop!(arr_val, vm);
    let (_, shape, _) = extract_ndarray_info(arr_val, name, vm)?;
    let k = if let Some(k_val) = k_val {
        defer_drop!(k_val, vm);
        value_to_i64_arg(k_val, name, "k")?
    } else {
        0
    };
    if shape.len() == 2 {
        triangle_indices_tuple(shape[0], k, shape[1], kind, vm)
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "input array must be 2-d").into())
    }
}

/// Parses `(n, k=0, m=None)` for triangle index helpers.
fn triangle_args(args: ArgValues, name: &str, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<(usize, i64, usize)> {
    let pos = args.into_pos_only(name, vm.heap)?;
    defer_drop_mut!(pos, vm);
    let n_val = pos.next().ok_or_else(|| ExcType::type_error_at_least(name, 1, 0))?;
    defer_drop!(n_val, vm);
    let n = value_to_nonnegative_usize(n_val, name, "n")?;
    let k = if let Some(k_val) = pos.next() {
        defer_drop!(k_val, vm);
        value_to_i64_arg(k_val, name, "k")?
    } else {
        0
    };
    let m = if let Some(m_val) = pos.next() {
        defer_drop!(m_val, vm);
        if matches!(m_val, Value::None) {
            n
        } else {
            value_to_nonnegative_usize(m_val, name, "m")?
        }
    } else {
        n
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most(name, 3, 4));
    }
    Ok((n, k, m))
}

/// Builds lower- or upper-triangle row and column index arrays.
fn triangle_indices_tuple(
    n: usize,
    k: i64,
    m: usize,
    kind: TriangleKind,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let mut rows = Vec::new();
    let mut cols = Vec::new();
    for row in 0..n {
        let row_i64 = usize_to_i64(row)?;
        for col in 0..m {
            let col_i64 = usize_to_i64(col)?;
            let include = match kind {
                TriangleKind::Lower => col_i64 <= row_i64.saturating_add(k),
                TriangleKind::Upper => col_i64 >= row_i64.saturating_add(k),
            };
            if include {
                rows.push(usize_to_f64(row));
                cols.push(usize_to_f64(col));
            }
        }
    }
    let len = cols.len();
    tuple_from_index_vectors(vm, vec![rows, cols], &[len])
}

/// `numpy.indices(dimensions)` — build dense integer coordinate grids.
fn call_indices(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (dims_val, dtype_val) = args.get_one_two_args("numpy.indices", vm.heap)?;
    defer_drop!(dims_val, vm);
    if let Some(dtype_val) = dtype_val {
        dtype_val.drop_with_heap(vm);
    }
    let dimensions = extract_shape_from_value(dims_val, "numpy.indices", vm)?;
    let ndim = dimensions.len();
    let total = checked_shape_product(&dimensions, "numpy.indices")?;
    check_array_alloc_size(total.saturating_mul(ndim), vm.heap.tracker())?;

    let mut data = Vec::with_capacity(total.saturating_mul(ndim));
    if total > 0 {
        for axis in 0..ndim {
            let stride = checked_shape_product(&dimensions[axis + 1..], "numpy.indices")?;
            for flat in 0..total {
                let coord = if dimensions[axis] == 0 {
                    0
                } else {
                    (flat / stride) % dimensions[axis]
                };
                data.push(usize_to_f64(coord));
            }
        }
    }
    let mut shape = Vec::with_capacity(ndim + 1);
    shape.push(ndim);
    shape.extend(dimensions);
    let arr = NdArray::new(data, shape, NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.unravel_index(indices, shape)` — convert flat indices to coordinates.
fn call_unravel_index(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (indices_val, shape_val) = args.get_two_args("numpy.unravel_index", vm.heap)?;
    defer_drop!(indices_val, vm);
    defer_drop!(shape_val, vm);
    let dimensions = extract_shape_from_value(shape_val, "numpy.unravel_index", vm)?;
    let index_input = index_input_info(indices_val, "numpy.unravel_index", vm)?;
    let total = checked_shape_product(&dimensions, "numpy.unravel_index")?;

    match index_input {
        IndexInput::Scalar(index) => {
            let coords = unravel_one_index(index, &dimensions, total, "numpy.unravel_index")?;
            let values: SmallVec<[Value; 3]> = coords.into_iter().map(Value::Int).collect();
            allocate_tuple(values, vm.heap).map_err(Into::into)
        }
        IndexInput::Array { data, shape } => {
            let mut vectors = vec![Vec::with_capacity(data.len()); dimensions.len()];
            for index in data {
                let coords = unravel_one_index(index, &dimensions, total, "numpy.unravel_index")?;
                for (axis, coord) in coords.into_iter().enumerate() {
                    vectors[axis].push(i64_to_f64(coord));
                }
            }
            tuple_from_index_vectors(vm, vectors, &shape)
        }
    }
}

/// `numpy.ravel_multi_index(multi_index, dims)` — convert coordinates to flat indices.
fn call_ravel_multi_index(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (multi_val, dims_val) = args.get_two_args("numpy.ravel_multi_index", vm.heap)?;
    defer_drop!(multi_val, vm);
    defer_drop!(dims_val, vm);

    let dimensions = extract_shape_from_value(dims_val, "numpy.ravel_multi_index", vm)?;
    let coord_values = sequence_items(multi_val, "numpy.ravel_multi_index", vm)?;
    defer_drop!(coord_values, vm);
    if coord_values.len() != dimensions.len() {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "parameter multi_index must be a sequence of length matching dims",
        )
        .into());
    }

    let coords = coord_values
        .iter()
        .map(|value| index_input_info(value, "numpy.ravel_multi_index", vm))
        .collect::<RunResult<Vec<_>>>()?;
    ravel_multi_index_result(&coords, &dimensions, vm)
}

/// Computes the scalar or array output for `ravel_multi_index`.
fn ravel_multi_index_result(
    coords: &[IndexInput],
    dimensions: &[usize],
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let array_shape = shared_index_array_shape(coords)?;
    if let Some(shape) = array_shape {
        let len = shape.iter().product::<usize>();
        let mut data = Vec::with_capacity(len);
        for offset in 0..len {
            let mut coord_at_offset = Vec::with_capacity(coords.len());
            for coord in coords {
                coord_at_offset.push(index_input_value_at(coord, offset));
            }
            data.push(i64_to_f64(ravel_one_index(
                &coord_at_offset,
                dimensions,
                "numpy.ravel_multi_index",
            )?));
        }
        let arr = NdArray::new(data, shape, NdArrayDtype::Int64);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let coord_at_offset = coords
            .iter()
            .map(|coord| index_input_value_at(coord, 0))
            .collect::<Vec<_>>();
        Ok(Value::Int(ravel_one_index(
            &coord_at_offset,
            dimensions,
            "numpy.ravel_multi_index",
        )?))
    }
}

/// Extracts a scalar or integer array from an index-like value.
fn index_input_info(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<IndexInput> {
    if let Ok((data, shape)) = integer_array_info(value, name, vm) {
        Ok(IndexInput::Array {
            data: data.into_iter().map(f64_to_i64).collect(),
            shape,
        })
    } else {
        integer_scalar_info(value, name).map(IndexInput::Scalar)
    }
}

/// Returns the common array shape among index inputs, if any input is array-shaped.
fn shared_index_array_shape(coords: &[IndexInput]) -> RunResult<Option<Vec<usize>>> {
    let mut shape = None;
    for coord in coords {
        if let IndexInput::Array { shape: coord_shape, .. } = coord {
            if let Some(existing) = &shape {
                if existing != coord_shape {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "operands could not be broadcast together",
                    )
                    .into());
                }
            } else {
                shape = Some(coord_shape.clone());
            }
        }
    }
    Ok(shape)
}

/// Reads the scalar or per-offset array coordinate from an index input.
fn index_input_value_at(input: &IndexInput, offset: usize) -> i64 {
    match input {
        IndexInput::Scalar(value) => *value,
        IndexInput::Array { data, .. } => data[offset],
    }
}

/// Converts one flat index into row-major coordinates for `unravel_index`.
fn unravel_one_index(index: i64, dimensions: &[usize], total: usize, name: &str) -> RunResult<Vec<i64>> {
    let mut index = nonnegative_index_in_bounds(index, total, name)?;
    let mut coords = vec![0; dimensions.len()];
    for axis in (0..dimensions.len()).rev() {
        let dim = dimensions[axis];
        if dim == 0 {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "cannot unravel if shape has zero entries").into(),
            );
        }
        coords[axis] = usize_to_i64(index % dim)?;
        index /= dim;
    }
    Ok(coords)
}

/// Converts one coordinate tuple into a row-major flat index.
fn ravel_one_index(coords: &[i64], dimensions: &[usize], name: &str) -> RunResult<i64> {
    let mut flat = 0usize;
    for (&coord, &dim) in coords.iter().zip(dimensions.iter()) {
        let coord = nonnegative_index_in_bounds(coord, dim, name)?;
        flat = flat
            .checked_mul(dim)
            .and_then(|value| value.checked_add(coord))
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "index dimensions overflow"))?;
    }
    usize_to_i64(flat)
}

/// Checks an index is non-negative and inside a dimension or total-size bound.
fn nonnegative_index_in_bounds(index: i64, upper: usize, name: &str) -> RunResult<usize> {
    let index = i64_to_nonnegative_usize(index, name, "index")?;
    if index >= upper {
        Err(SimpleException::new_msg(ExcType::ValueError, "invalid entry in coordinates array").into())
    } else {
        Ok(index)
    }
}

/// Extracts list/tuple items from a value by cloning references safely.
fn sequence_items(value: &Value, name: &str, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Vec<Value>> {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => Ok(list.as_slice().iter().map(|value| value.clone_with_heap(vm)).collect()),
            HeapData::Tuple(tuple) => Ok(tuple.as_slice().iter().map(|value| value.clone_with_heap(vm)).collect()),
            _ => Err(ExcType::type_error(format!("{name}() requires a sequence argument"))),
        },
        _ => Err(ExcType::type_error(format!("{name}() requires a sequence argument"))),
    }
}

/// Allocates a tuple of integer ndarrays using a shared result shape.
fn tuple_from_index_vectors(
    vm: &mut VM<'_, impl ResourceTracker>,
    vectors: Vec<Vec<f64>>,
    shape: &[usize],
) -> RunResult<Value> {
    let mut values: SmallVec<[Value; 3]> = SmallVec::new();
    for data in vectors {
        let arr = NdArray::new(data, shape.to_vec(), NdArrayDtype::Int64);
        values.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?));
    }
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// Computes a shape product with a NumPy-style overflow error.
fn checked_shape_product(shape: &[usize], name: &str) -> RunResult<usize> {
    shape
        .iter()
        .try_fold(1usize, |acc, &dim| acc.checked_mul(dim))
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("{name}() dimensions overflow")).into())
}

/// Converts an integer `Value` into a non-negative usize argument.
fn value_to_nonnegative_usize(value: &Value, name: &str, arg_name: &str) -> RunResult<usize> {
    let value = value_to_i64_arg(value, name, arg_name)?;
    i64_to_nonnegative_usize(value, name, arg_name)
}

/// Converts an integer `Value` into an i64 argument.
fn value_to_i64_arg(value: &Value, name: &str, arg_name: &str) -> RunResult<i64> {
    match value {
        Value::Int(value) => Ok(*value),
        _ => Err(ExcType::type_error(format!("{name}() {arg_name} must be an integer"))),
    }
}

/// Converts a non-negative i64 into usize with a targeted ValueError.
fn i64_to_nonnegative_usize(value: i64, name: &str, arg_name: &str) -> RunResult<usize> {
    if value < 0 {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("{name}() {arg_name} must be non-negative")).into())
    } else {
        usize::try_from(value).map_err(|_| {
            SimpleException::new_msg(ExcType::ValueError, format!("{name}() {arg_name} is too large")).into()
        })
    }
}

/// Converts a usize index into i64 for Python integer outputs.
fn usize_to_i64(value: usize) -> RunResult<i64> {
    i64::try_from(value).map_err(|_| SimpleException::new_msg(ExcType::ValueError, "index is too large").into())
}

/// Converts a usize index into ndarray f64 backing storage.
#[expect(
    clippy::cast_precision_loss,
    reason = "integer ndarray values are stored as f64 in Monty's current ndarray model"
)]
fn usize_to_f64(value: usize) -> f64 {
    value as f64
}

/// Shared implementation for unary NumPy functions that return two results.
///
/// NumPy's `frexp()` and `modf()` preserve the input's scalar-vs-array form but
/// package the two outputs in a tuple. This helper keeps that shape handling in
/// one place so both scalar broadcasting and list-to-array conversion match the
/// rest of Monty's ufunc subset.
fn call_unary_tuple_func(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64) -> (f64, f64),
    name: &str,
    first_dtype: NdArrayDtype,
    second_dtype: NdArrayDtype,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, _)) = extract_ndarray_info(arg, name, vm) {
        let (first_data, second_data): (Vec<f64>, Vec<f64>) = data.iter().map(|&value| f(value)).unzip();
        tuple_from_arrays(vm, first_data, second_data, shape, first_dtype, second_dtype)
    } else {
        let (value, _) = numeric_scalar_info(arg, name, vm)?;
        let (first, second) = f(value);
        tuple_from_scalars(first, second, first_dtype, second_dtype, vm)
    }
}

/// `numpy.ldexp(x, exp)` over Monty's numeric scalar/list/ndarray subset.
///
/// The exponent operand is intentionally restricted to integer and boolean
/// dtypes, matching NumPy's ufunc loop selection and preventing accidental
/// coercion of arbitrary floats into powers-of-two exponents.
fn call_ldexp(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, exp_val) = args.get_two_args("numpy.ldexp", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(exp_val, vm);

    let x_info = extract_ndarray_info(x_val, "numpy.ldexp", vm);
    let exp_info = integer_array_info(exp_val, "numpy.ldexp", vm);

    match (x_info, exp_info) {
        (Ok((x_data, x_shape, _)), Ok((exp_data, exp_shape))) => {
            if x_shape != exp_shape {
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
                );
            }
            let data: Vec<f64> = x_data
                .iter()
                .zip(exp_data.iter())
                .map(|(&x, &exp)| numpy_ldexp(x, exp))
                .collect();
            let arr = NdArray::new(data, x_shape, NdArrayDtype::Float64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Ok((x_data, x_shape, _)), Err(_)) => {
            let exp = integer_scalar_info(exp_val, "numpy.ldexp")?;
            let data: Vec<f64> = x_data.iter().map(|&x| numpy_ldexp(x, i64_to_f64(exp))).collect();
            let arr = NdArray::new(data, x_shape, NdArrayDtype::Float64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Ok((exp_data, exp_shape))) => {
            let (x, _) = numeric_scalar_info(x_val, "numpy.ldexp", vm)?;
            let data: Vec<f64> = exp_data.iter().map(|&exp| numpy_ldexp(x, exp)).collect();
            let arr = NdArray::new(data, exp_shape, NdArrayDtype::Float64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Err(_)) => {
            let (x, _) = numeric_scalar_info(x_val, "numpy.ldexp", vm)?;
            let exp = integer_scalar_info(exp_val, "numpy.ldexp")?;
            Ok(Value::Float(numpy_ldexp(x, i64_to_f64(exp))))
        }
    }
}

/// Shared implementation for integer-only binary ufuncs like `gcd()` and `lcm()`.
///
/// Float dtypes are rejected instead of being truncated, because real NumPy has
/// no safe float loop for these ufuncs. Boolean inputs are accepted and promoted
/// to integer results.
fn call_integer_binop(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(i64, i64) -> i64,
    name: &str,
) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_info = integer_array_info(a_val, name, vm);
    let b_info = integer_array_info(b_val, name, vm);

    match (a_info, b_info) {
        (Ok((a_data, a_shape)), Ok((b_data, b_shape))) => {
            if a_shape != b_shape {
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
                );
            }
            let data: Vec<f64> = a_data
                .iter()
                .zip(b_data.iter())
                .map(|(&a, &b)| i64_to_f64(f(f64_to_i64(a), f64_to_i64(b))))
                .collect();
            let arr = NdArray::new(data, a_shape, NdArrayDtype::Int64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Ok((a_data, a_shape)), Err(_)) => {
            let scalar = integer_scalar_info(b_val, name)?;
            let data: Vec<f64> = a_data.iter().map(|&a| i64_to_f64(f(f64_to_i64(a), scalar))).collect();
            let arr = NdArray::new(data, a_shape, NdArrayDtype::Int64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Ok((b_data, b_shape))) => {
            let scalar = integer_scalar_info(a_val, name)?;
            let data: Vec<f64> = b_data.iter().map(|&b| i64_to_f64(f(scalar, f64_to_i64(b)))).collect();
            let arr = NdArray::new(data, b_shape, NdArrayDtype::Int64);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Err(_)) => {
            let a = integer_scalar_info(a_val, name)?;
            let b = integer_scalar_info(b_val, name)?;
            Ok(Value::Int(f(a, b)))
        }
    }
}

/// Integer/boolean bitwise binary operation exposed as a NumPy ufunc.
#[derive(Clone, Copy)]
enum IntegerBitwiseOp {
    /// Element-wise `a & b`.
    And,
    /// Element-wise `a | b`.
    Or,
    /// Element-wise `a ^ b`.
    Xor,
    /// Element-wise `a << b` using NumPy's fixed-width integer behavior.
    LeftShift,
    /// Element-wise `a >> b` using NumPy's fixed-width integer behavior.
    RightShift,
}

/// Shared implementation for NumPy's integer-only bitwise binary ufuncs.
///
/// Float inputs are rejected, scalar broadcasting is supported, and boolean
/// AND/OR/XOR preserves bool dtype when both operands are boolean-valued.
fn call_bitwise_binop(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    op: IntegerBitwiseOp,
    name: &str,
) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);

    let a_info = integer_array_info_with_dtype(a_val, name, vm);
    let b_info = integer_array_info_with_dtype(b_val, name, vm);

    match (a_info, b_info) {
        (Ok((a_data, a_shape, a_dtype)), Ok((b_data, b_shape, b_dtype))) => {
            if a_shape != b_shape {
                return Err(
                    SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
                );
            }
            let dtype = bitwise_binop_dtype(op, a_dtype, b_dtype);
            let data = a_data
                .iter()
                .zip(b_data.iter())
                .map(|(&a, &b)| i64_to_f64(apply_integer_bitwise_op(op, f64_to_i64(a), f64_to_i64(b))))
                .collect();
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Ok((a_data, a_shape, a_dtype)), Err(_)) => {
            let (scalar, scalar_dtype) = integer_scalar_info_with_dtype(b_val, name)?;
            let dtype = bitwise_binop_dtype(op, a_dtype, scalar_dtype);
            let data = a_data
                .iter()
                .map(|&a| i64_to_f64(apply_integer_bitwise_op(op, f64_to_i64(a), scalar)))
                .collect();
            let arr = NdArray::new(data, a_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Ok((b_data, b_shape, b_dtype))) => {
            let (scalar, scalar_dtype) = integer_scalar_info_with_dtype(a_val, name)?;
            let dtype = bitwise_binop_dtype(op, scalar_dtype, b_dtype);
            let data = b_data
                .iter()
                .map(|&b| i64_to_f64(apply_integer_bitwise_op(op, scalar, f64_to_i64(b))))
                .collect();
            let arr = NdArray::new(data, b_shape, dtype);
            Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
        }
        (Err(_), Err(_)) => {
            let (a, a_dtype) = integer_scalar_info_with_dtype(a_val, name)?;
            let (b, b_dtype) = integer_scalar_info_with_dtype(b_val, name)?;
            let dtype = bitwise_binop_dtype(op, a_dtype, b_dtype);
            Ok(scalar_from_integer_result(apply_integer_bitwise_op(op, a, b), dtype))
        }
    }
}

/// `numpy.bitwise_not()` / `numpy.invert()` over integer and boolean inputs.
fn call_bitwise_not(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.bitwise_not", vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, dtype)) = integer_array_info_with_dtype(arg, "numpy.bitwise_not", vm) {
        let result_dtype = bitwise_not_dtype(dtype);
        let data = data
            .iter()
            .map(|&value| i64_to_f64(bitwise_not_value(f64_to_i64(value), dtype)))
            .collect();
        let arr = NdArray::new(data, shape, result_dtype);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let (value, dtype) = integer_scalar_info_with_dtype(arg, "numpy.bitwise_not")?;
        let result_dtype = bitwise_not_dtype(dtype);
        Ok(scalar_from_integer_result(
            bitwise_not_value(value, dtype),
            result_dtype,
        ))
    }
}

/// `numpy.bitwise_count()` — population count of each integer's absolute value.
fn call_bitwise_count(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.bitwise_count", vm.heap)?;
    defer_drop!(arg, vm);

    if let Ok((data, shape, _)) = integer_array_info_with_dtype(arg, "numpy.bitwise_count", vm) {
        let data = data
            .iter()
            .map(|&value| i64_to_f64(numpy_bitwise_count(f64_to_i64(value))))
            .collect();
        let arr = NdArray::new(data, shape, NdArrayDtype::Int64);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        let value = integer_scalar_info(arg, "numpy.bitwise_count")?;
        Ok(Value::Int(numpy_bitwise_count(value)))
    }
}

/// `numpy.packbits()` — pack flattened non-zero integer values into big-endian bytes.
fn call_packbits(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.packbits", vm.heap)?;
    defer_drop!(arg, vm);

    let bits = if let Ok((data, _, _)) = integer_array_info_with_dtype(arg, "numpy.packbits", vm) {
        data.into_iter().map(|value| f64_to_i64(value) != 0).collect()
    } else {
        vec![integer_scalar_info(arg, "numpy.packbits")? != 0]
    };
    let output_len = bits.len().div_ceil(8);
    check_array_alloc_size(output_len, vm.heap.tracker())?;
    let data = pack_big_endian_bits(&bits);
    let arr = NdArray::new(data, vec![output_len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.unpackbits()` — unpack flattened byte-sized integer values into bits.
///
/// Monty does not currently model `uint8`, so this accepts integer arrays whose
/// values are in the byte range. That keeps `unpackbits(packbits(x))` useful
/// while still rejecting floats and out-of-range values.
fn call_unpackbits(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.unpackbits", vm.heap)?;
    defer_drop!(arg, vm);
    let (data, _, dtype) = integer_array_info_with_dtype(arg, "numpy.unpackbits", vm)?;
    if dtype == NdArrayDtype::Bool {
        return Err(unpackbits_type_error());
    }
    let output_len = data
        .len()
        .checked_mul(8)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "numpy.unpackbits() output is too large"))?;
    check_array_alloc_size(output_len, vm.heap.tracker())?;
    let mut bits = Vec::with_capacity(output_len);
    for value in data {
        let byte = byte_from_integer_slot(value)?;
        for bit in (0..8).rev() {
            bits.push(f64::from((byte >> bit) & 1));
        }
    }
    let arr = NdArray::new(bits, vec![output_len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// Determines the dtype for a bitwise binary ufunc.
fn bitwise_binop_dtype(op: IntegerBitwiseOp, a: NdArrayDtype, b: NdArrayDtype) -> NdArrayDtype {
    match op {
        IntegerBitwiseOp::And | IntegerBitwiseOp::Or | IntegerBitwiseOp::Xor
            if a == NdArrayDtype::Bool && b == NdArrayDtype::Bool =>
        {
            NdArrayDtype::Bool
        }
        IntegerBitwiseOp::And
        | IntegerBitwiseOp::Or
        | IntegerBitwiseOp::Xor
        | IntegerBitwiseOp::LeftShift
        | IntegerBitwiseOp::RightShift => NdArrayDtype::Int64,
    }
}

/// Applies a single integer bitwise operation with NumPy-style shift edges.
fn apply_integer_bitwise_op(op: IntegerBitwiseOp, a: i64, b: i64) -> i64 {
    match op {
        IntegerBitwiseOp::And => a & b,
        IntegerBitwiseOp::Or => a | b,
        IntegerBitwiseOp::Xor => a ^ b,
        IntegerBitwiseOp::LeftShift => numpy_left_shift(a, b),
        IntegerBitwiseOp::RightShift => numpy_right_shift(a, b),
    }
}

/// NumPy-style fixed-width left shift for signed 64-bit integer loops.
fn numpy_left_shift(value: i64, shift: i64) -> i64 {
    if (0..64).contains(&shift) {
        value.wrapping_shl(u32::try_from(shift).expect("shift count is in range"))
    } else {
        0
    }
}

/// NumPy-style fixed-width arithmetic right shift for signed 64-bit integer loops.
fn numpy_right_shift(value: i64, shift: i64) -> i64 {
    if (0..64).contains(&shift) {
        value >> u32::try_from(shift).expect("shift count is in range")
    } else if value < 0 {
        -1
    } else {
        0
    }
}

/// Computes the scalar/container dtype for bitwise inversion.
fn bitwise_not_dtype(dtype: NdArrayDtype) -> NdArrayDtype {
    if dtype == NdArrayDtype::Bool {
        NdArrayDtype::Bool
    } else {
        NdArrayDtype::Int64
    }
}

/// Applies unary bitwise inversion to a bool or int slot.
fn bitwise_not_value(value: i64, dtype: NdArrayDtype) -> i64 {
    if dtype == NdArrayDtype::Bool {
        i64::from(value == 0)
    } else {
        !value
    }
}

/// Converts an integer ufunc result back to a scalar value with bool preservation.
fn scalar_from_integer_result(value: i64, dtype: NdArrayDtype) -> Value {
    if dtype == NdArrayDtype::Bool {
        Value::Bool(value != 0)
    } else {
        Value::Int(value)
    }
}

/// Population count matching `numpy.bitwise_count`, which counts `abs(x)`.
fn numpy_bitwise_count(value: i64) -> i64 {
    i64::from(value.unsigned_abs().count_ones())
}

/// Packs a flattened bit stream into byte values using NumPy's default big bit order.
fn pack_big_endian_bits(bits: &[bool]) -> Vec<f64> {
    let mut packed = Vec::with_capacity(bits.len().div_ceil(8));
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (index, bit) in chunk.iter().enumerate() {
            if *bit {
                byte |= 1 << (7 - index);
            }
        }
        packed.push(f64::from(byte));
    }
    packed
}

/// Extracts one byte from Monty's integer ndarray storage for `unpackbits`.
fn byte_from_integer_slot(value: f64) -> RunResult<u8> {
    let value = f64_to_i64(value);
    u8::try_from(value).map_err(|_| unpackbits_type_error())
}

/// TypeError used when `unpackbits` input cannot represent unsigned bytes.
fn unpackbits_type_error() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "Expected an input array of unsigned byte data type").into()
}

/// Supported one-argument NumPy window generators.
#[derive(Clone, Copy)]
enum WindowKind {
    /// Bartlett triangular window.
    Bartlett,
    /// Blackman taper window.
    Blackman,
    /// Hamming raised-cosine window.
    Hamming,
    /// Hann raised-cosine window using NumPy's `hanning` spelling.
    Hanning,
}

/// Shared implementation for NumPy's simple floating-point window generators.
fn call_window(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    kind: WindowKind,
    name: &str,
) -> RunResult<Value> {
    let arg = args.get_one_arg(name, vm.heap)?;
    defer_drop!(arg, vm);
    let len = window_len(arg, name)?;
    check_array_alloc_size(len, vm.heap.tracker())?;
    let data = window_values(len, kind);
    let arr = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.kaiser(M, beta)` — Kaiser window using the supported real-valued subset.
fn call_kaiser(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (m_val, beta_val) = args.get_two_args("numpy.kaiser", vm.heap)?;
    defer_drop!(m_val, vm);
    defer_drop!(beta_val, vm);
    let len = window_len(m_val, "numpy.kaiser")?;
    let (beta, _) = numeric_scalar_info(beta_val, "numpy.kaiser", vm)?;
    check_array_alloc_size(len, vm.heap.tracker())?;
    let data = kaiser_values(len, beta);
    let arr = NdArray::new(data, vec![len], NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// Parses a NumPy window length, where non-positive lengths produce an empty array.
fn window_len(value: &Value, name: &str) -> RunResult<usize> {
    match value {
        Value::Int(m) if *m <= 0 => Ok(0),
        Value::Int(m) => usize::try_from(*m).map_err(|_| {
            SimpleException::new_msg(ExcType::ValueError, format!("{name}() window length is too large")).into()
        }),
        _ => Err(ExcType::type_error(format!(
            "{name}() window length must be an integer"
        ))),
    }
}

/// Generates values for one of NumPy's one-argument real windows.
fn window_values(len: usize, kind: WindowKind) -> Vec<f64> {
    match len {
        0 => Vec::new(),
        1 => vec![1.0],
        _ => {
            let denom = usize_to_f64(len - 1);
            (0..len)
                .map(|index| {
                    let n = usize_to_f64(index);
                    let phase = 2.0 * PI * n / denom;
                    match kind {
                        WindowKind::Bartlett => 1.0 - ((n - denom / 2.0) / (denom / 2.0)).abs(),
                        WindowKind::Blackman => 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos(),
                        WindowKind::Hamming => 0.54 - 0.46 * phase.cos(),
                        WindowKind::Hanning => 0.5 - 0.5 * phase.cos(),
                    }
                })
                .collect()
        }
    }
}

/// Generates a Kaiser window using the order-0 modified Bessel approximation.
fn kaiser_values(len: usize, beta: f64) -> Vec<f64> {
    match len {
        0 => Vec::new(),
        1 => vec![1.0],
        _ => {
            let alpha = usize_to_f64(len - 1) / 2.0;
            let denom = numpy_i0(beta);
            (0..len)
                .map(|index| {
                    let ratio = (usize_to_f64(index) - alpha) / alpha;
                    let inner = (1.0 - ratio * ratio).max(0.0).sqrt();
                    numpy_i0(beta * inner) / denom
                })
                .collect()
        }
    }
}

/// `numpy.base_repr(number, base=2, padding=0)` — convert an integer to a base string.
fn call_base_repr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.base_repr", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let number_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.base_repr", 1, 0))?;
    defer_drop!(number_val, vm);
    let number = value_to_i64_arg(number_val, "numpy.base_repr", "number")?;
    let base = if let Some(base_val) = pos.next() {
        defer_drop!(base_val, vm);
        value_to_i64_arg(base_val, "numpy.base_repr", "base")?
    } else {
        2
    };
    let padding = if let Some(padding_val) = pos.next() {
        defer_drop!(padding_val, vm);
        value_to_i64_arg(padding_val, "numpy.base_repr", "padding")?
    } else {
        0
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.base_repr", 3, 4));
    }
    let result = format_base_repr(number, base, padding)?;
    allocate_string(result, vm.heap)
}

/// `numpy.binary_repr(num, width=None)` — convert an integer to a binary string.
fn call_binary_repr(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (num_val, width_val) = args.get_one_two_args("numpy.binary_repr", vm.heap)?;
    defer_drop!(num_val, vm);
    let num = value_to_i64_arg(num_val, "numpy.binary_repr", "num")?;
    let width = if let Some(width_val) = width_val {
        defer_drop!(width_val, vm);
        if matches!(width_val, Value::None) {
            None
        } else {
            Some(value_to_i64_arg(width_val, "numpy.binary_repr", "width")?)
        }
    } else {
        None
    };
    let result = format_binary_repr(num, width)?;
    allocate_string(result, vm.heap)
}

/// Formats `base_repr`, including NumPy's base limits and padding behavior.
fn format_base_repr(number: i64, base: i64, padding: i64) -> RunResult<String> {
    let base = validate_base_repr_base(base)?;
    let padding = nonnegative_padding(padding)?;
    let magnitude = u128::from(number.unsigned_abs());
    let digits = format_unsigned_base(magnitude, base);
    let zero_count = if magnitude == 0 {
        padding.saturating_sub(1)
    } else {
        padding
    };
    let zeros = "0".repeat(zero_count);
    let sign = if number < 0 { "-" } else { "" };
    Ok(format!("{sign}{zeros}{digits}"))
}

/// Formats `binary_repr`, including two's-complement output when width is supplied.
fn format_binary_repr(num: i64, width: Option<i64>) -> RunResult<String> {
    let magnitude_digits = format_unsigned_base(u128::from(num.unsigned_abs()), 2);
    if let Some(width) = width {
        let width_usize = binary_width(width)?;
        let needed_width = if num < 0 {
            magnitude_digits.len().saturating_add(1)
        } else {
            magnitude_digits.len()
        };
        if width_usize < needed_width {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("Insufficient bit width={width} provided for binwidth={needed_width}"),
            )
            .into());
        }
        if num < 0 {
            let value = twos_complement_value(num, width_usize)?;
            Ok(left_pad_zeros(format_unsigned_base(value, 2), width_usize))
        } else {
            Ok(left_pad_zeros(magnitude_digits, width_usize))
        }
    } else if num < 0 {
        Ok(format!("-{magnitude_digits}"))
    } else {
        Ok(magnitude_digits)
    }
}

/// Validates the base accepted by `base_repr`.
fn validate_base_repr_base(base: i64) -> RunResult<u32> {
    if base < 2 {
        Err(SimpleException::new_msg(ExcType::ValueError, "Bases less than 2 not handled in base_repr.").into())
    } else if base > 36 {
        Err(SimpleException::new_msg(ExcType::ValueError, "Bases greater than 36 not handled in base_repr.").into())
    } else {
        u32::try_from(base).map_err(|_| SimpleException::new_msg(ExcType::ValueError, "invalid base").into())
    }
}

/// Converts NumPy's `base_repr` padding argument to a repeat count.
fn nonnegative_padding(padding: i64) -> RunResult<usize> {
    if padding <= 0 {
        Ok(0)
    } else {
        usize::try_from(padding)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "base_repr() padding is too large").into())
    }
}

/// Converts and validates a `binary_repr` width.
fn binary_width(width: i64) -> RunResult<usize> {
    if width < 0 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Insufficient bit width={width} provided for binwidth=1"),
        )
        .into())
    } else {
        usize::try_from(width)
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "binary_repr() width is too large").into())
    }
}

/// Computes a negative integer's two's-complement value for a requested width.
fn twos_complement_value(num: i64, width: usize) -> RunResult<u128> {
    if width > 127 {
        Err(SimpleException::new_msg(ExcType::ValueError, "binary_repr() width is too large").into())
    } else {
        let modulus = 1_u128
            .checked_shl(u32::try_from(width).expect("width is bounded"))
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "binary_repr() width is too large"))?;
        Ok(modulus - u128::from(num.unsigned_abs()))
    }
}

/// Left-pads a string with zeros up to `width`.
fn left_pad_zeros(mut value: String, width: usize) -> String {
    if value.len() < width {
        let mut padded = "0".repeat(width - value.len());
        padded.push_str(&value);
        value = padded;
    }
    value
}

/// Formats an unsigned integer in bases 2 through 36.
fn format_unsigned_base(mut value: u128, base: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if value == 0 {
        return "0".to_owned();
    }
    let base = u128::from(base);
    let mut out = Vec::new();
    while value > 0 {
        let digit = usize::try_from(value % base).expect("digit is less than base");
        out.push(char::from(DIGITS[digit]));
        value /= base;
    }
    out.iter().rev().collect()
}

/// Shared implementation for binary NumPy functions that return two results.
///
/// `numpy.divmod()` is the motivating case: each operand can be a scalar, list,
/// or ndarray, and the quotient and remainder outputs must preserve the same
/// broadcasted shape while being returned as a pair.
fn call_numeric_tuple_binop(
    vm: &mut VM<'_, impl ResourceTracker>,
    args: ArgValues,
    f: fn(f64, f64) -> (f64, f64),
    name: &str,
    first_result: BinopResult,
    second_result: BinopResult,
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
            let (first_data, second_data): (Vec<f64>, Vec<f64>) =
                a_data.iter().zip(b_data.iter()).map(|(&a, &b)| f(a, b)).unzip();
            tuple_from_arrays(
                vm,
                first_data,
                second_data,
                a_shape,
                binop_dtype(first_result, a_dtype, b_dtype),
                binop_dtype(second_result, a_dtype, b_dtype),
            )
        }
        (Ok((a_data, a_shape, a_dtype)), Err(_)) => {
            let (scalar, scalar_dtype) = numeric_scalar_info(b_val, name, vm)?;
            let (first_data, second_data): (Vec<f64>, Vec<f64>) = a_data.iter().map(|&a| f(a, scalar)).unzip();
            tuple_from_arrays(
                vm,
                first_data,
                second_data,
                a_shape,
                binop_dtype(first_result, a_dtype, scalar_dtype),
                binop_dtype(second_result, a_dtype, scalar_dtype),
            )
        }
        (Err(_), Ok((b_data, b_shape, b_dtype))) => {
            let (scalar, scalar_dtype) = numeric_scalar_info(a_val, name, vm)?;
            let (first_data, second_data): (Vec<f64>, Vec<f64>) = b_data.iter().map(|&b| f(scalar, b)).unzip();
            tuple_from_arrays(
                vm,
                first_data,
                second_data,
                b_shape,
                binop_dtype(first_result, scalar_dtype, b_dtype),
                binop_dtype(second_result, scalar_dtype, b_dtype),
            )
        }
        (Err(_), Err(_)) => {
            let (a, a_dtype) = numeric_scalar_info(a_val, name, vm)?;
            let (b, b_dtype) = numeric_scalar_info(b_val, name, vm)?;
            let (first, second) = f(a, b);
            tuple_from_scalars(
                first,
                second,
                binop_dtype(first_result, a_dtype, b_dtype),
                binop_dtype(second_result, a_dtype, b_dtype),
                vm,
            )
        }
    }
}

/// Allocates a tuple containing two ndarray outputs with a shared shape.
fn tuple_from_arrays(
    vm: &mut VM<'_, impl ResourceTracker>,
    first_data: Vec<f64>,
    second_data: Vec<f64>,
    shape: Vec<usize>,
    first_dtype: NdArrayDtype,
    second_dtype: NdArrayDtype,
) -> RunResult<Value> {
    let first_arr = NdArray::new(first_data, shape.clone(), first_dtype);
    let second_arr = NdArray::new(second_data, shape, second_dtype);
    let first = Value::Ref(vm.heap.allocate(HeapData::NdArray(first_arr))?);
    let second = Value::Ref(vm.heap.allocate(HeapData::NdArray(second_arr))?);
    Ok(allocate_tuple(smallvec::smallvec![first, second], vm.heap)?)
}

/// Allocates a tuple containing two scalar ufunc outputs.
fn tuple_from_scalars(
    first: f64,
    second: f64,
    first_dtype: NdArrayDtype,
    second_dtype: NdArrayDtype,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    Ok(allocate_tuple(
        smallvec::smallvec![
            scalar_from_f64(first, first_dtype),
            scalar_from_f64(second, second_dtype)
        ],
        vm.heap,
    )?)
}

/// Extracts an integer scalar accepted by NumPy's integer-only ufunc loops.
fn integer_scalar_info(value: &Value, name: &str) -> RunResult<i64> {
    integer_scalar_info_with_dtype(value, name).map(|(value, _)| value)
}

/// Extracts an integer scalar plus the dtype NumPy would infer for it.
fn integer_scalar_info_with_dtype(value: &Value, name: &str) -> RunResult<(i64, NdArrayDtype)> {
    match value {
        Value::Int(n) => Ok((*n, NdArrayDtype::Int64)),
        Value::Bool(b) => Ok((i64::from(*b), NdArrayDtype::Bool)),
        _ => Err(integer_ufunc_type_error(name)),
    }
}

/// Extracts integer ndarray data, accepting lists and rejecting float dtypes.
fn integer_array_info(
    value: &Value,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<(Vec<f64>, Vec<usize>)> {
    let (data, shape, _) = integer_array_info_with_dtype(value, name, vm)?;
    Ok((data, shape))
}

/// Extracts integer ndarray data and dtype, accepting lists and rejecting float dtypes.
fn integer_array_info_with_dtype(
    value: &Value,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<(Vec<f64>, Vec<usize>, NdArrayDtype)> {
    let (data, shape, dtype) = extract_ndarray_info(value, name, vm)?;
    if dtype == NdArrayDtype::Float64 {
        Err(integer_ufunc_type_error(name))
    } else {
        Ok((data, shape, dtype))
    }
}

/// Builds a compact TypeError for unsupported integer ufunc inputs.
fn integer_ufunc_type_error(name: &str) -> RunError {
    let ufunc = name.strip_prefix("numpy.").unwrap_or(name);
    SimpleException::new_msg(
        ExcType::TypeError,
        format!("ufunc '{ufunc}' not supported for the input types"),
    )
    .into()
}

/// Converts an integer-valued ndarray slot back to `i64`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "integer ndarray values are represented as f64 in Monty's current ndarray storage"
)]
fn f64_to_i64(value: f64) -> i64 {
    value as i64
}

/// Converts an `i64` integer result into ndarray backing storage.
#[expect(
    clippy::cast_precision_loss,
    reason = "integer ndarray values are stored as f64 in Monty's current ndarray model"
)]
fn i64_to_f64(value: i64) -> f64 {
    value as f64
}

/// Approximation for `numpy.i0()`, the modified Bessel function I0.
///
/// This uses the classic Cephes polynomial split, which is accurate enough for
/// NumPy-compatible window generation while avoiding a new special-functions
/// dependency in the sandbox runtime.
fn numpy_i0(value: f64) -> f64 {
    let x = value.abs();
    if x <= 3.75 {
        let y = (x / 3.75).powi(2);
        1.0 + y
            * (3.515_622_9
                + y * (3.089_942_4 + y * (1.206_749_2 + y * (0.265_973_2 + y * (0.036_076_8 + y * 0.004_581_3)))))
    } else {
        let y = 3.75 / x;
        (x.exp() / x.sqrt())
            * (0.398_942_28
                + y * (0.013_285_92
                    + y * (0.002_253_19
                        + y * (-0.001_575_65
                            + y * (0.009_162_81
                                + y * (-0.020_577_06 + y * (0.026_355_37 + y * (-0.016_476_33 + y * 0.003_923_77))))))))
    }
}

/// `numpy.frexp()` scalar kernel returning exponent as an integer-valued float.
fn numpy_frexp(value: f64) -> (f64, f64) {
    let (mantissa, exponent) = libm::frexp(value);
    (mantissa, f64::from(exponent))
}

/// `numpy.modf()` scalar kernel.
fn numpy_modf(value: f64) -> (f64, f64) {
    libm::modf(value)
}

/// `numpy.ldexp()` scalar kernel with NumPy-style non-raising overflow behavior.
fn numpy_ldexp(value: f64, exponent: f64) -> f64 {
    let exponent = f64_to_i64(exponent);
    let exponent = i32::try_from(exponent).unwrap_or(if exponent < 0 { i32::MIN } else { i32::MAX });
    libm::ldexp(value, exponent)
}

/// `numpy.gcd()` scalar kernel using NumPy's wrapping int64 edge behavior.
fn numpy_gcd(a: i64, b: i64) -> i64 {
    wrapping_u64_to_i64(gcd_u64(a.unsigned_abs(), b.unsigned_abs()))
}

/// `numpy.lcm()` scalar kernel using NumPy's wrapping int64 edge behavior.
fn numpy_lcm(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        0
    } else {
        let gcd = gcd_u64(a.unsigned_abs(), b.unsigned_abs());
        wrapping_u64_to_i64((a.unsigned_abs() / gcd).wrapping_mul(b.unsigned_abs()))
    }
}

/// Euclidean GCD for unsigned integer magnitudes.
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Reinterprets a NumPy int64 ufunc magnitude after two's-complement wrapping.
#[expect(
    clippy::cast_possible_wrap,
    reason = "NumPy int64 integer ufuncs wrap overflowing unsigned magnitudes into int64"
)]
fn wrapping_u64_to_i64(value: u64) -> i64 {
    value as i64
}

/// Stable scalar kernel for `numpy.logaddexp()`.
fn numpy_logaddexp(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        let max = a.max(b);
        if max.is_infinite() {
            max
        } else {
            max + ((a - max).exp() + (b - max).exp()).ln()
        }
    }
}

/// Stable scalar kernel for `numpy.logaddexp2()`.
fn numpy_logaddexp2(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        let max = a.max(b);
        if max.is_infinite() {
            max
        } else {
            max + ((a - max).exp2() + (b - max).exp2()).log2()
        }
    }
}

/// Scalar kernel for `numpy.spacing()`.
fn numpy_spacing(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        f64::NAN
    } else if value == 0.0 {
        f64::from_bits(1)
    } else {
        let direction = if value > 0.0 { f64::INFINITY } else { f64::NEG_INFINITY };
        libm::nextafter(value, direction) - value
    }
}

/// Scalar kernel for `numpy.signbit()` using the f64 backing representation.
fn signbit_as_f64(value: f64) -> f64 {
    bool_to_f64(value.is_sign_negative())
}

/// Scalar kernel for NumPy's normalized `sinc(x) = sin(pi*x)/(pi*x)`.
fn numpy_sinc(value: f64) -> f64 {
    if value == 0.0 {
        1.0
    } else {
        let scaled = PI * value;
        scaled.sin() / scaled
    }
}

/// Scalar kernel for `numpy.heaviside()`.
fn numpy_heaviside(value: f64, zero_value: f64) -> f64 {
    if value.is_nan() {
        f64::NAN
    } else if value < 0.0 {
        0.0
    } else if value == 0.0 {
        zero_value
    } else {
        1.0
    }
}

/// Scalar kernel for `numpy.divmod()`.
fn numpy_divmod(a: f64, b: f64) -> (f64, f64) {
    ((a / b).floor(), py_mod(a, b))
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

/// `numpy.diagflat(v, k=0)` — create a diagonal matrix from flattened input.
fn call_diagflat(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, k_val) = args.get_one_two_args("numpy.diagflat", vm.heap)?;
    defer_drop!(arg, vm);
    let k = if let Some(k_val) = k_val {
        defer_drop!(k_val, vm);
        value_to_i64_arg(k_val, "numpy.diagflat", "k")?
    } else {
        0
    };
    let arr = ndarray_from_value(arg, "numpy.diagflat", vm)?;
    let offset = diagflat_offset(k)?;
    let size = arr
        .data()
        .len()
        .checked_add(offset)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "numpy.diagflat() dimensions overflow"))?;
    let total = size
        .checked_mul(size)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "numpy.diagflat() dimensions overflow"))?;
    check_array_alloc_size(total, vm.heap.tracker())?;

    let mut data = vec![0.0; total];
    for (index, value) in arr.data().iter().enumerate() {
        let (row, col) = diagflat_position(index, offset, k);
        data[row * size + col] = *value;
    }
    let result = NdArray::new(data, vec![size, size], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Converts a diagonal offset into a positive matrix-size expansion.
fn diagflat_offset(k: i64) -> RunResult<usize> {
    usize::try_from(k.unsigned_abs())
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "numpy.diagflat() k is too large").into())
}

/// Computes a row/column pair for one flattened input item.
fn diagflat_position(index: usize, offset: usize, k: i64) -> (usize, usize) {
    if k >= 0 {
        (index, index + offset)
    } else {
        (index + offset, index)
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

/// Compatibility conversion for layout/order helpers that are no-ops in Monty's ndarray model.
fn call_asarray_compat(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.asarray", vm.heap)?;
    defer_drop_mut!(pos, vm);
    let arg = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.asarray", 1, 0))?;
    defer_drop!(arg, vm);
    for extra in pos.by_ref() {
        extra.drop_with_heap(vm);
    }
    let arr = ndarray_from_value(arg, "numpy.asarray", vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
}

/// `numpy.ix_(*args)` — construct open mesh index arrays from 1-D sequences.
fn call_ix(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.ix_", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let mut arrays = Vec::new();
    for arg in pos.by_ref() {
        defer_drop!(arg, vm);
        let arr = ndarray_from_value(arg, "numpy.ix_", vm)?;
        if arr.shape().len() != 1 {
            return Err(SimpleException::new_msg(ExcType::ValueError, "Cross index must be 1 dimensional").into());
        }
        arrays.push(arr);
    }

    let total_len = arrays
        .iter()
        .try_fold(0usize, |acc, arr| acc.checked_add(arr.data().len()))
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "numpy.ix_() dimensions overflow"))?;
    check_array_alloc_size(total_len, vm.heap.tracker())?;

    let ndim = arrays.len();
    let mut values: SmallVec<[Value; 3]> = SmallVec::new();
    for (axis, arr) in arrays.iter().enumerate() {
        let shape = ix_output_shape(axis, arr.data().len(), ndim);
        let result = NdArray::new(arr.data().to_vec(), shape, arr.dtype());
        values.push(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    }
    allocate_tuple(values, vm.heap).map_err(Into::into)
}

/// Computes the broadcastable shape for one `ix_` output array.
fn ix_output_shape(axis: usize, len: usize, ndim: usize) -> Vec<usize> {
    let mut shape = vec![1; ndim];
    if let Some(dim) = shape.get_mut(axis) {
        *dim = len;
    }
    shape
}

/// `numpy.mask_indices(n, mask_func, k=0)` — indices selected by a triangular mask.
fn call_mask_indices(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.mask_indices", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let n_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.mask_indices", 2, 0))?;
    defer_drop!(n_val, vm);
    let n = value_to_nonnegative_usize(n_val, "numpy.mask_indices", "n")?;

    let mask_func = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.mask_indices", 2, 1))?;
    defer_drop!(mask_func, vm);
    let kind = triangle_kind_from_mask_func(mask_func)?;

    let k = if let Some(k_val) = pos.next() {
        defer_drop!(k_val, vm);
        value_to_i64_arg(k_val, "numpy.mask_indices", "k")?
    } else {
        0
    };
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.mask_indices", 3, 4));
    }

    triangle_indices_tuple(n, k, n, kind, vm)
}

/// Extracts the supported triangular mask function for `mask_indices()`.
fn triangle_kind_from_mask_func(value: &Value) -> RunResult<TriangleKind> {
    match value {
        Value::ModuleFunction(ModuleFunctions::Numpy(NumpyFunctions::Triu)) => Ok(TriangleKind::Upper),
        Value::ModuleFunction(ModuleFunctions::Numpy(NumpyFunctions::Tril)) => Ok(TriangleKind::Lower),
        _ => Err(ExcType::type_error(
            "numpy.mask_indices() only supports numpy.triu or numpy.tril mask functions",
        )),
    }
}

/// `numpy.isfortran(a)` — Monty arrays are currently stored only in row-major order.
fn call_isfortran(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.isfortran", vm.heap)?;
    arg.drop_with_heap(vm);
    Ok(Value::Bool(false))
}

/// `numpy.shares_memory()` / `numpy.may_share_memory()` for Monty's copy-based ndarray model.
fn call_memory_overlap(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues, name: &str) -> RunResult<Value> {
    let (a, b) = args.get_two_args(name, vm.heap)?;
    defer_drop!(a, vm);
    defer_drop!(b, vm);
    Ok(Value::Bool(same_ndarray_ref(a, b, vm)))
}

/// Checks whether both values are the exact same ndarray heap object.
fn same_ndarray_ref(a: &Value, b: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match (a, b) {
        (Value::Ref(a_id), Value::Ref(b_id)) if a_id == b_id => matches!(vm.heap.get(*a_id), HeapData::NdArray(_)),
        _ => false,
    }
}

/// `numpy.asarray_chkfinite(a)` — convert to array and reject NaN or infinity.
fn call_asarray_chkfinite(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.asarray_chkfinite", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.asarray_chkfinite", vm)?;
    if arr.data().iter().all(|value| value.is_finite()) {
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(arr))?))
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "array must not contain infs or NaNs").into())
    }
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

/// `numpy.dsplit(a, indices_or_sections)` — split arrays along depth axis.
fn call_dsplit(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arr_val, idx_val) = args.get_two_args("numpy.dsplit", vm.heap)?;
    defer_drop!(arr_val, vm);
    defer_drop!(idx_val, vm);
    let arr = ndarray_from_value(arr_val, "numpy.dsplit", vm)?;
    if arr.ndim() < 3 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "dsplit only works on arrays of 3 or more dimensions",
        )
        .into());
    }

    let split_indices = split_indices_for_axis(idx_val, arr.shape()[2], "numpy.dsplit", vm)?;
    split_ndarray_along_axis_to_list(&arr, 2, &split_indices, vm)
}

/// Extracts split points for a fixed array axis from an integer or index sequence.
fn split_indices_for_axis(
    value: &Value,
    axis_len: usize,
    name: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<Vec<usize>> {
    match value {
        Value::Int(sections) => equal_split_indices(*sections, axis_len),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::List(list) => list
                .as_slice()
                .iter()
                .map(|value| split_index_value_to_usize(value, axis_len, name))
                .collect(),
            HeapData::Tuple(tuple) => tuple
                .as_slice()
                .iter()
                .map(|value| split_index_value_to_usize(value, axis_len, name))
                .collect(),
            HeapData::NdArray(indices) => indices
                .data()
                .iter()
                .map(|&value| split_index_f64_to_usize(value, axis_len, name))
                .collect(),
            _ => Err(ExcType::type_error(format!("{name}() second arg must be int or list"))),
        },
        _ => Err(ExcType::type_error(format!("{name}() second arg must be int or list"))),
    }
}

/// Computes split points for equal-sized axis sections.
fn equal_split_indices(sections: i64, axis_len: usize) -> RunResult<Vec<usize>> {
    if sections <= 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "number sections must be larger than 0").into());
    }
    let sections = usize::try_from(sections)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "number sections is too large"))?;
    if !axis_len.is_multiple_of(sections) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "array split does not result in an equal division").into(),
        );
    }
    let chunk_size = axis_len / sections;
    Ok((1..sections).map(|index| index * chunk_size).collect())
}

/// Converts one Python split index to a clamped axis offset.
fn split_index_value_to_usize(value: &Value, axis_len: usize, name: &str) -> RunResult<usize> {
    match value {
        Value::Int(index) => split_index_i64_to_usize(*index, axis_len, name),
        _ => Err(ExcType::type_error("split indices must be integers")),
    }
}

/// Converts one ndarray-backed split index to a clamped axis offset.
#[expect(
    clippy::cast_possible_truncation,
    reason = "integer ndarray values are stored as f64 in Monty's current ndarray model"
)]
fn split_index_f64_to_usize(value: f64, axis_len: usize, name: &str) -> RunResult<usize> {
    if value.is_finite() {
        split_index_i64_to_usize(value as i64, axis_len, name)
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("{name}() split index must be finite")).into())
    }
}

/// Converts one signed split index to a NumPy-style clamped axis offset.
fn split_index_i64_to_usize(index: i64, axis_len: usize, name: &str) -> RunResult<usize> {
    let axis_len = i64::try_from(axis_len)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, format!("{name}() axis is too large")))?;
    let resolved = if index < 0 { index + axis_len } else { index };
    usize::try_from(resolved.clamp(0, axis_len))
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, format!("{name}() split index is too large")).into())
}

/// Builds a list of ndarray chunks for a set of split points along one axis.
fn split_ndarray_along_axis_to_list(
    arr: &NdArray,
    axis: usize,
    split_indices: &[usize],
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let mut parts = Vec::new();
    let mut previous = 0;
    for &index in split_indices {
        let end = index.min(arr.shape()[axis]);
        parts.push(axis_chunk_value(arr, axis, previous, end, vm)?);
        previous = end;
    }
    parts.push(axis_chunk_value(arr, axis, previous, arr.shape()[axis], vm)?);
    let list = List::new(parts);
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(list))?))
}

/// Allocates one ndarray chunk for a half-open range along a fixed axis.
fn axis_chunk_value(
    arr: &NdArray,
    axis: usize,
    start_axis: usize,
    end_axis: usize,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let data = slice_ndarray_along_axis(arr, axis, start_axis, end_axis);
    check_array_alloc_size(data.len(), vm.heap.tracker())?;
    let mut shape = arr.shape().to_vec();
    shape[axis] = end_axis.saturating_sub(start_axis);
    let chunk = NdArray::new(data, shape, arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(chunk))?))
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
    let result_data = argsort_index_data(arr.data());
    let len = result_data.len();
    let result = NdArray::new(result_data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.argpartition(a, kth)` — deterministic argsort-compatible subset.
fn call_argpartition(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, kth) = args.get_two_args("numpy.argpartition", vm.heap)?;
    defer_drop!(arg, vm);
    defer_drop!(kth, vm);
    let arr = ndarray_from_value(arg, "numpy.argpartition", vm)?;
    validate_partition_kth(kth, arr.data().len(), "numpy.argpartition")?;
    let result_data = argsort_index_data(arr.data());
    let len = result_data.len();
    let result = NdArray::new(result_data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.partition(a, kth)` — deterministic sorted-output subset for 1-D arrays.
fn call_partition(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, kth) = args.get_two_args("numpy.partition", vm.heap)?;
    defer_drop!(arg, vm);
    defer_drop!(kth, vm);
    let arr = ndarray_from_value(arg, "numpy.partition", vm)?;
    validate_partition_kth(kth, arr.data().len(), "numpy.partition")?;
    let mut data = arr.data().to_vec();
    data.sort_by(nan_last_cmp);
    let result = NdArray::new(data, arr.shape().to_vec(), arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.lexsort(keys)` — indirect stable sort using the last key as primary.
fn call_lexsort(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let keys_val = args.get_one_arg("numpy.lexsort", vm.heap)?;
    defer_drop!(keys_val, vm);
    let key_values = sequence_items(keys_val, "numpy.lexsort", vm)?;
    defer_drop!(key_values, vm);
    let keys = key_values
        .iter()
        .map(|value| ndarray_from_value(value, "numpy.lexsort", vm))
        .collect::<RunResult<Vec<_>>>()?;
    let Some(first) = keys.first() else {
        let result = NdArray::new(Vec::new(), vec![0], NdArrayDtype::Int64);
        return Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?));
    };
    let len = first.data().len();
    for key in &keys {
        if key.shape().len() != 1 || key.data().len() != len {
            return Err(SimpleException::new_msg(ExcType::ValueError, "all keys need to be the same shape").into());
        }
    }
    check_array_alloc_size(len, vm.heap.tracker())?;

    let mut indices: Vec<usize> = (0..len).collect();
    indices.sort_by(|&lhs, &rhs| compare_lexsort_indices(&keys, lhs, rhs));
    let result_data = indices.into_iter().map(usize_to_f64).collect();
    let result = NdArray::new(result_data, vec![len], NdArrayDtype::Int64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Produces stable argsort indices encoded in Monty's integer ndarray storage.
fn argsort_index_data(data: &[f64]) -> Vec<f64> {
    let mut indices: Vec<usize> = (0..data.len()).collect();
    indices.sort_by(|&a, &b| nan_last_cmp(&data[a], &data[b]));
    indices.into_iter().map(usize_to_f64).collect()
}

/// Validates the scalar `kth` accepted by the supported partition subset.
fn validate_partition_kth(kth: &Value, len: usize, name: &str) -> RunResult<()> {
    let kth = value_to_i64_arg(kth, name, "kth")?;
    let len_i64 = usize_to_i64(len)?;
    let normalized = if kth < 0 { len_i64.saturating_add(kth) } else { kth };
    if normalized < 0 || normalized >= len_i64 {
        Err(SimpleException::new_msg(ExcType::ValueError, "kth out of bounds").into())
    } else {
        Ok(())
    }
}

/// Compares two row indices across `lexsort` keys, with later keys taking priority.
fn compare_lexsort_indices(keys: &[NdArray], lhs: usize, rhs: usize) -> Ordering {
    for key in keys.iter().rev() {
        let ordering = nan_last_cmp(&key.data()[lhs], &key.data()[rhs]);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    lhs.cmp(&rhs)
}

/// `numpy.cov(m)` — covariance for 1-D or row-wise 2-D real arrays.
fn call_cov(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.cov", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.cov", vm)?;
    let (rows, _) = covariance_shape(&arr, "numpy.cov")?;
    let data = covariance_matrix_data(&arr, "numpy.cov", vm.heap.tracker())?;
    if rows == 1 {
        Ok(Value::Float(data[0]))
    } else {
        let result = NdArray::new(data, vec![rows, rows], NdArrayDtype::Float64);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
    }
}

/// `numpy.corrcoef(x)` — correlation coefficients for 1-D or row-wise 2-D arrays.
fn call_corrcoef(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("numpy.corrcoef", vm.heap)?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.corrcoef", vm)?;
    let (rows, _) = covariance_shape(&arr, "numpy.corrcoef")?;
    let cov = covariance_matrix_data(&arr, "numpy.corrcoef", vm.heap.tracker())?;
    if rows == 1 {
        Ok(Value::Float(1.0))
    } else {
        let mut data = Vec::with_capacity(cov.len());
        for row in 0..rows {
            for col in 0..rows {
                let denom = (cov[row * rows + row] * cov[col * rows + col]).sqrt();
                data.push(if denom.is_nan() || denom <= 0.0 {
                    f64::NAN
                } else {
                    cov[row * rows + col] / denom
                });
            }
        }
        let result = NdArray::new(data, vec![rows, rows], NdArrayDtype::Float64);
        Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
    }
}

/// Returns `(variables, observations)` for covariance-style helpers.
fn covariance_shape(arr: &NdArray, name: &str) -> RunResult<(usize, usize)> {
    match arr.shape() {
        [cols] => Ok((1, *cols)),
        [rows, cols] => Ok((*rows, *cols)),
        _ => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("{name}() input has more than 2 dimensions"),
        )
        .into()),
    }
}

/// Computes the row-wise sample covariance matrix, matching NumPy's default `bias=False`.
fn covariance_matrix_data(arr: &NdArray, name: &str, tracker: &impl ResourceTracker) -> RunResult<Vec<f64>> {
    let (rows, cols) = covariance_shape(arr, name)?;
    check_array_alloc_size(rows.saturating_mul(rows), tracker)?;
    let means = (0..rows)
        .map(|row| covariance_row_mean(arr, row, cols))
        .collect::<Vec<_>>();
    let denom = if cols > 1 { usize_to_f64(cols - 1) } else { f64::NAN };
    let mut data = Vec::with_capacity(rows * rows);
    for lhs in 0..rows {
        for rhs in 0..rows {
            let mut sum = 0.0;
            for col in 0..cols {
                let lhs_delta = covariance_value(arr, lhs, col, cols) - means[lhs];
                let rhs_delta = covariance_value(arr, rhs, col, cols) - means[rhs];
                sum += lhs_delta * rhs_delta;
            }
            data.push(sum / denom);
        }
    }
    Ok(data)
}

/// Computes one variable row mean for covariance-style helpers.
fn covariance_row_mean(arr: &NdArray, row: usize, cols: usize) -> f64 {
    let sum = (0..cols).map(|col| covariance_value(arr, row, col, cols)).sum::<f64>();
    sum / usize_to_f64(cols)
}

/// Reads a row/column value from either 1-D or row-wise 2-D covariance input.
fn covariance_value(arr: &NdArray, row: usize, col: usize, cols: usize) -> f64 {
    if arr.shape().len() == 1 {
        arr.data()[col]
    } else {
        arr.data()[row * cols + col]
    }
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

/// `numpy.trim_zeros(filt, trim='fb')` — trim leading and/or trailing zeros from a 1-D input.
fn call_trim_zeros(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (arg, trim) = args.get_one_two_args("numpy.trim_zeros", vm.heap)?;
    defer_drop!(arg, vm);
    let trim = if let Some(trim) = trim {
        defer_drop!(trim, vm);
        string_arg(trim, "numpy.trim_zeros", vm)?
    } else {
        "fb".to_owned()
    };
    let arr = ndarray_from_value(arg, "numpy.trim_zeros", vm)?;
    let trim_front = trim.contains('f') || trim.contains('F');
    let trim_back = trim.contains('b') || trim.contains('B');
    let mut start = 0usize;
    let mut end = arr.data().len();
    if trim_front {
        start = arr.data().iter().position(|value| *value != 0.0).unwrap_or(end);
    }
    if trim_back {
        end = arr
            .data()
            .iter()
            .rposition(|value| *value != 0.0)
            .map_or(start, |index| index + 1);
    }
    if end < start {
        end = start;
    }
    let data = arr.data()[start..end].to_vec();
    let len = data.len();
    let result = NdArray::new(data, vec![len], arr.dtype());
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// `numpy.unwrap(p, discont=None, axis=-1)` over Monty's real ndarray values.
fn call_unwrap(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.unwrap", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let arg = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.unwrap", 1, 0))?;
    defer_drop!(arg, vm);
    let arr = ndarray_from_value(arg, "numpy.unwrap", vm)?;
    if arr.ndim() == 0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "diff requires input that is at least one dimensional",
        )
        .into());
    }

    let discont = if let Some(discont_val) = pos.next() {
        defer_drop!(discont_val, vm);
        if matches!(discont_val, Value::None) {
            None
        } else {
            Some(to_f64(discont_val, vm)?)
        }
    } else {
        None
    };
    if let Some(axis_val) = pos.next() {
        defer_drop!(axis_val, vm);
        if !matches!(axis_val, Value::None) {
            let axis = value_to_i64_arg(axis_val, "numpy.unwrap", "axis")?;
            let axis = normalize_axis(axis, arr.ndim(), "numpy.unwrap")?;
            if axis != arr.ndim() - 1 {
                return Err(SimpleException::new_msg(
                    ExcType::ValueError,
                    "numpy.unwrap() only supports the last axis",
                )
                .into());
            }
        }
    }
    if let Some(extra) = pos.next() {
        extra.drop_with_heap(vm);
        return Err(ExcType::type_error_at_most("numpy.unwrap", 3, 4));
    }

    check_array_alloc_size(arr.data().len(), vm.heap.tracker())?;
    let data = unwrap_phase_values(arr.data(), discont);
    let result = NdArray::new(data, arr.shape().to_vec(), NdArrayDtype::Float64);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Computes NumPy-style phase unwrapping with the default `2*pi` period.
fn unwrap_phase_values(values: &[f64], discont: Option<f64>) -> Vec<f64> {
    let Some(&first) = values.first() else {
        return Vec::new();
    };
    let period = 2.0 * PI;
    let threshold = discont.unwrap_or(PI).max(PI);
    let mut output = Vec::with_capacity(values.len());
    output.push(first);
    let mut correction = 0.0;
    for pair in values.windows(2) {
        correction += unwrap_delta_correction(pair[1] - pair[0], threshold, period);
        output.push(pair[1] + correction);
    }
    output
}

/// Correction needed to map one phase delta into the requested discontinuity interval.
fn unwrap_delta_correction(delta: f64, threshold: f64, period: f64) -> f64 {
    if delta.abs() <= threshold {
        0.0
    } else {
        let half_period = period / 2.0;
        let mut delta_mod = (delta + half_period).rem_euclid(period) - half_period;
        if delta_mod.to_bits() == (-half_period).to_bits() && delta > 0.0 {
            delta_mod = half_period;
        }
        delta_mod - delta
    }
}

/// Extracts a string argument from heap or interned string values.
fn string_arg(value: &Value, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<String> {
    match value {
        Value::InternString(id) => Ok(vm.interns.get_str(*id).to_owned()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(value) => Ok(value.as_str().to_owned()),
            _ => Err(ExcType::type_error(format!("{name}() expected a string argument"))),
        },
        _ => Err(ExcType::type_error(format!("{name}() expected a string argument"))),
    }
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

/// `numpy.kron(a, b)` — Kronecker product for numeric ndarrays.
fn call_kron(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a_val, b_val) = args.get_two_args("numpy.kron", vm.heap)?;
    defer_drop!(a_val, vm);
    defer_drop!(b_val, vm);
    let a = ndarray_from_value(a_val, "numpy.kron", vm)?;
    let b = ndarray_from_value(b_val, "numpy.kron", vm)?;
    let result = kron_arrays(&a, &b, vm.heap.tracker())?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::NdArray(result))?))
}

/// Computes the Kronecker product using NumPy's left-padded shape alignment.
fn kron_arrays(a: &NdArray, b: &NdArray, tracker: &impl ResourceTracker) -> RunResult<NdArray> {
    let ndim = a.ndim().max(b.ndim());
    let a_shape = left_padded_shape(a.shape(), ndim);
    let b_shape = left_padded_shape(b.shape(), ndim);
    let output_shape = a_shape
        .iter()
        .zip(b_shape.iter())
        .map(|(&lhs, &rhs)| lhs.checked_mul(rhs))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "numpy.kron() dimensions overflow"))?;
    let output_len = checked_shape_product(&output_shape, "numpy.kron")?;
    check_array_alloc_size(output_len, tracker)?;

    let mut data = vec![0.0; output_len];
    for (a_index, &a_value) in a.data().iter().enumerate() {
        let a_coords = flat_index_to_coords(a_index, &a_shape);
        for (b_index, &b_value) in b.data().iter().enumerate() {
            let b_coords = flat_index_to_coords(b_index, &b_shape);
            let output_coords = a_coords
                .iter()
                .zip(b_coords.iter())
                .zip(b_shape.iter())
                .map(|((&a_coord, &b_coord), &b_dim)| a_coord * b_dim + b_coord)
                .collect::<Vec<_>>();
            let output_index = coords_to_flat_index(&output_coords, &output_shape);
            data[output_index] = a_value * b_value;
        }
    }
    Ok(NdArray::new(data, output_shape, promote_dtype(a.dtype(), b.dtype())))
}

/// Left-pads an ndarray shape with ones to participate in NumPy-style shape alignment.
fn left_padded_shape(shape: &[usize], ndim: usize) -> Vec<usize> {
    let mut padded = vec![1; ndim.saturating_sub(shape.len())];
    padded.extend_from_slice(shape);
    padded
}

/// Converts a row-major flat index to coordinates for a shape.
fn flat_index_to_coords(mut index: usize, shape: &[usize]) -> Vec<usize> {
    let mut coords = vec![0; shape.len()];
    for axis in (0..shape.len()).rev() {
        let dim = shape[axis];
        if dim > 0 {
            coords[axis] = index % dim;
            index /= dim;
        }
    }
    coords
}

/// Converts row-major coordinates to a flat index.
fn coords_to_flat_index(coords: &[usize], shape: &[usize]) -> usize {
    coords
        .iter()
        .zip(shape.iter())
        .fold(0usize, |index, (&coord, &dim)| index * dim + coord)
}

/// `numpy.trapezoid(y, x=None, dx=1.0)` — integrate 1-D samples by the trapezoidal rule.
fn call_trapezoid(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.trapezoid", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let y_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.trapezoid", 1, 0))?;
    defer_drop!(y_val, vm);
    let x_val = pos.next();
    let dx_val = pos.next();
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let y = ndarray_from_value(y_val, "numpy.trapezoid", vm)?;
    let x = if let Some(x_val) = x_val {
        defer_drop!(x_val, vm);
        if matches!(x_val, Value::None) {
            None
        } else {
            Some(ndarray_from_value(x_val, "numpy.trapezoid", vm)?)
        }
    } else {
        None
    };
    let dx = if let Some(dx_val) = dx_val {
        defer_drop!(dx_val, vm);
        to_f64(dx_val, vm)?
    } else {
        1.0
    };

    let result = trapezoid_1d(&y, x.as_ref(), dx)?;
    Ok(Value::Float(result))
}

/// Integrates flattened samples using either explicit x-coordinates or a fixed spacing.
fn trapezoid_1d(y: &NdArray, x: Option<&NdArray>, dx: f64) -> RunResult<f64> {
    if let Some(x) = x
        && x.len() != y.len()
    {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "numpy.trapezoid() x and y must have same length").into(),
        );
    }

    let mut total = 0.0;
    for index in 1..y.len() {
        let width = x.map_or(dx, |coords| coords.data()[index] - coords.data()[index - 1]);
        total += (y.data()[index - 1] + y.data()[index]) * 0.5 * width;
    }
    Ok(total)
}

/// `numpy.vander(x, N=None, increasing=False)` — construct a Vandermonde matrix.
fn call_vander(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let pos = args.into_pos_only("numpy.vander", vm.heap)?;
    defer_drop_mut!(pos, vm);

    let x_val = pos
        .next()
        .ok_or_else(|| ExcType::type_error_at_least("numpy.vander", 1, 0))?;
    defer_drop!(x_val, vm);
    let n_val = pos.next();
    let increasing_val = pos.next();
    for extra in pos {
        extra.drop_with_heap(vm);
    }

    let x = ndarray_from_value(x_val, "numpy.vander", vm)?;
    let n = if let Some(n_val) = n_val {
        defer_drop!(n_val, vm);
        if matches!(n_val, Value::None) {
            x.len()
        } else {
            value_to_nonnegative_usize(n_val, "numpy.vander", "N")?
        }
    } else {
        x.len()
    };
    let increasing = if let Some(increasing_val) = increasing_val {
        defer_drop!(increasing_val, vm);
        value_to_bool_arg(increasing_val, "numpy.vander", "increasing")?
    } else {
        false
    };

    vander_1d(&x, n, increasing, vm.heap)
}

/// Builds a Vandermonde matrix for a 1-D numeric input.
fn vander_1d(x: &NdArray, n: usize, increasing: bool, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    if x.ndim() == 1 {
        let len = x.len();
        check_array_alloc_size(len * n, heap.tracker())?;
        let mut data = Vec::with_capacity(len * n);
        for &value in x.data() {
            for col in 0..n {
                let power = if increasing { col } else { n - 1 - col };
                data.push(pow_usize(value, power));
            }
        }
        let result = NdArray::new(data, vec![len, n], x.dtype());
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    } else {
        Err(SimpleException::new_msg(ExcType::ValueError, "numpy.vander() x must be a one-dimensional array").into())
    }
}

/// Raises a base to a non-negative integer exponent without lossy casts.
fn pow_usize(base: f64, exponent: usize) -> f64 {
    let mut result = 1.0;
    for _ in 0..exponent {
        result *= base;
    }
    result
}

/// Converts a Python truth value argument used by NumPy option flags.
fn value_to_bool_arg(value: &Value, name: &str, arg_name: &str) -> RunResult<bool> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Int(value) => Ok(*value != 0),
        _ => Err(ExcType::type_error(format!("{name}() {arg_name} must be a boolean"))),
    }
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
