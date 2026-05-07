//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{Heap, HeapData, HeapGuard, HeapReadOutput},
    resource::ResourceTracker,
    types::{NdArray, PyTrait, Set, dict_view::collect_iterable_to_set, ndarray::NdArrayDtype, set::SetBinaryOp},
    value::{BitwiseOp, Value},
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Binary addition with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths to avoid
    /// overhead on the success path (99%+ of operations).
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path: intercept before general dispatch
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Add, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_add(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("+", lhs_type, rhs_type))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Binary subtraction with proper refcount handling.
    ///
    /// Handles both numeric subtraction and set difference (`-` operator).
    /// For sets/frozensets, delegates to [`binary_set_op`] which needs `interns`
    /// for element hashing and equality. Uses lazy type capture: only calls
    /// `py_type()` in error paths.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Sub, this)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_dict_view_op(lhs, rhs, DictViewBinaryOp::Sub)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_set_op(lhs, rhs, SetBinaryOp::Sub)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_sub(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("-", lhs_type, rhs_type))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Binary multiplication with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mult(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Mul, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_mult(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("*", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_div(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Div, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_div(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("/", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary floor division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_floordiv(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::FloorDiv, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_floordiv(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("//", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary modulo with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mod(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Mod, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_mod(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("%", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary power with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    #[inline(never)]
    pub(super) fn binary_pow(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_binary(lhs, rhs, NdArrayBinaryOp::Pow, this)? {
            this.push(result);
            return Ok(());
        }

        match lhs.py_pow(rhs, this) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this);
                let rhs_type = rhs.py_type(this);
                Err(ExcType::binary_type_error("** or pow()", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary bitwise operation on integers and sets.
    ///
    /// For integers, performs standard bitwise operations (AND, OR, XOR, shifts).
    /// For sets/frozensets, `|` maps to union, `&` to intersection, and `^` to
    /// symmetric difference. Set operations are handled here because `py_bitwise`
    /// doesn't have access to `interns`, which set operations need for hashing.
    pub(super) fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // Set/frozenset operations: |, &, ^ map to union, intersection,
        // symmetric_difference. Shifts don't apply to sets.
        let set_op = match op {
            BitwiseOp::Or => Some(SetBinaryOp::Or),
            BitwiseOp::And => Some(SetBinaryOp::And),
            BitwiseOp::Xor => Some(SetBinaryOp::Xor),
            BitwiseOp::LShift | BitwiseOp::RShift => None,
        };
        if let Some(set_op) = set_op
            && let Some(result) = this.binary_set_op(lhs, rhs, set_op)?
        {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_bitwise(rhs, op, this)?;
        this.push(result);
        Ok(())
    }

    /// Binary `&` with CPython-style dict-keys special handling before numeric fallback.
    ///
    /// Milestone one only needs one non-numeric behavior here: `dict_keys & iterable`
    /// should iterate the right-hand side, return a plain `set`, and raise
    /// `TypeError("'X' object is not iterable")` for non-iterable operands.
    pub(super) fn binary_and(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_bitwise(lhs, rhs, NdArrayBitwiseOp::And, this)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_dict_view_op(lhs, rhs, DictViewBinaryOp::And)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_set_op(lhs, rhs, SetBinaryOp::And)? {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_bitwise(rhs, BitwiseOp::And, this)?;
        this.push(result);
        Ok(())
    }

    /// Binary `|` with CPython-style dict-view handling before numeric fallback.
    pub(super) fn binary_or(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_bitwise(lhs, rhs, NdArrayBitwiseOp::Or, this)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_dict_view_op(lhs, rhs, DictViewBinaryOp::Or)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_set_op(lhs, rhs, SetBinaryOp::Or)? {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_bitwise(rhs, BitwiseOp::Or, this)?;
        this.push(result);
        Ok(())
    }

    /// Binary `^` with CPython-style dict-view handling before numeric fallback.
    pub(super) fn binary_xor(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path
        if let Some(result) = try_ndarray_bitwise(lhs, rhs, NdArrayBitwiseOp::Xor, this)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_dict_view_op(lhs, rhs, DictViewBinaryOp::Xor)? {
            this.push(result);
            return Ok(());
        }

        if let Some(result) = this.binary_set_op(lhs, rhs, SetBinaryOp::Xor)? {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_bitwise(rhs, BitwiseOp::Xor, this)?;
        this.push(result);
        Ok(())
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// For mutable types like lists, `py_iadd` mutates in place and returns true.
    /// For ndarray, modifies data in place and returns the same array reference.
    /// For immutable types, we fall back to regular addition.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    ///
    /// Note: Cannot use `defer_drop!` for `lhs` here because on successful in-place
    /// operation, we need to push `lhs` back onto the stack rather than drop it.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
        // NdArray in-place fast path — try before popping so we can fall through
        if try_ndarray_inplace(self, NdArrayInplaceOp::Add) {
            return Ok(());
        }

        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        // Use HeapGuard because inplace addition will push lhs back on the stack if successful
        let mut lhs_guard = HeapGuard::new(this.pop(), this);
        let (lhs, this) = lhs_guard.as_parts_mut();

        // Try in-place operation first (for mutable types like lists)
        if lhs.py_iadd(rhs, this, lhs.ref_id())? {
            // In-place operation succeeded - push lhs back
            let (lhs, this) = lhs_guard.into_parts();
            this.push(lhs);
            return Ok(());
        }

        // Next try regular addition
        if let Some(v) = lhs.py_add(rhs, this)? {
            this.push(v);
            return Ok(());
        }

        let lhs_type = lhs.py_type(this);
        let rhs_type = rhs.py_type(this);
        Err(ExcType::binary_type_error("+=", lhs_type, rhs_type))
    }

    /// Binary matrix multiplication (`@` operator).
    ///
    /// Dispatches to `NdArray::matmul` for ndarray operands:
    /// - **1D @ 1D**: dot product (scalar result)
    /// - **2D @ 2D**: matrix multiplication
    /// - **2D @ 1D** / **1D @ 2D**: matrix-vector / vector-matrix product
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // Both operands must be NdArray
        let (Some(Value::Ref(lid)), Some(Value::Ref(rid))) = (Some(lhs), Some(rhs)) else {
            return Err(ExcType::type_error("matmul requires ndarray operands"));
        };

        let HeapData::NdArray(l) = this.heap.get(*lid) else {
            return Err(ExcType::type_error("matmul requires ndarray operands"));
        };
        let HeapData::NdArray(r) = this.heap.get(*rid) else {
            return Err(ExcType::type_error("matmul requires ndarray operands"));
        };

        let result = l.matmul(r, this.heap)?;
        this.push(result);
        Ok(())
    }

    /// In-place subtraction for ndarray. Falls back to binary subtraction for other types.
    pub(super) fn inplace_sub(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::Sub) {
            return Ok(());
        }
        self.binary_sub()
    }

    /// In-place multiplication for ndarray. Falls back to binary multiplication for other types.
    pub(super) fn inplace_mul(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::Mul) {
            return Ok(());
        }
        self.binary_mult()
    }

    /// In-place division for ndarray. Falls back to binary division for other types.
    pub(super) fn inplace_div(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::Div) {
            return Ok(());
        }
        self.binary_div()
    }

    /// In-place floor division for ndarray. Falls back to binary floor division for other types.
    pub(super) fn inplace_floordiv(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::FloorDiv) {
            return Ok(());
        }
        self.binary_floordiv()
    }

    /// In-place modulo for ndarray. Falls back to binary modulo for other types.
    pub(super) fn inplace_mod(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::Mod) {
            return Ok(());
        }
        self.binary_mod()
    }

    /// In-place power for ndarray. Falls back to binary power for other types.
    pub(super) fn inplace_pow(&mut self) -> Result<(), RunError> {
        if try_ndarray_inplace(self, NdArrayInplaceOp::Pow) {
            return Ok(());
        }
        self.binary_pow()
    }

    /// Implements dict-view set-like operators before falling back to other dispatch.
    ///
    /// Returning `Ok(None)` means the left operand was not a set-like dict view, so the
    /// caller should continue with ordinary numeric or pure-set dispatch.
    fn binary_dict_view_op(
        &mut self,
        lhs: &Value,
        rhs: &Value,
        op: DictViewBinaryOp,
    ) -> Result<Option<Value>, RunError> {
        let this = self;
        let Value::Ref(lhs_id) = lhs else {
            return Ok(None);
        };

        let lhs_set = match this.heap.read(*lhs_id) {
            HeapReadOutput::DictKeysView(view) => view.to_set(this)?,
            HeapReadOutput::DictItemsView(view) => view.to_set(this)?,
            _ => return Ok(None),
        };
        defer_drop!(lhs_set, this);

        let rhs_set = collect_iterable_to_set(rhs.clone_with_heap(this), this)?;
        defer_drop!(rhs_set, this);

        let result = apply_dict_view_binary_op(lhs_set, rhs_set, op, this)?;

        let result_id = this.heap.allocate(HeapData::Set(result))?;
        Ok(Some(Value::Ref(result_id)))
    }

    /// Implements pure set/frozenset binary operators with strict operand checks.
    ///
    /// Method forms accept arbitrary iterables, but the operator forms handled here
    /// must reject non-set operands so Monty matches CPython's `TypeError` behavior.
    fn binary_set_op(&mut self, lhs: &Value, rhs: &Value, op: SetBinaryOp) -> Result<Option<Value>, RunError> {
        let this = self;
        let Value::Ref(lhs_id) = lhs else {
            return Ok(None);
        };

        let output = this.heap.read(*lhs_id);
        let result = match output {
            HeapReadOutput::Set(set) => set.binary_op_value(rhs, op, this)?.map(HeapData::Set),
            HeapReadOutput::FrozenSet(fset) => fset.binary_op_value(rhs, op, this)?.map(HeapData::FrozenSet),
            _ => None,
        };

        let Some(result) = result else {
            return Ok(None);
        };
        let result_id = this.heap.allocate(result)?;
        Ok(Some(Value::Ref(result_id)))
    }
}

/// Supported dict-view set-like operators.
#[derive(Debug, Clone, Copy)]
enum DictViewBinaryOp {
    And,
    Or,
    Xor,
    Sub,
}

/// Applies a set-like operator to two temporary sets and returns a plain `set`.
fn apply_dict_view_binary_op(
    lhs: &Set,
    rhs: &Set,
    op: DictViewBinaryOp,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Result<Set, RunError> {
    let mut result = match op {
        DictViewBinaryOp::And => Set::with_capacity(lhs.len().min(rhs.len())),
        DictViewBinaryOp::Or => Set::with_capacity(lhs.len() + rhs.len()),
        DictViewBinaryOp::Xor => Set::with_capacity(lhs.len() + rhs.len()),
        DictViewBinaryOp::Sub => Set::with_capacity(lhs.len()),
    };

    match op {
        DictViewBinaryOp::And => {
            let (smaller, larger) = if lhs.len() <= rhs.len() { (lhs, rhs) } else { (rhs, lhs) };
            for value in smaller.iter() {
                if vm.heap.protect(larger).contains(value, vm)? {
                    result.add(value.clone_with_heap(vm), vm)?;
                }
            }
        }
        DictViewBinaryOp::Or => {
            for value in lhs.iter() {
                result.add(value.clone_with_heap(vm), vm)?;
            }
            for value in rhs.iter() {
                result.add(value.clone_with_heap(vm), vm)?;
            }
        }
        DictViewBinaryOp::Xor => {
            for value in lhs.iter() {
                if !vm.heap.protect(rhs).contains(value, vm)? {
                    result.add(value.clone_with_heap(vm), vm)?;
                }
            }
            for value in rhs.iter() {
                if !vm.heap.protect(lhs).contains(value, vm)? {
                    result.add(value.clone_with_heap(vm), vm)?;
                }
            }
        }
        DictViewBinaryOp::Sub => {
            for value in lhs.iter() {
                if !vm.heap.protect(rhs).contains(value, vm)? {
                    result.add(value.clone_with_heap(vm), vm)?;
                }
            }
        }
    }

    Ok(result)
}

/// Supported ndarray element-wise binary operations.
#[derive(Debug, Clone, Copy)]
enum NdArrayBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

/// Extracts a scalar f64 from a `Value`, if it is a numeric type.
///
/// Returns `(f64_value, is_float)` — the `is_float` flag indicates whether the Python
/// value was a `float` (as opposed to `int` or `bool`), which is needed for correct
/// dtype promotion in ndarray operations.
fn value_to_f64(v: &Value) -> Option<(f64, bool)> {
    match v {
        Value::Int(i) => Some((*i as f64, false)),
        Value::Float(f) => Some((*f, true)),
        Value::Bool(b) => Some((if *b { 1.0 } else { 0.0 }, false)),
        _ => None,
    }
}

/// Dispatches an element-wise binary operation between an `NdArray` and a scalar.
fn ndarray_scalar_op(
    arr: &NdArray,
    scalar: f64,
    scalar_is_float: bool,
    op: NdArrayBinaryOp,
    scalar_on_left: bool,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    match (op, scalar_on_left) {
        // Commutative operations — direction doesn't matter
        (NdArrayBinaryOp::Add, _) => arr.add_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Mul, _) => arr.mul_scalar(scalar, scalar_is_float, heap),
        // Non-commutative: scalar on right (arr op scalar)
        (NdArrayBinaryOp::Sub, false) => arr.sub_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Div, false) => arr.div_scalar(scalar, heap),
        (NdArrayBinaryOp::FloorDiv, false) => arr.floordiv_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Mod, false) => arr.modulo_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Pow, false) => arr.pow_scalar(scalar, scalar_is_float, heap),
        // Non-commutative: scalar on left (scalar op arr)
        (NdArrayBinaryOp::Sub, true) => arr.rsub_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Div, true) => arr.rdiv_scalar(scalar, heap),
        (NdArrayBinaryOp::FloorDiv, true) => arr.rfloordiv_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Mod, true) => arr.rmod_scalar(scalar, scalar_is_float, heap),
        (NdArrayBinaryOp::Pow, true) => arr.rpow_scalar(scalar, scalar_is_float, heap),
    }
}

/// Dispatches an element-wise binary operation between two `NdArray`s.
fn ndarray_array_op(
    lhs: &NdArray,
    rhs: &NdArray,
    op: NdArrayBinaryOp,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    match op {
        NdArrayBinaryOp::Add => lhs.add(rhs, heap),
        NdArrayBinaryOp::Sub => lhs.sub(rhs, heap),
        NdArrayBinaryOp::Mul => lhs.mul(rhs, heap),
        NdArrayBinaryOp::Div => lhs.div(rhs, heap),
        NdArrayBinaryOp::FloorDiv => lhs.floordiv(rhs, heap),
        NdArrayBinaryOp::Mod => lhs.modulo(rhs, heap),
        NdArrayBinaryOp::Pow => lhs.pow(rhs, heap),
    }
}

/// Tries to dispatch an ndarray binary operation.
///
/// Returns `Ok(Some(value))` if either operand is an ndarray and the operation succeeded,
/// `Ok(None)` if neither operand is an ndarray (caller should fall through to normal dispatch),
/// or `Err` if the operation failed (e.g., shape mismatch).
fn try_ndarray_binary(
    lhs: &Value,
    rhs: &Value,
    op: NdArrayBinaryOp,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    // Both operands must involve at least one ndarray
    let lhs_id = if let Value::Ref(id) = lhs { Some(*id) } else { None };
    let rhs_id = if let Value::Ref(id) = rhs { Some(*id) } else { None };

    // Case 1: NdArray op NdArray
    if let (Some(lid), Some(rid)) = (lhs_id, rhs_id) {
        let lhs_is_ndarray = matches!(vm.heap.get(lid), HeapData::NdArray(_));
        let rhs_is_ndarray = matches!(vm.heap.get(rid), HeapData::NdArray(_));
        if lhs_is_ndarray && rhs_is_ndarray {
            let HeapData::NdArray(l) = vm.heap.get(lid) else {
                unreachable!()
            };
            let HeapData::NdArray(r) = vm.heap.get(rid) else {
                unreachable!()
            };
            return ndarray_array_op(l, r, op, vm.heap).map(Some);
        }
    }

    // Case 2: NdArray op scalar
    if let Some(lid) = lhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(lid)
        && let Some((scalar, is_float)) = value_to_f64(rhs)
    {
        return ndarray_scalar_op(arr, scalar, is_float, op, false, vm.heap).map(Some);
    }

    // Case 3: scalar op NdArray
    if let Some(rid) = rhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(rid)
        && let Some((scalar, is_float)) = value_to_f64(lhs)
    {
        return ndarray_scalar_op(arr, scalar, is_float, op, true, vm.heap).map(Some);
    }

    Ok(None)
}

/// Supported ndarray element-wise bitwise operations.
#[derive(Debug, Clone, Copy)]
enum NdArrayBitwiseOp {
    And,
    Or,
    Xor,
}

/// Extracts an integer value from a `Value` for bitwise ndarray operations.
///
/// Returns `Some(i64)` for `Int`, `Bool` (True=1, False=0). Returns `None` for non-integer types.
fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// Tries to dispatch an ndarray bitwise operation.
///
/// Returns `Ok(Some(value))` if either operand is an ndarray and the operation succeeded,
/// `Ok(None)` if neither operand is an ndarray (caller should fall through to normal dispatch),
/// or `Err` if the operation failed (e.g., float dtype, shape mismatch).
fn try_ndarray_bitwise(
    lhs: &Value,
    rhs: &Value,
    op: NdArrayBitwiseOp,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let lhs_id = if let Value::Ref(id) = lhs { Some(*id) } else { None };
    let rhs_id = if let Value::Ref(id) = rhs { Some(*id) } else { None };

    // Case 1: NdArray op NdArray
    if let (Some(lid), Some(rid)) = (lhs_id, rhs_id) {
        let lhs_is_ndarray = matches!(vm.heap.get(lid), HeapData::NdArray(_));
        let rhs_is_ndarray = matches!(vm.heap.get(rid), HeapData::NdArray(_));
        if lhs_is_ndarray && rhs_is_ndarray {
            let HeapData::NdArray(l) = vm.heap.get(lid) else {
                unreachable!()
            };
            let HeapData::NdArray(r) = vm.heap.get(rid) else {
                unreachable!()
            };
            let result = match op {
                NdArrayBitwiseOp::And => l.bitand(r, vm.heap),
                NdArrayBitwiseOp::Or => l.bitor(r, vm.heap),
                NdArrayBitwiseOp::Xor => l.bitxor(r, vm.heap),
            };
            return result.map(Some);
        }
    }

    // Case 2: NdArray op scalar
    if let Some(lid) = lhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(lid)
        && let Some(scalar) = value_to_i64(rhs)
    {
        let result = match op {
            NdArrayBitwiseOp::And => arr.bitand_scalar(scalar, vm.heap),
            NdArrayBitwiseOp::Or => arr.bitor_scalar(scalar, vm.heap),
            NdArrayBitwiseOp::Xor => arr.bitxor_scalar(scalar, vm.heap),
        };
        return result.map(Some);
    }

    // Case 3: scalar op NdArray (commutative)
    if let Some(rid) = rhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(rid)
        && let Some(scalar) = value_to_i64(lhs)
    {
        let result = match op {
            NdArrayBitwiseOp::And => arr.bitand_scalar(scalar, vm.heap),
            NdArrayBitwiseOp::Or => arr.bitor_scalar(scalar, vm.heap),
            NdArrayBitwiseOp::Xor => arr.bitxor_scalar(scalar, vm.heap),
        };
        return result.map(Some);
    }

    Ok(None)
}

/// Supported ndarray in-place operations.
#[derive(Debug, Clone, Copy)]
enum NdArrayInplaceOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

/// Tries to perform an in-place ndarray operation by peeking at the stack.
///
/// If the second-from-top value is an ndarray and the top value is a compatible
/// operand (scalar or same-shape ndarray), modifies the array data in-place,
/// drops the rhs, and leaves the lhs on the stack. Returns `Ok(true)` on success.
///
/// Returns `Ok(false)` if neither operand is an ndarray, leaving the stack unchanged
/// so the caller can fall through to the normal binary operation.
fn try_ndarray_inplace(vm: &mut VM<'_, impl ResourceTracker>, op: NdArrayInplaceOp) -> bool {
    // Peek at the two top-of-stack values without popping
    let lhs_ref = vm.peek_at(1);

    // Only proceed if lhs is an NdArray
    let Value::Ref(lid) = lhs_ref else {
        return false;
    };
    if !matches!(vm.heap.get(*lid), HeapData::NdArray(_)) {
        return false;
    }

    let lid = *lid;

    // Pop rhs into a HeapGuard so we can reclaim it on the fallback path.
    let mut rhs_guard = HeapGuard::new(vm.pop(), vm);
    let (rhs, vm_ref) = rhs_guard.as_parts_mut();

    // Try scalar rhs
    if let Some((scalar, _is_float)) = value_to_f64(rhs) {
        // Consume the guard — scalar types (Int/Float/Bool) don't need heap cleanup
        let (rhs, vm) = rhs_guard.into_parts();
        rhs.drop_with_heap(vm);
        let lhs = vm.pop();
        // Use HeapRead API for mutation
        let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(lid) else {
            unreachable!()
        };
        apply_inplace_scalar(arr_read.get_mut(vm.heap), scalar, op);
        drop(arr_read);
        vm.push(lhs);
        return true;
    }

    // Try ndarray rhs
    if let Value::Ref(rid) = rhs {
        let rid = *rid;
        let len_matches = {
            let HeapData::NdArray(r) = vm_ref.heap.get(rid) else {
                // rhs is a Ref but not an NdArray — put it back
                let (rhs, vm) = rhs_guard.into_parts();
                vm.push(rhs);
                return false;
            };
            let HeapData::NdArray(l) = vm_ref.heap.get(lid) else {
                unreachable!()
            };
            r.data().len() == l.data().len()
        };
        if len_matches {
            let rhs_data: Vec<f64> = {
                let HeapData::NdArray(r) = vm_ref.heap.get(rid) else {
                    unreachable!()
                };
                r.data().to_vec()
            };
            let (rhs, vm) = rhs_guard.into_parts();
            rhs.drop_with_heap(vm);
            let lhs = vm.pop();
            let HeapReadOutput::NdArray(mut arr_read) = vm.heap.read(lid) else {
                unreachable!()
            };
            apply_inplace_array(arr_read.get_mut(vm.heap), &rhs_data, op);
            drop(arr_read);
            vm.push(lhs);
            return true;
        }
    }

    // Not a compatible ndarray operation — put rhs back on the stack
    let (rhs, vm) = rhs_guard.into_parts();
    vm.push(rhs);
    false
}

/// Applies an in-place scalar operation to an ndarray's data.
fn apply_inplace_scalar(arr: &mut NdArray, scalar: f64, op: NdArrayInplaceOp) {
    match op {
        NdArrayInplaceOp::Add => {
            for v in &mut arr.data {
                *v += scalar;
            }
        }
        NdArrayInplaceOp::Sub => {
            for v in &mut arr.data {
                *v -= scalar;
            }
        }
        NdArrayInplaceOp::Mul => {
            for v in &mut arr.data {
                *v *= scalar;
            }
        }
        NdArrayInplaceOp::Div => {
            arr.dtype = NdArrayDtype::Float64;
            for v in &mut arr.data {
                *v /= scalar;
            }
        }
        NdArrayInplaceOp::FloorDiv => {
            for v in &mut arr.data {
                *v = (*v / scalar).floor();
            }
        }
        NdArrayInplaceOp::Mod => {
            for v in &mut arr.data {
                *v %= scalar;
            }
        }
        NdArrayInplaceOp::Pow => {
            for v in &mut arr.data {
                *v = v.powf(scalar);
            }
        }
    }
}

/// Applies an in-place array-to-array operation to an ndarray's data.
fn apply_inplace_array(arr: &mut NdArray, rhs_data: &[f64], op: NdArrayInplaceOp) {
    match op {
        NdArrayInplaceOp::Add => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v += rv;
            }
        }
        NdArrayInplaceOp::Sub => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v -= rv;
            }
        }
        NdArrayInplaceOp::Mul => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v *= rv;
            }
        }
        NdArrayInplaceOp::Div => {
            arr.dtype = NdArrayDtype::Float64;
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v /= rv;
            }
        }
        NdArrayInplaceOp::FloorDiv => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v = (*v / rv).floor();
            }
        }
        NdArrayInplaceOp::Mod => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v %= rv;
            }
        }
        NdArrayInplaceOp::Pow => {
            for (v, rv) in arr.data.iter_mut().zip(rhs_data.iter()) {
                *v = v.powf(*rv);
            }
        }
    }
}
