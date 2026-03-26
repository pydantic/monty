//! NumPy-compatible ndarray type for the Monty interpreter.
//!
//! Provides a multi-dimensional array of f64 values that emulates the subset of
//! `numpy.ndarray` commonly used by LLMs. Backed by a flat `Vec<f64>` with shape
//! metadata, supporting element-wise arithmetic, comparisons, indexing, and
//! aggregation methods.
//!
//! This is a built-in type (like `list` or `dict`) rather than a user-defined class,
//! so operator overloading and method dispatch are hardcoded in the VM — no class
//! support is required.
//!
//! # Supported operations
//!
//! - Element-wise arithmetic: `+`, `-`, `*`, `/`, `//`, `%`, `**`, unary `-`
//! - Scalar broadcasting: `arr + 5`, `arr * 2.0`
//! - Comparisons: `>`, `<`, `==`, `>=`, `<=`, `!=` (return boolean arrays)
//! - Boolean indexing: `arr[arr > 3]`
//! - Integer indexing: `arr[0]`, `arr[1][2]` for 2D
//! - Aggregation: `sum()`, `mean()`, `min()`, `max()`, `std()`
//! - Shape manipulation: `reshape()`, `flatten()`
//! - Element-wise transforms: `cumsum()`, `abs()`, `round()`, `clip()`, `sort()`
//! - Conversion: `tolist()`
//! - Attributes: `.shape`, `.dtype`, `.size`, `.ndim`, `.T`

use std::{
    cmp::Ordering,
    fmt::{self, Write},
};

use ahash::AHashSet;
use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{List, PyTrait, Str, Type, allocate_tuple},
    value::{EitherStr, Value},
};

/// The element type stored in an ndarray.
///
/// NumPy arrays have a dtype that determines how elements are stored and displayed.
/// We support the two most common dtypes: 64-bit integers and 64-bit floats.
/// Boolean arrays (from comparisons) use `Bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum NdArrayDtype {
    /// 64-bit signed integer (`int64` / `numpy.int64`).
    Int64,
    /// 64-bit floating point (`float64` / `numpy.float64`).
    Float64,
    /// Boolean array (`bool` / `numpy.bool_`), used for comparison results and masks.
    Bool,
}

impl fmt::Display for NdArrayDtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int64 => f.write_str("int64"),
            Self::Float64 => f.write_str("float64"),
            Self::Bool => f.write_str("bool"),
        }
    }
}

/// A multi-dimensional array of numeric values, emulating `numpy.ndarray`.
///
/// Data is stored as a flat `Vec<f64>` with shape metadata. Even integer arrays
/// store values as f64 internally — the `dtype` field controls display formatting
/// (integers show without decimal points) and type promotion rules.
///
/// Boolean arrays store 0.0 for `False` and 1.0 for `True`.
///
/// # Memory layout
///
/// Row-major (C-contiguous) order, matching NumPy's default. A 2D array with
/// shape `(3, 2)` stores elements as `[row0_col0, row0_col1, row1_col0, ...]`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct NdArray {
    /// Flat storage of all elements in row-major order.
    data: Vec<f64>,
    /// Dimensions of the array (e.g., `[3]` for 1D, `[2, 3]` for 2D).
    shape: Vec<usize>,
    /// Element type, controlling display format and type promotion.
    dtype: NdArrayDtype,
}

// ===========================
// Public constructors and accessors
// ===========================

impl NdArray {
    /// Creates a new ndarray from flat data with the given shape and dtype.
    ///
    /// The caller must ensure `data.len() == shape.iter().product()`.
    pub fn new(data: Vec<f64>, shape: Vec<usize>, dtype: NdArrayDtype) -> Self {
        debug_assert_eq!(
            data.len(),
            shape.iter().product::<usize>(),
            "data length must match shape product"
        );
        Self { data, shape, dtype }
    }

    /// Creates a 1D array from a flat vector of f64 values.
    pub fn from_vec_f64(data: Vec<f64>) -> Self {
        let len = data.len();
        Self {
            data,
            shape: vec![len],
            dtype: NdArrayDtype::Float64,
        }
    }

    /// Returns the total number of elements in the array.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns a reference to the raw f64 data backing this array.
    pub fn data(&self) -> &[f64] {
        &self.data
    }

    /// Returns the shape as a slice of dimensions.
    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    /// Returns the dtype of the array.
    pub fn dtype(&self) -> NdArrayDtype {
        self.dtype
    }

    /// Returns the number of dimensions (ndim).
    pub fn ndim(&self) -> usize {
        self.shape.len()
    }
}

// ===========================
// Indexing operations
// ===========================

impl NdArray {
    /// Indexes a 1D array by integer, returning a scalar Value.
    ///
    /// For multi-dimensional arrays, returns a sub-array (row slice).
    pub fn getitem_int(&self, index: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if self.ndim() == 1 {
            let idx = resolve_index(index, self.shape[0])?;
            Ok(self.element_to_value(self.data[idx]))
        } else {
            // For multi-dimensional arrays, return a sub-array (row)
            let idx = resolve_index(index, self.shape[0])?;
            let row_size: usize = self.shape[1..].iter().product();
            let start = idx * row_size;
            let end = start + row_size;
            let row_data = self.data[start..end].to_vec();
            let row_shape = self.shape[1..].to_vec();
            let row = Self::new(row_data, row_shape, self.dtype);
            Ok(Value::Ref(heap.allocate(HeapData::NdArray(row))?))
        }
    }

    /// Indexes by a boolean mask array, returning elements where mask is true.
    pub fn getitem_bool_mask(&self, mask: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if mask.len() != self.len() {
            return Err(
                SimpleException::new_msg(ExcType::IndexError, "boolean index did not match indexed array").into(),
            );
        }
        let filtered: Vec<f64> = self
            .data
            .iter()
            .zip(mask.data.iter())
            .filter(|(_, m)| **m != 0.0)
            .map(|(v, _)| *v)
            .collect();
        let len = filtered.len();
        let result = Self::new(filtered, vec![len], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    }

    /// Converts a single f64 element to the appropriate Value based on dtype.
    fn element_to_value(&self, val: f64) -> Value {
        match self.dtype {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f64 to i64 truncation is the intended int conversion"
            )]
            NdArrayDtype::Int64 => Value::Int(val as i64),
            NdArrayDtype::Float64 => Value::Float(val),
            NdArrayDtype::Bool => Value::Bool(val != 0.0),
        }
    }
}

// ===========================
// Element-wise binary and comparison operations
// ===========================

impl NdArray {
    /// Element-wise binary operation between two arrays of the same shape.
    fn elementwise_op(
        &self,
        other: &Self,
        op: fn(f64, f64) -> f64,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        if self.shape != other.shape {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
            );
        }
        let result_dtype = promote_dtype(self.dtype, other.dtype);
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(other.data.iter())
            .map(|(&a, &b)| op(a, b))
            .collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise operation with a scalar on the right.
    fn scalar_op_right(
        &self,
        scalar: f64,
        op: fn(f64, f64) -> f64,
        result_dtype: NdArrayDtype,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&a| op(a, scalar)).collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise operation with a scalar on the left (scalar op array).
    ///
    /// Used for non-commutative operations like `5 - arr` or `10 / arr`.
    fn scalar_op_left(
        &self,
        scalar: f64,
        op: fn(f64, f64) -> f64,
        result_dtype: NdArrayDtype,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&a| op(scalar, a)).collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise comparison, producing a boolean array.
    fn elementwise_cmp(
        &self,
        other: &Self,
        cmp: fn(f64, f64) -> bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        if self.shape != other.shape {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
            );
        }
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(other.data.iter())
            .map(|(&a, &b)| if cmp(a, b) { 1.0 } else { 0.0 })
            .collect();
        let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Bool);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Scalar comparison, producing a boolean array.
    fn scalar_cmp(
        &self,
        scalar: f64,
        cmp: fn(f64, f64) -> bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let data: Vec<f64> = self
            .data
            .iter()
            .map(|&a| if cmp(a, scalar) { 1.0 } else { 0.0 })
            .collect();
        let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Bool);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise addition with another array.
    pub fn add(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| a + b, heap)
    }

    /// Addition with a scalar value.
    pub fn add_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, |a, b| a + b, dtype, heap)
    }

    /// Element-wise subtraction.
    pub fn sub(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| a - b, heap)
    }

    /// Subtraction with a scalar value.
    pub fn sub_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, |a, b| a - b, dtype, heap)
    }

    /// Element-wise multiplication.
    pub fn mul(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| a * b, heap)
    }

    /// Multiplication with a scalar value.
    pub fn mul_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, |a, b| a * b, dtype, heap)
    }

    /// Element-wise true division (always returns float).
    pub fn div(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().zip(other.data.iter()).map(|(&a, &b)| a / b).collect();
        let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Float64);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Division with a scalar value (always returns float).
    pub fn div_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_op_right(scalar, |a, b| a / b, NdArrayDtype::Float64, heap)
    }

    /// Element-wise floor division.
    pub fn floordiv(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| (a / b).floor(), heap)
    }

    /// Floor division with a scalar value.
    pub fn floordiv_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, |a, b| (a / b).floor(), dtype, heap)
    }

    /// Element-wise modulo.
    pub fn modulo(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, py_mod, heap)
    }

    /// Modulo with a scalar value.
    pub fn modulo_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, py_mod, dtype, heap)
    }

    /// Element-wise power.
    pub fn pow(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, f64::powf, heap)
    }

    /// Power with a scalar exponent.
    pub fn pow_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_right(scalar, f64::powf, dtype, heap)
    }

    /// Reverse subtraction with scalar: `scalar - array`.
    pub fn rsub_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_left(scalar, |a, b| a - b, dtype, heap)
    }

    /// Reverse division with scalar: `scalar / array`.
    pub fn rdiv_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_op_left(scalar, |a, b| a / b, NdArrayDtype::Float64, heap)
    }

    /// Reverse floor division with scalar: `scalar // array`.
    pub fn rfloordiv_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_left(scalar, |a, b| (a / b).floor(), dtype, heap)
    }

    /// Reverse modulo with scalar: `scalar % array`.
    pub fn rmod_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_left(scalar, py_mod, dtype, heap)
    }

    /// Reverse power with scalar: `scalar ** array`.
    pub fn rpow_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar);
        self.scalar_op_left(scalar, f64::powf, dtype, heap)
    }

    /// Unary negation.
    pub fn neg(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&a| -a).collect();
        let arr = Self::new(data, self.shape.clone(), self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Unary bitwise invert (logical NOT for boolean arrays).
    ///
    /// For boolean arrays, flips True/False. For numeric arrays, behaves
    /// like element-wise logical NOT (truthy becomes 0, falsy becomes 1).
    pub fn invert(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&a| if a == 0.0 { 1.0 } else { 0.0 }).collect();
        let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Bool);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise greater-than comparison.
    pub fn gt(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_cmp(other, |a, b| a > b, heap)
    }

    /// Greater-than comparison with scalar.
    pub fn gt_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_cmp(scalar, |a, b| a > b, heap)
    }

    /// Element-wise less-than comparison.
    pub fn lt(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_cmp(other, |a, b| a < b, heap)
    }

    /// Less-than comparison with scalar.
    pub fn lt_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_cmp(scalar, |a, b| a < b, heap)
    }

    /// Element-wise equality comparison.
    pub fn eq_array(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_cmp(other, |a, b| a == b, heap)
    }

    /// Equality comparison with scalar.
    pub fn eq_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_cmp(scalar, |a, b| a == b, heap)
    }

    /// Element-wise greater-than-or-equal comparison.
    pub fn gte(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_cmp(other, |a, b| a >= b, heap)
    }

    /// Greater-than-or-equal comparison with scalar.
    pub fn gte_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_cmp(scalar, |a, b| a >= b, heap)
    }

    /// Element-wise less-than-or-equal comparison.
    pub fn lte(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_cmp(other, |a, b| a <= b, heap)
    }

    /// Less-than-or-equal comparison with scalar.
    pub fn lte_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_cmp(scalar, |a, b| a <= b, heap)
    }

    /// Element-wise not-equal comparison.
    pub fn ne_array(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        #[expect(clippy::float_cmp, reason = "exact equality is correct for numpy != semantics")]
        self.elementwise_cmp(other, |a, b| a != b, heap)
    }

    /// Not-equal comparison with scalar.
    pub fn ne_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        #[expect(clippy::float_cmp, reason = "exact equality is correct for numpy != semantics")]
        self.scalar_cmp(scalar, |a, b| a != b, heap)
    }
}

// ===========================
// Aggregation methods
// ===========================

impl NdArray {
    /// Returns the sum of all elements.
    pub fn sum(&self) -> f64 {
        self.data.iter().sum()
    }

    /// Returns the arithmetic mean of all elements.
    pub fn mean(&self) -> f64 {
        self.sum() / self.len() as f64
    }

    /// Returns the minimum element.
    pub fn min_val(&self) -> RunResult<f64> {
        self.data
            .iter()
            .copied()
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "zero-size array has no minimum").into())
    }

    /// Returns the maximum element.
    pub fn max_val(&self) -> RunResult<f64> {
        self.data
            .iter()
            .copied()
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "zero-size array has no maximum").into())
    }

    /// Returns the population standard deviation.
    pub fn std_dev(&self) -> f64 {
        let mean = self.mean();
        let variance = self.data.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / self.len() as f64;
        variance.sqrt()
    }

    /// Returns the index of the minimum element.
    pub fn argmin(&self) -> RunResult<usize> {
        self.data
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(Ordering::Equal))
            .map(|(i, _)| i)
            .ok_or_else(|| {
                SimpleException::new_msg(ExcType::ValueError, "attempt to get argmin of an empty sequence").into()
            })
    }

    /// Returns the index of the maximum element.
    pub fn argmax(&self) -> RunResult<usize> {
        self.data
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(Ordering::Equal))
            .map(|(i, _)| i)
            .ok_or_else(|| {
                SimpleException::new_msg(ExcType::ValueError, "attempt to get argmax of an empty sequence").into()
            })
    }

    /// Returns true if all elements are truthy (non-zero).
    pub fn all(&self) -> bool {
        self.data.iter().all(|&x| x != 0.0)
    }

    /// Returns true if any element is truthy (non-zero).
    pub fn any(&self) -> bool {
        self.data.iter().any(|&x| x != 0.0)
    }
}

// ===========================
// Shape manipulation and transform methods
// ===========================

impl NdArray {
    /// Reshapes the array to a new shape, returning a new NdArray.
    ///
    /// The total number of elements must remain the same.
    pub fn reshape(&self, new_shape: Vec<usize>, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let new_size: usize = new_shape.iter().product();
        if new_size != self.len() {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "cannot reshape array of size into shape").into(),
            );
        }
        let arr = Self::new(self.data.clone(), new_shape, self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Flattens the array to 1D, returning a new NdArray.
    pub fn flatten(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let arr = Self::new(self.data.clone(), vec![self.len()], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Returns the transpose of a 2D array. For 1D arrays, returns a copy.
    pub fn transpose(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if self.ndim() <= 1 {
            // 1D arrays are returned as-is (copy)
            let arr = Self::new(self.data.clone(), self.shape.clone(), self.dtype);
            return Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?));
        }
        if self.ndim() == 2 {
            let rows = self.shape[0];
            let cols = self.shape[1];
            let mut data = vec![0.0; self.data.len()];
            for r in 0..rows {
                for c in 0..cols {
                    data[c * rows + r] = self.data[r * cols + c];
                }
            }
            let arr = Self::new(data, vec![cols, rows], self.dtype);
            return Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?));
        }
        Err(ExcType::type_error("transpose not supported for arrays with ndim > 2"))
    }

    /// `cumsum()` — returns a 1D array of cumulative sums.
    pub fn cumsum(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut sum = 0.0;
        let data: Vec<f64> = self
            .data
            .iter()
            .map(|&v| {
                sum += v;
                sum
            })
            .collect();
        let arr = Self::new(data, vec![self.len()], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `round(decimals)` — returns a new array with each element rounded.
    pub fn round_array(&self, decimals: i32, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let factor = 10f64.powi(decimals);
        let data: Vec<f64> = self.data.iter().map(|&v| (v * factor).round() / factor).collect();
        let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Float64);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `clip(min, max)` — returns a new array with each element clamped to `[min, max]`.
    pub fn clip_array(&self, min: f64, max: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&v| v.clamp(min, max)).collect();
        let arr = Self::new(data, self.shape.clone(), self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `sort()` — returns a new array with elements sorted in ascending order.
    pub fn sort_array(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut data = self.data.clone();
        data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let arr = Self::new(data, self.shape.clone(), self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `argsort()` — returns indices that would sort the array.
    pub fn argsort(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut indices: Vec<usize> = (0..self.data.len()).collect();
        indices.sort_by(|&a, &b| self.data[a].partial_cmp(&self.data[b]).unwrap_or(Ordering::Equal));
        let data: Vec<f64> = indices.iter().map(|&i| i as f64).collect();
        let arr = Self::new(data, vec![self.data.len()], NdArrayDtype::Int64);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `astype(dtype_str)` — cast array to a new dtype.
    pub fn astype(&self, dtype_str: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let new_dtype = match dtype_str {
            "int64" | "int" => NdArrayDtype::Int64,
            "float64" | "float" => NdArrayDtype::Float64,
            "bool" => NdArrayDtype::Bool,
            _ => {
                return Err(
                    SimpleException::new_msg(ExcType::TypeError, format!("unsupported dtype: {dtype_str}")).into(),
                );
            }
        };
        let data = match new_dtype {
            NdArrayDtype::Bool => self.data.iter().map(|&v| if v == 0.0 { 0.0 } else { 1.0 }).collect(),
            NdArrayDtype::Int64 =>
            {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "f64 to i64 truncation is the intended int conversion"
                )]
                self.data.iter().map(|&v| (v as i64) as f64).collect()
            }
            NdArrayDtype::Float64 => self.data.clone(),
        };
        let arr = Self::new(data, self.shape.clone(), new_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `dot(other)` — dot product of two 1D arrays, returning a scalar.
    pub fn dot(&self, other: &Self) -> RunResult<Value> {
        if self.data.len() != other.data.len() {
            return Err(SimpleException::new_msg(ExcType::ValueError, "shapes are not aligned for dot product").into());
        }
        let result: f64 = self.data.iter().zip(other.data.iter()).map(|(&a, &b)| a * b).sum();
        let result_dtype = promote_dtype(self.dtype, other.dtype);
        if result_dtype == NdArrayDtype::Int64 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f64 to i64 truncation is intended for int dot product"
            )]
            return Ok(Value::Int(result as i64));
        }
        Ok(Value::Float(result))
    }

    /// Returns a copy of this ndarray.
    pub fn copy_array(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let arr = Self::new(self.data.clone(), self.shape.clone(), self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Converts the array to a Python list.
    pub fn tolist(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let values: Vec<Value> = self.data.iter().map(|&v| self.element_to_value(v)).collect();
        let list = List::new(values);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
    }
}

// ===========================
// Attribute accessors
// ===========================

impl NdArray {
    /// Returns the shape as a Python tuple of ints.
    pub fn shape_tuple(&self, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
        #[expect(clippy::cast_possible_wrap, reason = "shape dimensions won't exceed i64::MAX")]
        let values: SmallVec<[Value; 3]> = self.shape.iter().map(|&d| Value::Int(d as i64)).collect();
        allocate_tuple(values, heap)
    }

    /// Returns the dtype as a string Value.
    pub fn dtype_str(&self, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
        let s = Str::new(self.dtype.to_string());
        Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
    }
}

// ===========================
// Repr formatting
// ===========================

impl NdArray {
    /// Writes the repr format to the given formatter.
    ///
    /// Produces output like `array([1, 2, 3])` for int arrays
    /// or `array([1., 2., 3.])` for float arrays.
    pub fn py_repr_fmt_inner(&self, f: &mut impl Write) -> fmt::Result {
        f.write_str("array(")?;
        self.write_recursive(f, &self.shape, 0)?;
        f.write_char(')')
    }

    /// Recursively writes nested list representation for multi-dimensional arrays.
    fn write_recursive(&self, f: &mut impl Write, remaining_shape: &[usize], offset: usize) -> fmt::Result {
        if remaining_shape.len() == 1 {
            f.write_char('[')?;
            let len = remaining_shape[0];
            for i in 0..len {
                if i > 0 {
                    f.write_str(", ")?;
                }
                let val = self.data[offset + i];
                match self.dtype {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "f64 to i64 truncation is the intended int display"
                    )]
                    NdArrayDtype::Int64 => write!(f, "{}", val as i64)?,
                    NdArrayDtype::Float64 => {
                        if val.fract() == 0.0 && val.is_finite() {
                            write!(f, "{val:.1}")?;
                        } else {
                            write!(f, "{val}")?;
                        }
                    }
                    NdArrayDtype::Bool => {
                        if val == 0.0 {
                            f.write_str("False")?;
                        } else {
                            f.write_str(" True")?;
                        }
                    }
                }
            }
            f.write_char(']')
        } else {
            f.write_char('[')?;
            let sub_size: usize = remaining_shape[1..].iter().product();
            for i in 0..remaining_shape[0] {
                if i > 0 {
                    f.write_str(", ")?;
                }
                self.write_recursive(f, &remaining_shape[1..], offset + i * sub_size)?;
            }
            f.write_char(']')
        }
    }
}

// ===========================
// PyTrait implementation via HeapRead
// ===========================

impl<'h> PyTrait<'h> for HeapRead<'h, NdArray> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::NdArray
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        Ok(a.shape == b.shape && a.data == b.data && a.dtype == b.dtype)
    }

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        let arr = self.get(vm.heap);
        // NumPy only allows bool() on single-element arrays.
        // For 0 or >1 elements, NumPy raises ValueError — but the py_bool trait
        // returns bool, not Result, so we fall back to non-empty check.
        // TODO: propagate ValueError when py_bool returns RunResult<bool>.
        if arr.len() == 1 {
            arr.data[0] != 0.0
        } else {
            !arr.data.is_empty()
        }
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        Ok(self.get(vm.heap).py_repr_fmt_inner(f)?)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let arr = self.get(vm.heap);
        match key {
            Value::Int(n) => arr.getitem_int(*n, vm.heap),
            Value::Bool(b) => arr.getitem_int(i64::from(*b), vm.heap),
            Value::Ref(mask_id) => {
                if let HeapData::NdArray(mask) = vm.heap.get(*mask_id) {
                    arr.getitem_bool_mask(mask, vm.heap)
                } else {
                    Err(ExcType::type_error(
                        "ndarray indices must be integers or boolean arrays",
                    ))
                }
            }
            _ => Err(ExcType::type_error(
                "ndarray indices must be integers or boolean arrays",
            )),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let arr = self.get(vm.heap);
        let result = match attr.static_string() {
            Some(StaticStrings::NpShape) => arr.shape_tuple(vm.heap)?,
            Some(StaticStrings::Dtype) => arr.dtype_str(vm.heap)?,
            #[expect(clippy::cast_possible_wrap, reason = "array length won't exceed i64::MAX")]
            Some(StaticStrings::NpSize) => Value::Int(arr.len() as i64),
            #[expect(clippy::cast_possible_wrap, reason = "ndim is always small")]
            Some(StaticStrings::NpNdim) => Value::Int(arr.ndim() as i64),
            Some(StaticStrings::NpT) => arr.transpose(vm.heap)?,
            _ => return Err(ExcType::attribute_error(Type::NdArray, attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(result)))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let result = match attr.static_string() {
            Some(StaticStrings::NpSum) => {
                args.check_zero_args("ndarray.sum", vm.heap)?;
                Ok(call_sum(self.get(vm.heap)))
            }
            Some(StaticStrings::Mean) => {
                args.check_zero_args("ndarray.mean", vm.heap)?;
                Ok(Value::Float(self.get(vm.heap).mean()))
            }
            Some(StaticStrings::NpMin) => {
                args.check_zero_args("ndarray.min", vm.heap)?;
                call_min(self.get(vm.heap))
            }
            Some(StaticStrings::NpMax) => {
                args.check_zero_args("ndarray.max", vm.heap)?;
                call_max(self.get(vm.heap))
            }
            Some(StaticStrings::Std) => {
                args.check_zero_args("ndarray.std", vm.heap)?;
                Ok(Value::Float(self.get(vm.heap).std_dev()))
            }
            Some(StaticStrings::Flatten) => {
                args.check_zero_args("ndarray.flatten", vm.heap)?;
                self.get(vm.heap).flatten(vm.heap)
            }
            Some(StaticStrings::Tolist) => {
                args.check_zero_args("ndarray.tolist", vm.heap)?;
                self.get(vm.heap).tolist(vm.heap)
            }
            Some(StaticStrings::Copy) => {
                args.check_zero_args("ndarray.copy", vm.heap)?;
                self.get(vm.heap).copy_array(vm.heap)
            }
            Some(StaticStrings::Sort) => {
                args.check_zero_args("ndarray.sort", vm.heap)?;
                self.get(vm.heap).sort_array(vm.heap)
            }
            Some(StaticStrings::NpArgsort) => {
                args.check_zero_args("ndarray.argsort", vm.heap)?;
                self.get(vm.heap).argsort(vm.heap)
            }
            Some(StaticStrings::Argmin) => {
                args.check_zero_args("ndarray.argmin", vm.heap)?;
                #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
                Ok(Value::Int(self.get(vm.heap).argmin()? as i64))
            }
            Some(StaticStrings::Argmax) => {
                args.check_zero_args("ndarray.argmax", vm.heap)?;
                #[expect(clippy::cast_possible_wrap, reason = "array index won't exceed i64::MAX")]
                Ok(Value::Int(self.get(vm.heap).argmax()? as i64))
            }
            Some(StaticStrings::NpAll) => {
                args.check_zero_args("ndarray.all", vm.heap)?;
                Ok(Value::Bool(self.get(vm.heap).all()))
            }
            Some(StaticStrings::NpAny) => {
                args.check_zero_args("ndarray.any", vm.heap)?;
                Ok(Value::Bool(self.get(vm.heap).any()))
            }
            Some(StaticStrings::Cumsum) => {
                args.check_zero_args("ndarray.cumsum", vm.heap)?;
                self.get(vm.heap).cumsum(vm.heap)
            }
            Some(StaticStrings::Reshape) => {
                let pos = args.into_pos_only("ndarray.reshape", vm.heap)?;
                let result = call_reshape(self.get(vm.heap), pos.as_slice(), vm.heap);
                pos.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::Round) => {
                let opt = args.get_zero_one_arg("ndarray.round", vm.heap)?;
                #[expect(clippy::cast_possible_truncation, reason = "decimals value from user input")]
                let decimals = match opt {
                    Some(Value::Int(n)) => n as i32,
                    Some(other) => {
                        other.drop_with_heap(vm);
                        return Err(ExcType::type_error("decimals must be an integer"));
                    }
                    None => 0,
                };
                self.get(vm.heap).round_array(decimals, vm.heap)
            }
            Some(StaticStrings::Clip) => {
                let pos = args.into_pos_only("ndarray.clip", vm.heap)?;
                let result = if pos.as_slice().len() >= 2 {
                    let min_val = extract_f64(&pos.as_slice()[0]);
                    let max_val = extract_f64(&pos.as_slice()[1]);
                    self.get(vm.heap).clip_array(min_val, max_val, vm.heap)
                } else {
                    Err(ExcType::type_error("clip() requires min and max arguments"))
                };
                pos.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::Dot) => {
                let other_val = args.get_one_arg("ndarray.dot", vm.heap)?;
                let result = match &other_val {
                    Value::Ref(other_id) => {
                        if let HeapData::NdArray(other) = vm.heap.get(*other_id) {
                            self.get(vm.heap).dot(other)
                        } else {
                            Err(ExcType::type_error("dot() requires an ndarray argument"))
                        }
                    }
                    _ => Err(ExcType::type_error("dot() requires an ndarray argument")),
                };
                other_val.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::NpAstype) => {
                let arg = args.get_one_arg("ndarray.astype", vm.heap)?;
                let result = match &arg {
                    Value::InternString(id) => {
                        let name = vm.interns.get_str(*id);
                        self.get(vm.heap).astype(name, vm.heap)
                    }
                    Value::Ref(id) => {
                        if let HeapData::Str(s) = vm.heap.get(*id) {
                            let name = s.as_str().to_owned();
                            self.get(vm.heap).astype(&name, vm.heap)
                        } else {
                            Err(ExcType::type_error("astype() requires a string argument"))
                        }
                    }
                    _ => Err(ExcType::type_error("astype() requires a string argument")),
                };
                arg.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::NpTranspose) => {
                args.check_zero_args("ndarray.transpose", vm.heap)?;
                self.get(vm.heap).transpose(vm.heap)
            }
            _ => {
                args.drop_with_heap(vm);
                return Err(ExcType::attribute_error(Type::NdArray, attr.as_str(vm.interns)));
            }
        };
        result.map(CallResult::Value)
    }
}

// ===========================
// HeapItem implementation
// ===========================

impl HeapItem for NdArray {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.data.capacity() * std::mem::size_of::<f64>()
            + self.shape.capacity() * std::mem::size_of::<usize>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // NdArray is a leaf type — stores only f64 data, no heap references.
    }
}

// ===========================
// Helper functions
// ===========================

/// Returns `sum()` with dtype-appropriate return type.
fn call_sum(arr: &NdArray) -> Value {
    let s = arr.sum();
    match arr.dtype() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64 to i64 truncation is intended for int sum"
        )]
        NdArrayDtype::Int64 => Value::Int(s as i64),
        NdArrayDtype::Float64 | NdArrayDtype::Bool => Value::Float(s),
    }
}

/// Returns `min()` as a scalar matching the array's dtype.
fn call_min(arr: &NdArray) -> RunResult<Value> {
    let m = arr.min_val()?;
    Ok(arr.element_to_value(m))
}

/// Returns `max()` as a scalar matching the array's dtype.
fn call_max(arr: &NdArray) -> RunResult<Value> {
    let m = arr.max_val()?;
    Ok(arr.element_to_value(m))
}

/// Handles `reshape(*shape)` — takes shape as positional args.
fn call_reshape(arr: &NdArray, args: &[Value], heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut new_shape = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "shape dimensions from user input"
            )]
            Value::Int(n) => new_shape.push(*n as usize),
            _ => {
                return Err(ExcType::type_error("an integer is required for reshape dimensions"));
            }
        }
    }
    arr.reshape(new_shape, heap)
}

/// Extracts an f64 from a Value, returning 0.0 for unsupported types.
///
/// Used by ndarray methods (like `clip`) that accept numeric arguments from Python.
fn extract_f64(value: &Value) -> f64 {
    match value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "i64 to f64 precision loss acceptable for numeric args"
        )]
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        Value::Bool(true) => 1.0,
        _ => 0.0,
    }
}

/// Converts a potentially negative index to a positive one, or returns an error.
///
/// Supports Python-style negative indexing: `-1` is the last element, etc.
fn resolve_index(index: i64, axis_len: usize) -> RunResult<usize> {
    #[expect(clippy::cast_possible_wrap, reason = "axis_len won't exceed i64::MAX")]
    let resolved = if index < 0 {
        let pos = axis_len as i64 + index;
        if pos < 0 {
            return Err(SimpleException::new_msg(ExcType::IndexError, "index out of range").into());
        }
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "pos is guaranteed non-negative above"
        )]
        let r = pos as usize;
        r
    } else {
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "index is guaranteed non-negative"
        )]
        let r = index as usize;
        r
    };
    if resolved >= axis_len {
        return Err(SimpleException::new_msg(ExcType::IndexError, "index out of range").into());
    }
    Ok(resolved)
}

/// Creates an ndarray from a Python list Value (potentially nested).
///
/// Recursively traverses nested lists to determine shape and flatten data.
pub(crate) fn ndarray_from_list(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<NdArray> {
    let mut data = Vec::new();
    let mut shape = Vec::new();
    let mut has_float = false;
    collect_from_value(value, heap, &mut data, &mut shape, 0, &mut has_float)?;

    let dtype = if has_float {
        NdArrayDtype::Float64
    } else {
        NdArrayDtype::Int64
    };

    Ok(NdArray::new(data, shape, dtype))
}

/// Recursively collects numeric data from a nested list/value structure.
fn collect_from_value(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
    data: &mut Vec<f64>,
    shape: &mut Vec<usize>,
    depth: usize,
    has_float: &mut bool,
) -> RunResult<()> {
    match value {
        Value::Int(n) => {
            data.push(*n as f64);
            Ok(())
        }
        Value::Float(f) => {
            *has_float = true;
            data.push(*f);
            Ok(())
        }
        Value::Bool(b) => {
            data.push(if *b { 1.0 } else { 0.0 });
            Ok(())
        }
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::List(list) => {
                let items = list.as_slice();
                let len = items.len();

                if depth >= shape.len() {
                    shape.push(len);
                } else if shape[depth] != len {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "setting an array element with a sequence",
                    )
                    .into());
                }

                for item in items {
                    collect_from_value(item, heap, data, shape, depth + 1, has_float)?;
                }
                Ok(())
            }
            _ => Err(ExcType::type_error("cannot create ndarray from this type")),
        },
        _ => Err(ExcType::type_error("cannot create ndarray from this type")),
    }
}

/// Determines the result dtype when combining two dtypes.
///
/// Follows NumPy's type promotion: if either operand is float, result is float.
pub(crate) fn promote_dtype(a: NdArrayDtype, b: NdArrayDtype) -> NdArrayDtype {
    match (a, b) {
        (NdArrayDtype::Float64, _) | (_, NdArrayDtype::Float64) => NdArrayDtype::Float64,
        _ => NdArrayDtype::Int64,
    }
}

/// Determines result dtype when combining an array dtype with a scalar.
fn promote_dtype_with_scalar(arr_dtype: NdArrayDtype, scalar: f64) -> NdArrayDtype {
    if arr_dtype == NdArrayDtype::Float64 || scalar.fract() != 0.0 {
        NdArrayDtype::Float64
    } else {
        arr_dtype
    }
}

/// Python-compatible modulo: result has the same sign as the divisor.
fn py_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r > 0.0) != (b > 0.0)) { r + b } else { r }
}
