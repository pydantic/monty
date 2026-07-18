//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError},
    heap::DropGuard,
    resource::ResourceTracker,
    types::{BinaryOp, PyTrait},
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Binary addition.
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Add)
    }

    /// Binary subtraction.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Sub)
    }

    /// Binary multiplication.
    pub(super) fn binary_mult(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Mul)
    }

    /// Binary division.
    pub(super) fn binary_div(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::TrueDiv)
    }

    /// Binary floor division.
    pub(super) fn binary_floordiv(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::FloorDiv)
    }

    /// Binary modulo.
    pub(super) fn binary_mod(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Mod)
    }

    /// Binary power.
    #[inline(never)]
    pub(super) fn binary_pow(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Pow)
    }

    /// Binary bitwise operation.
    pub(super) fn binary_bitwise(&mut self, op: BinaryOp) -> Result<(), RunError> {
        self.binary_op(op)
    }

    /// Binary `&`.
    pub(super) fn binary_and(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::And)
    }

    /// Binary `|`.
    pub(super) fn binary_or(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Or)
    }

    /// Binary `^`.
    pub(super) fn binary_xor(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::Xor)
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
        // Use DropGuard because inplace addition will push lhs back on the stack if successful
        let mut lhs_guard = DropGuard::new(this.pop(), this);
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
        let lhs_name = lhs_type.name(this.heap, this.interns);
        Err(ExcType::binary_type_error(
            "+=",
            lhs_type,
            lhs_name,
            rhs.py_type_name(this),
        ))
    }

    /// Binary matrix multiplication (`@` operator).
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        self.binary_op(BinaryOp::MatMul)
    }

    /// Executes a binary operation through `Value::py_binary`.
    fn binary_op(&mut self, op: BinaryOp) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.py_binary(rhs, op, this)?;
        this.push(result);
        Ok(())
    }
}
