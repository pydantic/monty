//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, HeapData},
    modules::collections::counter::{CounterOp, counter_binary_op, counter_inplace_op},
    types::{PyTrait, deque::deque_extend},
    value::Value,
};

impl VM<'_> {
    /// Handles a binary `Counter` operator (`+ - & |`) when both operands are
    /// Counters, returning the result. Any other operand pair returns `None` so
    /// the caller falls through to its normal numeric/dict/set dispatch.
    fn binary_counter_op(&mut self, lhs: &Value, rhs: &Value, op: CounterOp) -> RunResult<Option<Value>> {
        let (Value::Ref(l), Value::Ref(r)) = (lhs, rhs) else {
            return Ok(None);
        };
        let (l, r) = (*l, *r);
        let both_counters = matches!(self.heap.get(l), HeapData::Dict(d) if d.is_counter())
            && matches!(self.heap.get(r), HeapData::Dict(d) if d.is_counter());
        if both_counters {
            Ok(Some(counter_binary_op(l, r, op, self)?))
        } else {
            Ok(None)
        }
    }

    /// Runs a binary operator, first giving `Counter` a chance to handle it when
    /// both operands are Counters, and otherwise applying the generic dispatcher.
    fn binary_op_counter(
        &mut self,
        op: CounterOp,
        operation: impl FnOnce(&Value, &Value, &mut Self) -> RunResult<Value>,
    ) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        if let Some(result) = this.binary_counter_op(lhs, rhs, op)? {
            this.push(result);
            return Ok(());
        }

        let result = operation(lhs, rhs, this)?;
        this.push(result);
        Ok(())
    }

    /// Handles an in-place `Counter` operator (`+= -= &= |=`) when the *left*
    /// operand is a Counter, mutating it and leaving it on the stack.
    ///
    /// Only the left operand must be a Counter: CPython's `__iadd__`/etc. accept
    /// any mapping on the right (`c += {'a': 2}`) and reject a non-mapping with
    /// whatever the underlying `other.items()` / `other[elem]` raises — so once
    /// the left is a Counter this owns the operation, error paths included.
    /// Peeks the left operand so a non-Counter left leaves the stack untouched
    /// for the caller's binary fallback; returns whether it was handled.
    fn try_inplace_counter(&mut self, op: CounterOp) -> RunResult<bool> {
        let len = self.stack.len();
        let Some(&Value::Ref(l)) = self.stack.get(len - 2) else {
            return Ok(false);
        };
        if !matches!(self.heap.get(l), HeapData::Dict(d) if d.is_counter()) {
            return Ok(false);
        }
        let rhs = self.pop();
        let mut lhs_guard = DropGuard::new(self.pop(), self);
        let (_, this) = lhs_guard.as_parts_mut();
        let outcome = counter_inplace_op(l, &rhs, op, this);
        rhs.drop_with(this);
        outcome?;
        // The left operand keeps its identity, so an alias sees the update.
        let (lhs, this) = lhs_guard.into_parts();
        this.push(lhs);
        Ok(true)
    }

    /// Handles `deque += <iterable>` in place, mutating the left deque.
    ///
    /// CPython's `deque.__iadd__` *is* `extend`, so any iterable works
    /// (`d += [1, 2]`, `d += 'ab'`) and a non-iterable raises `TypeError` from the
    /// iterator protocol rather than falling back to `+`'s concatenation error —
    /// which is why this cannot ride the `py_iadd_impl` trait method (that returns
    /// only a `ResourceError`). Peeks the left operand so a non-deque leaves the
    /// stack untouched; returns whether it was handled.
    fn try_inplace_deque(&mut self) -> RunResult<bool> {
        let len = self.stack.len();
        let Some(&Value::Ref(deque_id)) = self.stack.get(len - 2) else {
            return Ok(false);
        };
        if !matches!(self.heap.get(deque_id), HeapData::Deque(_)) {
            return Ok(false);
        }
        let rhs = self.pop();
        let mut lhs_guard = DropGuard::new(self.pop(), self);
        let (_, this) = lhs_guard.as_parts_mut();
        // `extend` consumes `rhs`, raising `TypeError` if it is not iterable.
        deque_extend(deque_id, rhs, this)?;
        // The left operand keeps its identity, so an alias sees the update.
        let (lhs, this) = lhs_guard.into_parts();
        this.push(lhs);
        Ok(true)
    }

    /// Binary addition.
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        self.binary_op_counter(CounterOp::Add, |lhs, rhs, vm| lhs.py_add(rhs, vm))
    }

    /// Binary subtraction.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        self.binary_op_counter(CounterOp::Sub, |lhs, rhs, vm| lhs.py_sub(rhs, vm))
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
        self.binary_op_counter(CounterOp::And, |lhs, rhs, vm| lhs.py_and(rhs, vm))
    }

    /// Binary `|`.
    pub(super) fn binary_or(&mut self) -> Result<(), RunError> {
        self.binary_op_counter(CounterOp::Or, |lhs, rhs, vm| lhs.py_or(rhs, vm))
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

    /// `-=` — an in-place Counter subtraction, or ordinary binary subtraction.
    pub(super) fn inplace_sub(&mut self) -> Result<(), RunError> {
        if self.try_inplace_counter(CounterOp::Sub)? {
            Ok(())
        } else {
            self.binary_sub()
        }
    }

    /// `&=` — an in-place Counter intersection, or ordinary binary `&`.
    pub(super) fn inplace_and(&mut self) -> Result<(), RunError> {
        if self.try_inplace_counter(CounterOp::And)? {
            Ok(())
        } else {
            self.binary_and()
        }
    }

    /// `|=` — an in-place Counter union, or ordinary binary `|`.
    pub(super) fn inplace_or(&mut self) -> Result<(), RunError> {
        if self.try_inplace_counter(CounterOp::Or)? {
            Ok(())
        } else {
            self.binary_or()
        }
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// A Counter (`+=` adds counts) and a deque (`+=` is extend) are handled first
    /// because their in-place forms diverge from a plain binary `+`. For everything
    /// else, mutable types like lists mutate in place via `py_iadd_impl` (returning
    /// `true`), while immutable types fall back to regular addition.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
        if self.try_inplace_counter(CounterOp::Add)? {
            return Ok(());
        }
        if self.try_inplace_deque()? {
            return Ok(());
        }
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        // Use DropGuard because inplace addition will push lhs back on the stack if successful
        let mut lhs_guard = DropGuard::new(this.pop(), this);
        let (lhs, this) = lhs_guard.as_parts_mut();

        if lhs.py_iadd_impl(rhs, this, lhs.ref_id())? {
            let (lhs, this) = lhs_guard.into_parts();
            this.push(lhs);
            return Ok(());
        }

        if let Some(value) = lhs.py_add_result(rhs, this)? {
            this.push(value);
            Ok(())
        } else {
            let lhs_type = lhs.py_type(this);
            Err(ExcType::binary_type_error(
                "+=",
                lhs_type,
                lhs.py_type_name(this),
                rhs.py_type_name(this),
            ))
        }
    }

    /// Binary matrix multiplication (`@` operator).
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        self.binary_op(|lhs, rhs, vm| lhs.py_matmul(rhs, vm))
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
}
