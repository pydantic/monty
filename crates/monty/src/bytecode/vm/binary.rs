//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError},
    heap::{HeapData, HeapGuard, HeapReadOutput},
    resource::ResourceTracker,
    types::{PyTrait, Set, dict_view::collect_iterable_to_set, set::SetBinaryOp},
    value::BitwiseOp,
};

impl<T: ResourceTracker> VM<'_, '_, T> {
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
    /// For immutable types, we fall back to regular addition.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    ///
    /// Note: Cannot use `defer_drop!` for `lhs` here because on successful in-place
    /// operation, we need to push `lhs` back onto the stack rather than drop it.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
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
    /// Currently not implemented - returns a `NotImplementedError`.
    /// Matrix multiplication requires numpy-like array types which Monty doesn't support.
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        lhs.drop_with_heap(self);
        rhs.drop_with_heap(self);
        Err(ExcType::not_implemented("matrix multiplication (@) is not supported").into())
    }

    /// Implements dict-view set-like operators before falling back to other dispatch.
    ///
    /// Returning `Ok(None)` means the left operand was not a set-like dict view, so the
    /// caller should continue with ordinary numeric or pure-set dispatch.
    fn binary_dict_view_op(
        &mut self,
        lhs: &crate::value::Value,
        rhs: &crate::value::Value,
        op: DictViewBinaryOp,
    ) -> Result<Option<crate::value::Value>, RunError> {
        let this = self;
        let crate::value::Value::Ref(lhs_id) = lhs else {
            return Ok(None);
        };

        let lhs_set = match this.heap.get(*lhs_id) {
            HeapData::DictKeysView(view) => view.to_set(this)?,
            HeapData::DictItemsView(view) => view.to_set(this)?,
            _ => return Ok(None),
        };
        defer_drop!(lhs_set, this);

        let rhs_set = collect_iterable_to_set(rhs.clone_with_heap(this), this)?;
        defer_drop!(rhs_set, this);

        let result = apply_dict_view_binary_op(lhs_set, rhs_set, op, this)?;

        let result_id = this.heap.allocate(HeapData::Set(result))?;
        Ok(Some(crate::value::Value::Ref(result_id)))
    }

    /// Implements pure set/frozenset binary operators with strict operand checks.
    ///
    /// Method forms accept arbitrary iterables, but the operator forms handled here
    /// must reject non-set operands so Monty matches CPython's `TypeError` behavior.
    fn binary_set_op(
        &mut self,
        lhs: &crate::value::Value,
        rhs: &crate::value::Value,
        op: SetBinaryOp,
    ) -> Result<Option<crate::value::Value>, RunError> {
        let this = self;
        let crate::value::Value::Ref(lhs_id) = lhs else {
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
        Ok(Some(crate::value::Value::Ref(result_id)))
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
    vm: &mut VM<'_, '_, impl ResourceTracker>,
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
