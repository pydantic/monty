//! Unary, binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    bytecode::op::Opcode,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, HeapData, HeapReadOutput},
    types::PyTrait,
    value::Value,
};

impl VM<'_> {
    /// Unary minus (`-x`).
    pub(super) fn unary_neg(&mut self) -> Result<(), RunError> {
        self.unary_op(PyTrait::py_neg_impl, "-")
    }

    /// Unary plus (`+x`) — an identity for numbers, but not for `bool`
    /// (`+True` is `1`) or a `Counter` (which drops its non-positive counts).
    pub(super) fn unary_pos(&mut self) -> Result<(), RunError> {
        self.unary_op(PyTrait::py_pos_impl, "+")
    }

    /// Bitwise inversion (`~x`).
    pub(super) fn unary_invert(&mut self) -> Result<(), RunError> {
        self.unary_op(PyTrait::py_invert_impl, "~")
    }

    /// Binary addition.
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_add(rhs, vm))
    }

    /// Binary subtraction.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_sub(rhs, vm))
    }

    /// Binary multiplication.
    pub(super) fn binary_mult(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_mul(rhs, vm))
    }

    /// Binary division.
    pub(super) fn binary_div(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_truediv(rhs, vm))
    }

    /// Binary floor division.
    pub(super) fn binary_floordiv(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_floordiv(rhs, vm))
    }

    /// Binary modulo.
    pub(super) fn binary_mod(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_mod(rhs, vm))
    }

    /// Binary power.
    #[inline(never)]
    pub(super) fn binary_pow(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_pow(rhs, None, vm))
    }

    /// Binary `&`.
    pub(super) fn binary_and(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_and(rhs, vm))
    }

    /// Binary `|`.
    pub(super) fn binary_or(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_or(rhs, vm))
    }

    /// Binary `^`.
    pub(super) fn binary_xor(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_xor(rhs, vm))
    }

    /// Binary left shift.
    pub(super) fn binary_lshift(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_lshift(rhs, vm))
    }

    /// Binary right shift.
    pub(super) fn binary_rshift(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_rshift(rhs, vm))
    }

    /// In-place addition, with buffer reuse for unobserved strings.
    ///
    /// Mutable types (a list, a `Counter`, a `deque` whose `+=` is `extend`)
    /// mutate through `py_iadd_impl` and keep their identity, so an alias sees
    /// the update; other immutable values fall back to ordinary addition. Uses lazy type
    /// capture: only calls `py_type()` in error paths.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
        if self.try_string_append()? {
            return Ok(());
        }

        self.inplace_op(PyTrait::py_iadd_impl, |lhs, rhs, vm| {
            if let Some(value) = lhs.py_add_result(rhs, vm)? {
                Ok(value)
            } else {
                let lhs_type = lhs.py_type(vm);
                Err(ExcType::binary_type_error(
                    "+=",
                    lhs_type,
                    lhs.py_type_name(vm),
                    rhs.py_type_name(vm),
                ))
            }
        })
    }

    /// Fuses string `+=` with an infallible matching store; no checkpoint may expose a half-finished assignment.
    fn try_string_append(&mut self) -> RunResult<bool> {
        let len = self.stack.len();
        let Value::Ref(id) = self.stack[len - 2] else {
            return Ok(false);
        };
        if !self.heap.has_binding_and_operand_only(id) || !self.stack[len - 1].is_str(self.heap) {
            return Ok(false);
        }
        if !matches!(self.heap.get(id), HeapData::Str(text) if text.can_reuse_append_buffer(self.heap)) {
            return Ok(false);
        }

        let ip = self.current_frame.ip;
        let bytes = self.current_frame.bytecode;
        let Some(opcode) = bytes.get(ip).and_then(|byte| Opcode::from_repr(*byte)) else {
            return Ok(false);
        };
        let (global, width) = match opcode {
            Opcode::StoreLocal => (false, 2),
            Opcode::StoreLocalW => (false, 3),
            Opcode::StoreGlobal => (true, 3),
            _ => return Ok(false),
        };
        let Some(operand) = bytes.get(ip + 1..ip + width) else {
            return Ok(false);
        };
        let slot = if width == 2 {
            usize::from(operand[0])
        } else {
            usize::from(u16::from_le_bytes([operand[0], operand[1]]))
        };
        let target = if global {
            self.globals.get(slot)
        } else {
            self.stack.get(self.current_frame.stack_base() + slot)
        };
        if !matches!(target, Some(Value::Ref(target_id)) if *target_id == id) {
            return Ok(false);
        }

        let this = self;
        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);
        let Value::Ref(id) = lhs else {
            unreachable!("append operand was checked before popping")
        };
        let HeapReadOutput::Str(string) = this.heap.read(*id) else {
            unreachable!("append operand is a string")
        };
        string.append_unobserved(rhs, this)?;

        // The sole binding already points to the updated object. Skip the store without running Python or polling.
        this.current_frame.ip += width;
        Ok(true)
    }

    /// `-=` — an in-place mutation (a `Counter` subtracts counts), or binary `-`.
    pub(super) fn inplace_sub(&mut self) -> Result<(), RunError> {
        self.inplace_op(PyTrait::py_isub_impl, |lhs, rhs, vm| lhs.py_sub(rhs, vm))
    }

    /// `&=` — an in-place mutation (a `Counter` intersects counts), or binary `&`.
    pub(super) fn inplace_and(&mut self) -> Result<(), RunError> {
        self.inplace_op(PyTrait::py_iand_impl, |lhs, rhs, vm| lhs.py_and(rhs, vm))
    }

    /// `|=` — an in-place mutation (a `Counter` unions counts), or binary `|`.
    pub(super) fn inplace_or(&mut self) -> Result<(), RunError> {
        self.inplace_op(PyTrait::py_ior_impl, |lhs, rhs, vm| lhs.py_or(rhs, vm))
    }

    /// Binary matrix multiplication (`@` operator).
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_matmul(rhs, vm))
    }

    /// Applies a unary operation while owning stack-value cleanup, raising
    /// CPython's `TypeError` (naming `symbol`) when the type has no such form.
    fn unary_op(
        &mut self,
        operation: impl FnOnce(&Value, &mut Self) -> RunResult<Option<Value>>,
        symbol: &str,
    ) -> Result<(), RunError> {
        let this = self;

        let value = this.pop();
        defer_drop!(value, this);

        if let Some(result) = operation(value, this)? {
            this.push(result);
            Ok(())
        } else {
            Err(ExcType::unary_type_error(symbol, &value.py_type_name(this)))
        }
    }

    /// Applies a binary operation while owning stack-value cleanup.
    fn binary_op(
        &mut self,
        operation: impl FnOnce(&Value, &Value, &mut Self) -> RunResult<Value>,
    ) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = operation(lhs, rhs, this)?;
        this.push(result);
        Ok(())
    }

    /// Applies an in-place operation, offering the left operand `inplace` (its
    /// `py_i*_impl` hook) first and otherwise falling back to `binary`.
    ///
    /// A hook reporting `true` has mutated the left operand, which is pushed
    /// back so it keeps its identity; `false` means the type has no in-place
    /// form and the ordinary binary operator produces a fresh value.
    fn inplace_op(
        &mut self,
        inplace: impl FnOnce(&mut Value, &Value, &mut Self) -> RunResult<bool>,
        binary: impl FnOnce(&Value, &Value, &mut Self) -> RunResult<Value>,
    ) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        // A `DropGuard` rather than `defer_drop!`: a successful in-place operation
        // reclaims the left operand to push it back onto the stack.
        let mut lhs_guard = DropGuard::new(this.pop(), this);
        let (lhs, this) = lhs_guard.as_parts_mut();

        if inplace(lhs, rhs, this)? {
            let (lhs, this) = lhs_guard.into_parts();
            this.push(lhs);
            Ok(())
        } else {
            let result = binary(lhs, rhs, this)?;
            this.push(result);
            Ok(())
        }
    }
}
