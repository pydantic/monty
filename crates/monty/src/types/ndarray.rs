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
//! - Element-wise transforms: `cumsum()`, `cumprod()`, `abs()`, `round()`, `clip()`, `sort()`
//! - Selection: `take()`, `compress()`, `diagonal()`, `item()`, `squeeze()`
//! - In-place: `fill()`
//! - Linear algebra: `trace()`, `swapaxes()`
//! - Conversion: `tolist()`
//! - Attributes: `.shape`, `.dtype`, `.size`, `.ndim`, `.T`, `.nbytes`, `.itemsize`

use std::{
    cmp::Ordering,
    fmt::{self, Write},
    mem::size_of,
    string::ToString,
};

use ahash::AHashSet;
use smallvec::{SmallVec, smallvec};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropWithHeap, Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker, check_array_alloc_size},
    types::{List, PyTrait, Slice, Str, Type, allocate_tuple},
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
    pub(crate) data: Vec<f64>,
    /// Dimensions of the array (e.g., `[3]` for 1D, `[2, 3]` for 2D).
    pub(crate) shape: Vec<usize>,
    /// Element type, controlling display format and type promotion.
    pub(crate) dtype: NdArrayDtype,
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

    /// Indexes by an integer ndarray (fancy indexing), gathering elements at the specified indices.
    pub fn getitem_int_array(&self, idx_arr: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut data = Vec::with_capacity(idx_arr.len());
        for &idx_f in idx_arr.data() {
            #[expect(clippy::cast_possible_truncation, reason = "index from f64")]
            let idx = idx_f as i64;
            let resolved = resolve_index(idx, self.data.len())?;
            data.push(self.data[resolved]);
        }
        let len = data.len();
        let result = Self::new(data, vec![len], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(result))?))
    }

    /// Indexes by a Python slice object (e.g. `arr[1:3]`, `arr[::2]`, `arr[::-1]`).
    pub fn getitem_slice(&self, slice: &Slice, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let len = self.data.len();
        let (start, stop, step) = slice.indices(len)?;

        let mut data = Vec::new();
        if step > 0 {
            let mut i = start;
            while i < stop {
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "positive-step slice indices are clamped to the array bounds"
                )]
                {
                    data.push(self.data[i as usize]);
                }
                i += step;
            }
        } else {
            let mut i = start;
            while i > stop {
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "negative-step slice indices visited here are in bounds"
                )]
                {
                    data.push(self.data[i as usize]);
                }
                i += step;
            }
        }

        let result_len = data.len();
        let result = Self::new(data, vec![result_len], self.dtype);
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
    pub fn add_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_right(scalar, |a, b| a + b, dtype, heap)
    }

    /// Element-wise subtraction.
    pub fn sub(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| a - b, heap)
    }

    /// Subtraction with a scalar value.
    pub fn sub_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_right(scalar, |a, b| a - b, dtype, heap)
    }

    /// Element-wise multiplication.
    pub fn mul(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, |a, b| a * b, heap)
    }

    /// Multiplication with a scalar value.
    pub fn mul_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
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
    pub fn floordiv_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_right(scalar, |a, b| (a / b).floor(), dtype, heap)
    }

    /// Element-wise modulo.
    pub fn modulo(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, py_mod, heap)
    }

    /// Modulo with a scalar value.
    pub fn modulo_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_right(scalar, py_mod, dtype, heap)
    }

    /// Element-wise power.
    pub fn pow(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.elementwise_op(other, f64::powf, heap)
    }

    /// Power with a scalar exponent.
    pub fn pow_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_right(scalar, f64::powf, dtype, heap)
    }

    /// Reverse subtraction with scalar: `scalar - array`.
    pub fn rsub_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_left(scalar, |a, b| a - b, dtype, heap)
    }

    /// Reverse division with scalar: `scalar / array`.
    pub fn rdiv_scalar(&self, scalar: f64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.scalar_op_left(scalar, |a, b| a / b, NdArrayDtype::Float64, heap)
    }

    /// Reverse floor division with scalar: `scalar // array`.
    pub fn rfloordiv_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_left(scalar, |a, b| (a / b).floor(), dtype, heap)
    }

    /// Reverse modulo with scalar: `scalar % array`.
    pub fn rmod_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_left(scalar, py_mod, dtype, heap)
    }

    /// Reverse power with scalar: `scalar ** array`.
    pub fn rpow_scalar(
        &self,
        scalar: f64,
        scalar_is_float: bool,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        let dtype = promote_dtype_with_scalar(self.dtype, scalar_is_float);
        self.scalar_op_left(scalar, f64::powf, dtype, heap)
    }

    /// Element-wise bitwise AND between two arrays.
    ///
    /// - **Bool arrays**: element-wise logical AND.
    /// - **Int arrays**: bitwise AND on each pair of elements cast to `i64`.
    /// - **Float arrays**: raises `TypeError`, matching NumPy's behavior.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitand(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "&")?;
        check_bitwise_dtype(other.dtype, "&")?;
        if self.shape != other.shape {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
            );
        }
        let result_dtype = if self.dtype == NdArrayDtype::Bool && other.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(other.data.iter())
            .map(|(&a, &b)| (a as i64 & b as i64) as f64)
            .collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Bitwise AND with a scalar value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitand_scalar(&self, scalar: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "&")?;
        let result_dtype = if self.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self.data.iter().map(|&a| (a as i64 & scalar) as f64).collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise bitwise OR between two arrays.
    ///
    /// - **Bool arrays**: element-wise logical OR.
    /// - **Int arrays**: bitwise OR on each pair of elements cast to `i64`.
    /// - **Float arrays**: raises `TypeError`, matching NumPy's behavior.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitor(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "|")?;
        check_bitwise_dtype(other.dtype, "|")?;
        if self.shape != other.shape {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
            );
        }
        let result_dtype = if self.dtype == NdArrayDtype::Bool && other.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(other.data.iter())
            .map(|(&a, &b)| (a as i64 | b as i64) as f64)
            .collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Bitwise OR with a scalar value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitor_scalar(&self, scalar: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "|")?;
        let result_dtype = if self.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self.data.iter().map(|&a| (a as i64 | scalar) as f64).collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Element-wise bitwise XOR between two arrays.
    ///
    /// - **Bool arrays**: element-wise logical XOR.
    /// - **Int arrays**: bitwise XOR on each pair of elements cast to `i64`.
    /// - **Float arrays**: raises `TypeError`, matching NumPy's behavior.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitxor(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "^")?;
        check_bitwise_dtype(other.dtype, "^")?;
        if self.shape != other.shape {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "operands could not be broadcast together").into(),
            );
        }
        let result_dtype = if self.dtype == NdArrayDtype::Bool && other.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(other.data.iter())
            .map(|(&a, &b)| (a as i64 ^ b as i64) as f64)
            .collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Bitwise XOR with a scalar value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn bitxor_scalar(&self, scalar: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        check_bitwise_dtype(self.dtype, "^")?;
        let result_dtype = if self.dtype == NdArrayDtype::Bool {
            NdArrayDtype::Bool
        } else {
            NdArrayDtype::Int64
        };
        let data: Vec<f64> = self.data.iter().map(|&a| (a as i64 ^ scalar) as f64).collect();
        let arr = Self::new(data, self.shape.clone(), result_dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Matrix multiplication (the `@` operator).
    ///
    /// - **1D @ 1D**: dot product, returns a scalar.
    /// - **2D @ 2D**: standard matrix multiplication, returns a 2D array.
    /// - **2D @ 1D**: matrix-vector product, returns a 1D array.
    /// - **1D @ 2D**: vector-matrix product, returns a 1D array.
    pub fn matmul(&self, other: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let result_dtype = promote_dtype(self.dtype, other.dtype);
        match (self.ndim(), other.ndim()) {
            (1, 1) => {
                // Dot product
                if self.data.len() != other.data.len() {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "matmul: Input operand 1 does not have enough dimensions",
                    )
                    .into());
                }
                let result: f64 = self.data.iter().zip(other.data.iter()).map(|(&a, &b)| a * b).sum();
                if result_dtype == NdArrayDtype::Int64 {
                    #[expect(clippy::cast_possible_truncation, reason = "intended int truncation")]
                    return Ok(Value::Int(result as i64));
                }
                Ok(Value::Float(result))
            }
            (2, 2) => {
                // Matrix multiplication
                let (m, k1) = (self.shape[0], self.shape[1]);
                let (k2, n) = (other.shape[0], other.shape[1]);
                if k1 != k2 {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        format!("matmul: Input operand 1 has a mismatch in its core dimension 0, (size {k1} is different from {k2})"),
                    )
                    .into());
                }
                let mut data = Vec::with_capacity(m * n);
                for i in 0..m {
                    for j in 0..n {
                        let mut sum = 0.0;
                        for p in 0..k1 {
                            sum += self.data[i * k1 + p] * other.data[p * n + j];
                        }
                        data.push(sum);
                    }
                }
                let arr = Self::new(data, vec![m, n], result_dtype);
                Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
            }
            (2, 1) => {
                // Matrix-vector product
                let (m, k1) = (self.shape[0], self.shape[1]);
                if k1 != other.data.len() {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "matmul: Input operand 1 has a mismatch in its core dimension 0",
                    )
                    .into());
                }
                let mut data = Vec::with_capacity(m);
                for i in 0..m {
                    let mut sum = 0.0;
                    for p in 0..k1 {
                        sum += self.data[i * k1 + p] * other.data[p];
                    }
                    data.push(sum);
                }
                let arr = Self::new(data, vec![m], result_dtype);
                Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
            }
            (1, 2) => {
                // Vector-matrix product
                let (k2, n) = (other.shape[0], other.shape[1]);
                if self.data.len() != k2 {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        "matmul: Input operand 1 has a mismatch in its core dimension 0",
                    )
                    .into());
                }
                let mut data = Vec::with_capacity(n);
                for j in 0..n {
                    let mut sum = 0.0;
                    for p in 0..k2 {
                        sum += self.data[p] * other.data[p * n + j];
                    }
                    data.push(sum);
                }
                let arr = Self::new(data, vec![n], result_dtype);
                Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
            }
            _ => Err(ExcType::type_error("matmul not supported for these array dimensions")),
        }
    }

    /// Unary negation.
    pub fn neg(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self.data.iter().map(|&a| -a).collect();
        let arr = Self::new(data, self.shape.clone(), self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// Unary bitwise invert (`~`).
    ///
    /// - **Bool arrays**: flips True↔False (returns Bool dtype).
    /// - **Int arrays**: bitwise NOT on each element cast to `i64` (returns Int64 dtype).
    /// - **Float arrays**: raises `TypeError`, matching NumPy's behavior.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f64→i64 truncation is intentional for int-typed ndarray elements"
    )]
    pub fn invert(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.dtype {
            NdArrayDtype::Bool => {
                let data: Vec<f64> = self.data.iter().map(|&a| if a == 0.0 { 1.0 } else { 0.0 }).collect();
                let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Bool);
                Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
            }
            NdArrayDtype::Int64 => {
                let data: Vec<f64> = self.data.iter().map(|&a| !(a as i64) as f64).collect();
                let arr = Self::new(data, self.shape.clone(), NdArrayDtype::Int64);
                Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
            }
            NdArrayDtype::Float64 => Err(SimpleException::new_msg(
                ExcType::TypeError,
                "ufunc 'invert' not supported for the input types",
            )
            .into()),
        }
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
    ///
    /// If any element is NaN, returns NaN — matching NumPy's propagation semantics.
    /// Uses a NaN-propagating reduction: once a NaN is seen, the result is NaN.
    pub fn min_val(&self) -> RunResult<f64> {
        self.data
            .iter()
            .copied()
            .reduce(|acc, v| {
                if acc.is_nan() || v.is_nan() {
                    f64::NAN
                } else {
                    acc.min(v)
                }
            })
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "zero-size array has no minimum").into())
    }

    /// Returns the maximum element.
    ///
    /// If any element is NaN, returns NaN — matching NumPy's propagation semantics.
    pub fn max_val(&self) -> RunResult<f64> {
        self.data
            .iter()
            .copied()
            .reduce(|acc, v| {
                if acc.is_nan() || v.is_nan() {
                    f64::NAN
                } else {
                    acc.max(v)
                }
            })
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
            .reduce(|(i_max, v_max), (i, v)| {
                if v.partial_cmp(v_max).unwrap_or(Ordering::Equal) == Ordering::Greater {
                    (i, v)
                } else {
                    (i_max, v_max)
                }
            })
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

    /// Returns the product of all elements.
    pub fn prod(&self) -> f64 {
        self.data.iter().copied().fold(1.0, |acc, v| acc * v)
    }

    /// Returns the population variance (ddof=0).
    pub fn var(&self) -> f64 {
        let mean = self.mean();
        self.data.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / self.len() as f64
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
        let new_size: usize = new_shape
            .iter()
            .try_fold(1usize, |acc, &d| acc.checked_mul(d))
            .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, "reshape dimensions overflow"))?;
        if new_size != self.len() {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!(
                    "cannot reshape array of size {} into shape ({})",
                    self.len(),
                    new_shape.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                ),
            )
            .into());
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

    /// `cumprod()` — returns a 1D array of cumulative products.
    ///
    /// Each element is the product of all preceding elements (inclusive).
    /// The result is always 1D, matching NumPy's behavior when no axis is specified.
    pub fn cumprod(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut acc = 1.0;
        let data: Vec<f64> = self
            .data
            .iter()
            .map(|&v| {
                acc *= v;
                acc
            })
            .collect();
        let arr = Self::new(data, vec![self.len()], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `item()` — return the single element of a size-1 array as a Python scalar.
    ///
    /// Raises `ValueError` if the array has more than one element, matching NumPy.
    pub fn item(&self) -> RunResult<Value> {
        if self.data.len() != 1 {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                "can only convert an array of size 1 to a Python scalar",
            )
            .into());
        }
        Ok(self.element_to_value(self.data[0]))
    }

    /// `squeeze()` — remove axes of length 1, returning a new array.
    ///
    /// If all axes have length 1, the result is a 1-element array with shape `(1,)`.
    pub fn squeeze(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let shape: Vec<usize> = self.shape.iter().copied().filter(|&s| s != 1).collect();
        let shape = if shape.is_empty() { vec![1] } else { shape };
        let arr = Self::new(self.data.clone(), shape, self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `take(indices)` — gather elements at the given integer indices.
    ///
    /// The indices array is flattened and used to index into the flattened source array.
    /// Negative indices are supported.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "index from f64 is intentional for int-typed ndarray elements"
    )]
    pub fn take_indices(&self, indices: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut data = Vec::with_capacity(indices.len());
        for &idx_f in indices.data() {
            let idx = idx_f as i64;
            let resolved = resolve_index(idx, self.data.len())?;
            data.push(self.data[resolved]);
        }
        let len = data.len();
        let arr = Self::new(data, vec![len], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `diagonal()` — return the diagonal of a 2D array.
    ///
    /// Raises `ValueError` for arrays with fewer than 2 dimensions.
    pub fn diagonal(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if self.shape.len() < 2 {
            return Err(SimpleException::new_msg(ExcType::ValueError, "diagonal requires 2-d array").into());
        }
        let rows = self.shape[0];
        let cols = self.shape[1];
        let n = rows.min(cols);
        let data: Vec<f64> = (0..n).map(|i| self.data[i * cols + i]).collect();
        let arr = Self::new(data, vec![n], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `trace()` — return the sum of the diagonal elements of a 2D array.
    ///
    /// Returns an int for int arrays and a float for float arrays, matching NumPy.
    /// Raises `ValueError` for arrays with fewer than 2 dimensions.
    pub fn trace(&self) -> RunResult<Value> {
        if self.shape.len() < 2 {
            return Err(SimpleException::new_msg(ExcType::ValueError, "trace requires 2-d array").into());
        }
        let cols = self.shape[1];
        let n = self.shape[0].min(cols);
        let sum: f64 = (0..n).map(|i| self.data[i * cols + i]).sum();
        Ok(self.element_to_value(sum))
    }

    /// `fill(value)` — fill the array in-place with the given scalar value.
    pub fn fill(&mut self, value: f64) {
        self.data.fill(value);
    }

    /// `compress(condition)` — return elements where the boolean condition array is true.
    ///
    /// Operates on the flattened array. The condition array's truthy elements select
    /// corresponding elements from the source.
    pub fn compress(&self, condition: &Self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(condition.data.iter())
            .filter(|pair| *pair.1 != 0.0)
            .map(|pair| *pair.0)
            .collect();
        let len = data.len();
        let arr = Self::new(data, vec![len], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `swapaxes(a, b)` — swap two axes of the array.
    ///
    /// For 2D arrays with axes 0 and 1, this is equivalent to a transpose.
    /// For 1D arrays (or swapping an axis with itself), returns a copy.
    pub fn swapaxes(&self, axis_a: usize, axis_b: usize, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if axis_a >= self.ndim() || axis_b >= self.ndim() {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("bad axis for array with {} dimensions", self.ndim()),
            )
            .into());
        }
        if axis_a == axis_b || self.ndim() <= 1 {
            // No-op: return a copy
            let arr = Self::new(self.data.clone(), self.shape.clone(), self.dtype);
            return Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?));
        }
        // For 2D with axes (0, 1), this is a transpose
        if self.ndim() == 2 {
            self.transpose(heap)
        } else {
            Err(ExcType::type_error("swapaxes not supported for arrays with ndim > 2"))
        }
    }

    /// `repeat(n)` — repeat each element `n` times, returning a 1D array.
    pub fn repeat_array(&self, n: usize, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut data = Vec::with_capacity(self.data.len() * n);
        for &v in &self.data {
            for _ in 0..n {
                data.push(v);
            }
        }
        check_array_alloc_size(data.len(), heap.tracker())?;
        let len = data.len();
        let arr = Self::new(data, vec![len], self.dtype);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `nonzero()` — returns a tuple of arrays, one per dimension, containing non-zero indices.
    ///
    /// For 1D arrays, returns a 1-element tuple of an index array.
    /// For 2D arrays, returns a 2-element tuple of row and column index arrays.
    pub fn nonzero_method(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        if self.ndim() <= 1 {
            let indices: Vec<f64> = self
                .data
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0.0)
                .map(|(i, _)| i as f64)
                .collect();
            let len = indices.len();
            let arr = Self::new(indices, vec![len], NdArrayDtype::Int64);
            let arr_val = Value::Ref(heap.allocate(HeapData::NdArray(arr))?);
            let tup = allocate_tuple(smallvec![arr_val], heap)?;
            Ok(tup)
        } else if self.ndim() == 2 {
            let rows = self.shape[0];
            let cols = self.shape[1];
            let mut row_indices = Vec::new();
            let mut col_indices = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    if self.data[r * cols + c] != 0.0 {
                        row_indices.push(r as f64);
                        col_indices.push(c as f64);
                    }
                }
            }
            let row_len = row_indices.len();
            let col_len = col_indices.len();
            let row_arr = Self::new(row_indices, vec![row_len], NdArrayDtype::Int64);
            let col_arr = Self::new(col_indices, vec![col_len], NdArrayDtype::Int64);
            let row_val = Value::Ref(heap.allocate(HeapData::NdArray(row_arr))?);
            let col_val = Value::Ref(heap.allocate(HeapData::NdArray(col_arr))?);
            let tup = allocate_tuple(smallvec![row_val, col_val], heap)?;
            Ok(tup)
        } else {
            Err(ExcType::type_error("nonzero() not supported for arrays with ndim > 2"))
        }
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

    /// `sort()` — sort the array data in-place (mutates `self`).
    ///
    /// NaN values sort to the end, matching NumPy's `ndarray.sort()` behavior.
    pub fn sort_in_place(&mut self) {
        self.data.sort_by(nan_last_cmp);
    }

    /// `argsort()` — returns indices that would sort the array.
    ///
    /// NaN values sort to the end, matching NumPy's behavior.
    pub fn argsort(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut indices: Vec<usize> = (0..self.data.len()).collect();
        indices.sort_by(|&a, &b| nan_last_cmp(&self.data[a], &self.data[b]));
        let data: Vec<f64> = indices.iter().map(|&i| i as f64).collect();
        let arr = Self::new(data, vec![self.data.len()], NdArrayDtype::Int64);
        Ok(Value::Ref(heap.allocate(HeapData::NdArray(arr))?))
    }

    /// `astype(dtype_str)` — cast array to a new dtype.
    ///
    /// Accepts NumPy dtype aliases that Monty maps onto its compact dtype set.
    pub fn astype(&self, dtype_str: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let new_dtype = match dtype_str {
            "int64" | "int" | "int32" => NdArrayDtype::Int64,
            "float64" | "float" | "float32" => NdArrayDtype::Float64,
            "bool" | "bool_" => NdArrayDtype::Bool,
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

    /// Converts the array to a (possibly nested) Python list.
    ///
    /// For 1D arrays, returns a flat list. For 2D+ arrays, returns nested lists
    /// matching the shape, e.g. shape `(2, 3)` → `[[1, 2, 3], [4, 5, 6]]`.
    pub fn tolist(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        self.tolist_recursive(&self.shape, 0, heap)
    }

    /// Recursively builds nested lists for `tolist()`.
    fn tolist_recursive(
        &self,
        remaining_shape: &[usize],
        offset: usize,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<Value> {
        if remaining_shape.len() == 1 {
            // Leaf dimension: flat list of scalars
            let len = remaining_shape[0];
            let values: Vec<Value> = (0..len).map(|i| self.element_to_value(self.data[offset + i])).collect();
            let list = List::new(values);
            Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
        } else {
            // Build nested list: each element is a sub-list
            let sub_size: usize = remaining_shape[1..].iter().product();
            let mut values = Vec::with_capacity(remaining_shape[0]);
            for i in 0..remaining_shape[0] {
                values.push(self.tolist_recursive(&remaining_shape[1..], offset + i * sub_size, heap)?);
            }
            let list = List::new(values);
            Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
        }
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
        // NumPy includes dtype suffix for empty arrays since element format can't convey it
        if self.data.is_empty() {
            write!(f, ", dtype={}", self.dtype)?;
        }
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
                        if val.is_nan() {
                            f.write_str("nan")?;
                        } else if val.is_infinite() {
                            if val.is_sign_negative() {
                                f.write_str("-inf")?;
                            } else {
                                f.write_str("inf")?;
                            }
                        } else if val.fract() == 0.0 {
                            // NumPy displays whole floats as "1." not "1.0"
                            write!(f, "{val:.0}.")?;
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
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::NdArray
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        // NumPy's len() returns the size of the first dimension, not total elements.
        let arr = self.get(vm.heap);
        Some(arr.shape().first().copied().unwrap_or(0))
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        Ok(a.shape == b.shape && a.data == b.data && a.dtype == b.dtype)
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
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
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        Ok(self.get(vm.heap).py_repr_fmt_inner(f)?)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let arr = self.get(vm.heap);
        match key {
            Value::Int(n) => arr.getitem_int(*n, vm.heap),
            Value::Bool(b) => arr.getitem_int(i64::from(*b), vm.heap),
            Value::Ref(key_id) => {
                match vm.heap.get(*key_id) {
                    HeapData::NdArray(mask_or_idx) => {
                        if mask_or_idx.dtype() == NdArrayDtype::Bool {
                            arr.getitem_bool_mask(mask_or_idx, vm.heap)
                        } else {
                            // Integer array indexing (fancy indexing)
                            arr.getitem_int_array(mask_or_idx, vm.heap)
                        }
                    }
                    HeapData::Slice(slice) => arr.getitem_slice(slice, vm.heap),
                    _ => Err(ExcType::type_error(
                        "ndarray indices must be integers, slices, or boolean/integer arrays",
                    )),
                }
            }
            _ => Err(ExcType::type_error(
                "ndarray indices must be integers, slices, or boolean/integer arrays",
            )),
        }
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop!(value, vm);

        match *key {
            // arr[int] = val — set a single element by integer index
            Value::Int(idx) => {
                let scalar = extract_f64(value);
                let arr = self.get_mut(vm.heap);
                if arr.ndim() != 1 {
                    return Err(ExcType::type_error("only 1D array integer assignment is supported"));
                }
                let resolved = resolve_index(idx, arr.shape[0])?;
                arr.data[resolved] = scalar;
                Ok(())
            }
            Value::Bool(b) => {
                let scalar = extract_f64(value);
                let arr = self.get_mut(vm.heap);
                if arr.ndim() != 1 {
                    return Err(ExcType::type_error("only 1D array integer assignment is supported"));
                }
                let resolved = resolve_index(i64::from(b), arr.shape[0])?;
                arr.data[resolved] = scalar;
                Ok(())
            }
            Value::Ref(key_id) => {
                match vm.heap.get(key_id) {
                    // arr[bool_mask] = val — set elements where mask is True
                    HeapData::NdArray(mask) if mask.dtype() == NdArrayDtype::Bool => {
                        let mask_data: Vec<bool> = mask.data().iter().map(|&v| v != 0.0).collect();
                        let scalar = extract_f64(value);
                        let arr = self.get_mut(vm.heap);
                        if mask_data.len() != arr.data.len() {
                            return Err(SimpleException::new_msg(
                                ExcType::IndexError,
                                "boolean index did not match indexed array",
                            )
                            .into());
                        }
                        for (i, &m) in mask_data.iter().enumerate() {
                            if m {
                                arr.data[i] = scalar;
                            }
                        }
                        Ok(())
                    }
                    // arr[slice] = val — set slice of elements (scalar or array)
                    HeapData::Slice(slice) => {
                        // Extract RHS values: scalar broadcasts, array assigns element-wise
                        let rhs_data: Option<Vec<f64>> = match *value {
                            Value::Ref(val_id) => match vm.heap.get(val_id) {
                                HeapData::NdArray(rhs_arr) => Some(rhs_arr.data().to_vec()),
                                _ => None,
                            },
                            _ => None,
                        };
                        let scalar = if rhs_data.is_none() { extract_f64(value) } else { 0.0 };
                        let len = self.get(vm.heap).data.len();
                        let (start, stop, step) = slice.indices(len)?;
                        let arr = self.get_mut(vm.heap);
                        if step > 0 {
                            let mut i = start;
                            let mut rhs_idx = 0usize;
                            while i < stop {
                                #[expect(
                                    clippy::cast_sign_loss,
                                    clippy::cast_possible_truncation,
                                    reason = "positive-step slice indices are clamped to the array bounds"
                                )]
                                {
                                    arr.data[i as usize] = if let Some(ref rhs) = rhs_data {
                                        rhs.get(rhs_idx).copied().unwrap_or(scalar)
                                    } else {
                                        scalar
                                    };
                                }
                                rhs_idx += 1;
                                i += step;
                            }
                        } else {
                            let mut i = start;
                            let mut rhs_idx = 0usize;
                            while i > stop {
                                #[expect(
                                    clippy::cast_sign_loss,
                                    clippy::cast_possible_truncation,
                                    reason = "negative-step slice indices visited here are in bounds"
                                )]
                                {
                                    arr.data[i as usize] = if let Some(ref rhs) = rhs_data {
                                        rhs.get(rhs_idx).copied().unwrap_or(scalar)
                                    } else {
                                        scalar
                                    };
                                }
                                rhs_idx += 1;
                                i += step;
                            }
                        }
                        Ok(())
                    }
                    _ => Err(ExcType::type_error(
                        "ndarray indices must be integers, slices, or boolean arrays",
                    )),
                }
            }
            _ => Err(ExcType::type_error(
                "ndarray indices must be integers, slices, or boolean arrays",
            )),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let arr = self.get(vm.heap);
        let result = match attr.static_string() {
            Some(StaticStrings::NpShape) => arr.shape_tuple(vm.heap)?,
            Some(StaticStrings::Dtype) => arr.dtype_str(vm.heap)?,
            #[expect(clippy::cast_possible_wrap, reason = "array length won't exceed i64::MAX")]
            Some(StaticStrings::NpSize) => Value::Int(arr.len() as i64),
            #[expect(clippy::cast_possible_wrap, reason = "ndim is always small")]
            Some(StaticStrings::NpNdim) => Value::Int(arr.ndim() as i64),
            #[expect(clippy::cast_possible_wrap, reason = "nbytes won't exceed i64::MAX")]
            Some(StaticStrings::NpNbytes) => Value::Int((arr.len() * 8) as i64),
            Some(StaticStrings::NpItemsize) => Value::Int(8),
            Some(StaticStrings::NpFlat) => {
                let flat = NdArray::new(arr.data.clone(), vec![arr.data.len()], arr.dtype);
                Value::Ref(vm.heap.allocate(HeapData::NdArray(flat))?)
            }
            Some(StaticStrings::NpT) => arr.transpose(vm.heap)?,
            _ => {
                // "T" is a single ASCII character so it is interned as an ASCII StringId,
                // not as a StaticStrings variant — handle it in the fallback arm.
                if attr.as_str(vm.interns) == "T" {
                    arr.transpose(vm.heap)?
                } else {
                    return Err(ExcType::attribute_error(Type::NdArray, attr.as_str(vm.interns)));
                }
            }
        };
        Ok(Some(CallResult::Value(result)))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
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
                self.get_mut(vm.heap).sort_in_place();
                Ok(Value::None)
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
            Some(StaticStrings::NpProd) => {
                args.check_zero_args("ndarray.prod", vm.heap)?;
                Ok(call_prod_method(self.get(vm.heap)))
            }
            Some(StaticStrings::NpVar) => {
                args.check_zero_args("ndarray.var", vm.heap)?;
                Ok(Value::Float(self.get(vm.heap).var()))
            }
            Some(StaticStrings::NpRavel) => {
                args.check_zero_args("ndarray.ravel", vm.heap)?;
                self.get(vm.heap).flatten(vm.heap)
            }
            Some(StaticStrings::NpItem) => {
                args.check_zero_args("ndarray.item", vm.heap)?;
                self.get(vm.heap).item()
            }
            Some(StaticStrings::NpCumprod) => {
                args.check_zero_args("ndarray.cumprod", vm.heap)?;
                self.get(vm.heap).cumprod(vm.heap)
            }
            Some(StaticStrings::NpSqueeze) => {
                args.check_zero_args("ndarray.squeeze", vm.heap)?;
                self.get(vm.heap).squeeze(vm.heap)
            }
            Some(StaticStrings::NpTake) => {
                let idx_val = args.get_one_arg("ndarray.take", vm.heap)?;
                let result = match &idx_val {
                    Value::Ref(other_id) => {
                        if let HeapData::NdArray(other) = vm.heap.get(*other_id) {
                            self.get(vm.heap).take_indices(other, vm.heap)
                        } else {
                            Err(ExcType::type_error("take() requires an ndarray of indices"))
                        }
                    }
                    _ => Err(ExcType::type_error("take() requires an ndarray of indices")),
                };
                idx_val.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::NpDiagonal) => {
                args.check_zero_args("ndarray.diagonal", vm.heap)?;
                self.get(vm.heap).diagonal(vm.heap)
            }
            Some(StaticStrings::NpTrace) => {
                args.check_zero_args("ndarray.trace", vm.heap)?;
                self.get(vm.heap).trace()
            }
            Some(StaticStrings::NpFill) => {
                let arg = args.get_one_arg("ndarray.fill", vm.heap)?;
                let val = extract_f64(&arg);
                arg.drop_with_heap(vm);
                self.get_mut(vm.heap).fill(val);
                Ok(Value::None)
            }
            Some(StaticStrings::NpCompress) => {
                let cond_val = args.get_one_arg("ndarray.compress", vm.heap)?;
                let result = match &cond_val {
                    Value::Ref(other_id) => {
                        if let HeapData::NdArray(cond) = vm.heap.get(*other_id) {
                            self.get(vm.heap).compress(cond, vm.heap)
                        } else {
                            Err(ExcType::type_error("compress() requires a boolean ndarray condition"))
                        }
                    }
                    _ => Err(ExcType::type_error("compress() requires a boolean ndarray condition")),
                };
                cond_val.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::NpRepeat) => {
                let arg = args.get_one_arg("ndarray.repeat", vm.heap)?;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "repeat count from user"
                )]
                let n = extract_f64(&arg) as usize;
                arg.drop_with_heap(vm);
                self.get(vm.heap).repeat_array(n, vm.heap)
            }
            Some(StaticStrings::NpNonzero) => {
                args.check_zero_args("ndarray.nonzero", vm.heap)?;
                self.get(vm.heap).nonzero_method(vm.heap)
            }
            Some(StaticStrings::NpSwapaxes) => {
                let pos = args.into_pos_only("ndarray.swapaxes", vm.heap)?;
                let result = if pos.as_slice().len() >= 2 {
                    let a = extract_f64(&pos.as_slice()[0]);
                    let b = extract_f64(&pos.as_slice()[1]);
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "axis from user")]
                    self.get(vm.heap).swapaxes(a as usize, b as usize, vm.heap)
                } else {
                    Err(ExcType::type_error("swapaxes() requires two arguments"))
                };
                pos.drop_with_heap(vm);
                result
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
        size_of::<Self>() + self.data.capacity() * size_of::<f64>() + self.shape.capacity() * size_of::<usize>()
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

/// Returns `prod()` with dtype-appropriate return type.
fn call_prod_method(arr: &NdArray) -> Value {
    let p = arr.prod();
    match arr.dtype() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f64 to i64 truncation is intended for int prod"
        )]
        NdArrayDtype::Int64 => Value::Int(p as i64),
        NdArrayDtype::Float64 | NdArrayDtype::Bool => Value::Float(p),
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

/// Comparison function that sorts NaN values to the end, matching NumPy's sort behavior.
///
/// Non-NaN values are compared normally. NaN is treated as greater than any non-NaN value,
/// and two NaN values are considered equal.
///
/// Takes `&f64` so it can be passed directly to `[f64]::sort_by`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature required by sort_by(Fn(&T, &T) -> Ordering)"
)]
pub(crate) fn nan_last_cmp(a: &f64, b: &f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
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
    let mut has_int = false;
    let mut has_bool = false;
    collect_from_value(
        value,
        heap,
        &mut data,
        &mut shape,
        0,
        &mut has_float,
        &mut has_int,
        &mut has_bool,
    )?;

    let dtype = if has_float {
        NdArrayDtype::Float64
    } else if has_int {
        NdArrayDtype::Int64
    } else if has_bool {
        NdArrayDtype::Bool
    } else {
        // Empty array defaults to float64, matching NumPy's behavior
        NdArrayDtype::Float64
    };

    Ok(NdArray::new(data, shape, dtype))
}

/// Recursively collects numeric data from a nested list/value structure.
///
/// Tracks which scalar types are present (`has_float`, `has_int`, `has_bool`) so the
/// caller can determine the correct dtype: float > int > bool, matching NumPy's
/// type promotion rules.
#[expect(clippy::too_many_arguments)]
fn collect_from_value(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
    data: &mut Vec<f64>,
    shape: &mut Vec<usize>,
    depth: usize,
    has_float: &mut bool,
    has_int: &mut bool,
    has_bool: &mut bool,
) -> RunResult<()> {
    match value {
        Value::Int(n) => {
            *has_int = true;
            data.push(*n as f64);
            Ok(())
        }
        Value::Float(f) => {
            *has_float = true;
            data.push(*f);
            Ok(())
        }
        Value::Bool(b) => {
            *has_bool = true;
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
                    collect_from_value(item, heap, data, shape, depth + 1, has_float, has_int, has_bool)?;
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
///
/// `scalar_is_float` indicates whether the Python value was a `float` (as opposed to `int`).
/// This is necessary because `1.0` and `1` are both `f64` internally, but NumPy promotes
/// `int_arr * 1.0` to float64 while `int_arr * 1` stays int64.
pub(crate) fn promote_dtype_with_scalar(arr_dtype: NdArrayDtype, scalar_is_float: bool) -> NdArrayDtype {
    if arr_dtype == NdArrayDtype::Float64 || scalar_is_float {
        NdArrayDtype::Float64
    } else {
        arr_dtype
    }
}

/// Validates that the dtype supports bitwise operations.
///
/// NumPy raises `TypeError` for bitwise ops on float arrays. Bool and Int64 are supported.
fn check_bitwise_dtype(dtype: NdArrayDtype, op_symbol: &str) -> RunResult<()> {
    if dtype == NdArrayDtype::Float64 {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("ufunc 'bitwise_{op_symbol}' not supported for the input types"),
        )
        .into());
    }
    Ok(())
}

/// Python-compatible modulo: result has the same sign as the divisor.
fn py_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r > 0.0) != (b > 0.0)) { r + b } else { r }
}
