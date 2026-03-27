//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::HeapData,
    resource::ResourceTracker,
    types::{LongInt, NdArray, PyTrait},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, '_, T> {
    /// Equality comparison.
    pub(super) fn compare_eq(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path: element-wise comparison returning boolean array
        if let Some(result) = try_ndarray_cmp(lhs, rhs, NdArrayCmpOp::Eq, this)? {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_eq(rhs, this)?;
        this.push(Value::Bool(result));
        Ok(())
    }

    /// Inequality comparison.
    pub(super) fn compare_ne(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path: element-wise comparison returning boolean array
        if let Some(result) = try_ndarray_cmp(lhs, rhs, NdArrayCmpOp::Ne, this)? {
            this.push(result);
            return Ok(());
        }

        let result = !lhs.py_eq(rhs, this)?;
        this.push(Value::Bool(result));
        Ok(())
    }

    /// Ordering comparison with a predicate.
    pub(super) fn compare_ord<F>(&mut self, check: F) -> Result<(), RunError>
    where
        F: Fn(std::cmp::Ordering) -> bool,
    {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        // NdArray fast path: detect ordering comparisons involving ndarrays.
        // We need to determine the specific comparison from the check predicate by testing it.
        if let Some(ndarray_op) = ndarray_cmp_from_ord_check(&check)
            && let Some(result) = try_ndarray_cmp(lhs, rhs, ndarray_op, this)?
        {
            this.push(result);
            return Ok(());
        }

        let result = lhs.py_cmp(rhs, this)?.is_some_and(check);
        this.push(Value::Bool(result));
        Ok(())
    }

    /// Identity comparison (is/is not).
    ///
    /// Compares identity using `Value::is()` which compares IDs.
    ///
    /// Identity is determined by `Value::id()` which uses:
    /// - Fixed IDs for singletons (None, True, False, Ellipsis)
    /// - Interned string/bytes index for InternString/InternBytes
    /// - HeapId for heap-allocated values (Ref)
    /// - Value-based hashing for immediate types (Int, Float, Function, etc.)
    pub(super) fn compare_is(&mut self, negate: bool) {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.is(rhs);
        this.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in).
    pub(super) fn compare_in(&mut self, negate: bool) -> Result<(), RunError> {
        let this = self;

        let container = this.pop(); // container (rhs)
        defer_drop!(container, this);
        let item = this.pop(); // item to find (lhs)
        defer_drop!(item, this);

        let contained = container.py_contains(item, this)?;
        this.push(Value::Bool(if negate { !contained } else { contained }));
        Ok(())
    }

    /// Modulo equality comparison: a % b == k
    ///
    /// This is an optimization for patterns like `x % 3 == 0`. The constant k
    /// is provided by the caller (fetched from the constant pool using the
    /// cached code reference in the run loop).
    ///
    /// Uses a fast path for Int/Float types via `py_mod_eq`, and falls back to
    /// computing `py_mod` then comparing with `py_eq` for other types (e.g., LongInt).
    pub(super) fn compare_mod_eq(&mut self, k: &Value) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop(); // divisor (b)
        defer_drop!(rhs, this);
        let lhs = this.pop(); // dividend (a)
        defer_drop!(lhs, this);

        // Try fast path for Int/Float types
        let mod_result = match k {
            Value::Int(k_val) => lhs.py_mod_eq(rhs, *k_val),
            _ => None,
        };

        if let Some(is_equal) = mod_result {
            // Fast path succeeded
            this.push(Value::Bool(is_equal));
            Ok(())
        } else {
            // Fallback: compute py_mod then compare with py_eq
            // This handles LongInt and other Ref types
            let mod_value = lhs.py_mod(rhs, this);

            match mod_value {
                Ok(Some(v)) => {
                    defer_drop!(v, this);

                    // Handle InternLongInt by converting to heap LongInt for comparison
                    let k_value = if let Value::InternLongInt(id) = k {
                        let bi = this.interns.get_long_int(*id).clone();
                        LongInt::new(bi).into_value(this.heap)?
                    } else {
                        // k is from the constant pool and is always an immediate value
                        k.clone_immediate()
                    };
                    defer_drop!(k_value, this);

                    let is_equal = v.py_eq(k_value, this)?;
                    this.push(Value::Bool(is_equal));
                    Ok(())
                }
                Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for %")),
                Err(e) => Err(e),
            }
        }
    }
}

/// Supported ndarray element-wise comparison operations.
#[derive(Debug, Clone, Copy)]
enum NdArrayCmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// Extracts a scalar f64 from a `Value`, if it is a numeric type.
///
/// Comparisons always return `Bool` dtype so the float flag is unused, but we match
/// the return type from `binary.rs` for consistency.
fn value_to_f64(v: &Value) -> Option<(f64, bool)> {
    match v {
        Value::Int(i) => Some((*i as f64, false)),
        Value::Float(f) => Some((*f, true)),
        Value::Bool(b) => Some((if *b { 1.0 } else { 0.0 }, false)),
        _ => None,
    }
}

/// Determines the ndarray comparison op from a `compare_ord` predicate.
///
/// Tests the predicate with `Less`, `Equal`, and `Greater` to infer which
/// comparison is being performed (e.g., `<`, `<=`, `>`, `>=`).
fn ndarray_cmp_from_ord_check(check: &impl Fn(std::cmp::Ordering) -> bool) -> Option<NdArrayCmpOp> {
    let lt = check(std::cmp::Ordering::Less);
    let eq = check(std::cmp::Ordering::Equal);
    let gt = check(std::cmp::Ordering::Greater);
    match (lt, eq, gt) {
        (true, false, false) => Some(NdArrayCmpOp::Lt),
        (true, true, false) => Some(NdArrayCmpOp::Lte),
        (false, false, true) => Some(NdArrayCmpOp::Gt),
        (false, true, true) => Some(NdArrayCmpOp::Gte),
        _ => None,
    }
}

/// Dispatches an ndarray comparison between an ndarray and a scalar.
fn ndarray_scalar_cmp(
    arr: &NdArray,
    scalar: f64,
    op: NdArrayCmpOp,
    heap: &crate::heap::Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    match op {
        NdArrayCmpOp::Gt => arr.gt_scalar(scalar, heap),
        NdArrayCmpOp::Lt => arr.lt_scalar(scalar, heap),
        NdArrayCmpOp::Eq => arr.eq_scalar(scalar, heap),
        NdArrayCmpOp::Gte => arr.gte_scalar(scalar, heap),
        NdArrayCmpOp::Lte => arr.lte_scalar(scalar, heap),
        NdArrayCmpOp::Ne => arr.ne_scalar(scalar, heap),
    }
}

/// Dispatches an ndarray comparison between two ndarrays.
fn ndarray_array_cmp(
    lhs: &NdArray,
    rhs: &NdArray,
    op: NdArrayCmpOp,
    heap: &crate::heap::Heap<impl ResourceTracker>,
) -> RunResult<Value> {
    match op {
        NdArrayCmpOp::Gt => lhs.gt(rhs, heap),
        NdArrayCmpOp::Lt => lhs.lt(rhs, heap),
        NdArrayCmpOp::Eq => lhs.eq_array(rhs, heap),
        NdArrayCmpOp::Gte => lhs.gte(rhs, heap),
        NdArrayCmpOp::Lte => lhs.lte(rhs, heap),
        NdArrayCmpOp::Ne => lhs.ne_array(rhs, heap),
    }
}

/// Tries to dispatch an ndarray comparison operation.
///
/// Returns `Ok(Some(value))` if either operand is an ndarray, `Ok(None)` if neither is.
fn try_ndarray_cmp(
    lhs: &Value,
    rhs: &Value,
    op: NdArrayCmpOp,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let lhs_id = if let Value::Ref(id) = lhs { Some(*id) } else { None };
    let rhs_id = if let Value::Ref(id) = rhs { Some(*id) } else { None };

    // Case 1: NdArray cmp NdArray
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
            return ndarray_array_cmp(l, r, op, vm.heap).map(Some);
        }
    }

    // Case 2: NdArray cmp scalar
    if let Some(lid) = lhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(lid)
        && let Some((scalar, _)) = value_to_f64(rhs)
    {
        return ndarray_scalar_cmp(arr, scalar, op, vm.heap).map(Some);
    }

    // Case 3: scalar cmp NdArray (reverse the comparison)
    if let Some(rid) = rhs_id
        && let HeapData::NdArray(arr) = vm.heap.get(rid)
        && let Some((scalar, _)) = value_to_f64(lhs)
    {
        // Reverse: `5 > arr` becomes `arr < 5`
        let reversed_op = match op {
            NdArrayCmpOp::Gt => NdArrayCmpOp::Lt,
            NdArrayCmpOp::Lt => NdArrayCmpOp::Gt,
            NdArrayCmpOp::Gte => NdArrayCmpOp::Lte,
            NdArrayCmpOp::Lte => NdArrayCmpOp::Gte,
            NdArrayCmpOp::Eq => NdArrayCmpOp::Eq,
            NdArrayCmpOp::Ne => NdArrayCmpOp::Ne,
        };
        return ndarray_scalar_cmp(arr, scalar, reversed_op, vm.heap).map(Some);
    }

    Ok(None)
}
