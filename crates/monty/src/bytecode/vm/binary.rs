//! Binary and in-place operation helpers for the VM.
//!
//! Binary operations follow the Python dunder protocol:
//! 1. Try the native `py_add`/`py_sub`/etc. on Value (fast path for builtins)
//! 2. If that returns `None`, check for instance dunder methods (`__add__`/`__radd__`/etc.)
//! 3. If the dunder returns `NotImplemented`, try the reflected dunder on the other operand
//!
//! The return type is `Result<CallResult, RunError>` because dunder methods may push
//! a new call frame (user-defined Python functions).

use super::{VM, call::CallResult};
use crate::{
    exception_private::{ExcType, RunError},
    heap::HeapGuard,
    intern::StaticStrings,
    io::PrintWriter,
    resource::ResourceTracker,
    types::PyTrait,
    value::BitwiseOp,
};

/// Helper macro for binary ops with dunder fallback.
///
/// Tries the native Value operation first, then falls back to dunder dispatch.
/// Returns `CallResult` to support frame-pushing dunder calls.
macro_rules! binary_op_with_dunder {
    ($self:expr, $py_op:ident, $op_str:expr, $dunder:expr, $reflected:expr $(, $extra_arg:expr)*) => {{
        let rhs = $self.pop();
        let lhs = $self.pop();

        // Fast path: try native operation
        match lhs.$py_op(&rhs, $self.heap $(, $extra_arg)*) {
            Ok(Some(v)) => {
                // Check for NotImplemented return from instance dunder that was called
                // inside py_op (shouldn't happen for native types, but be safe)
                lhs.drop_with_heap($self.heap);
                rhs.drop_with_heap($self.heap);
                Ok(CallResult::Push(v))
            }
            Ok(None) => {
                // Native op returned None - try dunder dispatch on instances
                let dunder_id: crate::intern::StringId = $dunder.into();
                let reflected_id: Option<crate::intern::StringId> = $reflected.map(|s: StaticStrings| s.into());

                match $self.try_binary_dunder(&lhs, &rhs, dunder_id, reflected_id)? {
                    Some(result) => {
                        lhs.drop_with_heap($self.heap);
                        rhs.drop_with_heap($self.heap);
                        Ok(result)
                    }
                    None => {
                        let lhs_type = lhs.py_type($self.heap);
                        let rhs_type = rhs.py_type($self.heap);
                        lhs.drop_with_heap($self.heap);
                        rhs.drop_with_heap($self.heap);
                        Err(ExcType::binary_type_error($op_str, lhs_type, rhs_type))
                    }
                }
            }
            Err(e) => {
                lhs.drop_with_heap($self.heap);
                rhs.drop_with_heap($self.heap);
                Err(e.into())
            }
        }
    }};
}

/// Helper macro for in-place ops with dunder fallback.
///
/// Tries the native in-place operation, then native binary, then dunder dispatch.
macro_rules! inplace_op_with_dunder {
    ($self:expr, $py_op:ident, $py_inplace:ident, $op_str:expr,
     $inplace_dunder:expr, $dunder:expr, $reflected:expr $(, $extra_arg:expr)*) => {{
        let rhs = $self.pop();
        let lhs = $self.pop();

        // Fast path: try native operation
        match lhs.$py_op(&rhs, $self.heap $(, $extra_arg)*) {
            Ok(Some(v)) => {
                lhs.drop_with_heap($self.heap);
                rhs.drop_with_heap($self.heap);
                Ok(CallResult::Push(v))
            }
            Ok(None) => {
                // Native op returned None - try dunder dispatch
                let inplace_id: crate::intern::StringId = $inplace_dunder.into();
                let dunder_id: crate::intern::StringId = $dunder.into();
                let reflected_id: Option<crate::intern::StringId> = $reflected.map(|s: StaticStrings| s.into());

                match $self.try_inplace_dunder(&lhs, &rhs, inplace_id, dunder_id, reflected_id)? {
                    Some(result) => {
                        lhs.drop_with_heap($self.heap);
                        rhs.drop_with_heap($self.heap);
                        Ok(result)
                    }
                    None => {
                        let lhs_type = lhs.py_type($self.heap);
                        let rhs_type = rhs.py_type($self.heap);
                        lhs.drop_with_heap($self.heap);
                        rhs.drop_with_heap($self.heap);
                        Err(ExcType::binary_type_error($op_str, lhs_type, rhs_type))
                    }
                }
            }
            Err(e) => {
                lhs.drop_with_heap($self.heap);
                rhs.drop_with_heap($self.heap);
                Err(e.into())
            }
        }
    }};
}

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Binary addition with dunder fallback.
    pub(super) fn binary_add(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_add,
            "+",
            StaticStrings::DunderAdd,
            Some(StaticStrings::DunderRadd),
            self.interns
        )
    }

    /// Binary subtraction with dunder fallback.
    pub(super) fn binary_sub(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_sub,
            "-",
            StaticStrings::DunderSub,
            Some(StaticStrings::DunderRsub)
        )
    }

    /// Binary multiplication with dunder fallback.
    pub(super) fn binary_mult(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_mult,
            "*",
            StaticStrings::DunderMul,
            Some(StaticStrings::DunderRmul),
            self.interns
        )
    }

    /// Binary division with dunder fallback.
    pub(super) fn binary_div(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_div,
            "/",
            StaticStrings::DunderTruediv,
            Some(StaticStrings::DunderRtruediv),
            self.interns
        )
    }

    /// Binary floor division with dunder fallback.
    pub(super) fn binary_floordiv(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_floordiv,
            "//",
            StaticStrings::DunderFloordiv,
            Some(StaticStrings::DunderRfloordiv)
        )
    }

    /// Binary modulo with dunder fallback.
    pub(super) fn binary_mod(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_mod,
            "%",
            StaticStrings::DunderMod,
            Some(StaticStrings::DunderRmod)
        )
    }

    /// Binary power with dunder fallback.
    #[inline(never)]
    pub(super) fn binary_pow(&mut self) -> Result<CallResult, RunError> {
        binary_op_with_dunder!(
            self,
            py_pow,
            "** or pow()",
            StaticStrings::DunderPow,
            Some(StaticStrings::DunderRpow)
        )
    }

    /// Binary matmul (@) with dunder dispatch.
    pub(super) fn binary_matmul(&mut self) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // No native py_matmul - go straight to dunder
        let dunder_id: crate::intern::StringId = StaticStrings::DunderMatmul.into();
        let reflected_id: Option<crate::intern::StringId> = Some(StaticStrings::DunderRmatmul.into());

        if let Some(result) = self.try_binary_dunder(&lhs, &rhs, dunder_id, reflected_id)? {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Ok(result)
        } else {
            let lhs_type = lhs.py_type(self.heap);
            let rhs_type = rhs.py_type(self.heap);
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Err(ExcType::binary_type_error("@", lhs_type, rhs_type))
        }
    }

    /// Binary bitwise operation with dunder fallback.
    pub(super) fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        match lhs.py_bitwise(&rhs, op, self.heap) {
            Ok(v) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Ok(CallResult::Push(v))
            }
            Err(e) => {
                // Only try dunder dispatch for TypeError (unsupported operand types).
                // Propagate all other errors (MemoryError, ValueError, etc.) immediately.
                let is_type_error = matches!(&e,
                    RunError::Exc(exc) if exc.exc.exc_type() == ExcType::TypeError
                );
                if !is_type_error {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    return Err(e);
                }

                // Try dunder dispatch for instances
                let (dunder, reflected) = match op {
                    BitwiseOp::And => (StaticStrings::DunderAnd, Some(StaticStrings::DunderRand)),
                    BitwiseOp::Or => (StaticStrings::DunderOr, Some(StaticStrings::DunderRor)),
                    BitwiseOp::Xor => (StaticStrings::DunderXor, Some(StaticStrings::DunderRxor)),
                    BitwiseOp::LShift => (StaticStrings::DunderLshift, Some(StaticStrings::DunderRlshift)),
                    BitwiseOp::RShift => (StaticStrings::DunderRshift, Some(StaticStrings::DunderRrshift)),
                };
                let dunder_id: crate::intern::StringId = dunder.into();
                let reflected_id: Option<crate::intern::StringId> = reflected.map(std::convert::Into::into);

                if let Some(result) = self.try_binary_dunder(&lhs, &rhs, dunder_id, reflected_id)? {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    Ok(result)
                } else {
                    let lhs_type = lhs.py_type(self.heap);
                    let rhs_type = rhs.py_type(self.heap);
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    let op_str = match op {
                        BitwiseOp::And => "&",
                        BitwiseOp::Or => "|",
                        BitwiseOp::Xor => "^",
                        BitwiseOp::LShift => "<<",
                        BitwiseOp::RShift => ">>",
                    };
                    Err(ExcType::binary_type_error(op_str, lhs_type, rhs_type))
                }
            }
        }
    }

    /// In-place addition with dunder fallback.
    pub(super) fn inplace_add(&mut self) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let mut lhs_guard = HeapGuard::new(self.pop(), self);
        let (lhs, this) = lhs_guard.as_parts_mut();

        // Try in-place operation first (for mutable types like lists)
        if lhs.py_iadd(rhs.clone_with_heap(this.heap), this.heap, lhs.ref_id(), this.interns)? {
            let (lhs, this) = lhs_guard.into_parts();
            rhs.drop_with_heap(this.heap);
            return Ok(CallResult::Push(lhs));
        }

        // Next try regular addition
        if let Some(v) = lhs.py_add(&rhs, this.heap, this.interns)? {
            rhs.drop_with_heap(this.heap);
            return Ok(CallResult::Push(v));
        }

        // Release the guard before calling dunder (needs &mut self)
        let (lhs, this) = lhs_guard.into_parts();

        // Try dunder dispatch
        let inplace_id: crate::intern::StringId = StaticStrings::DunderIadd.into();
        let dunder_id: crate::intern::StringId = StaticStrings::DunderAdd.into();
        let reflected_id: Option<crate::intern::StringId> = Some(StaticStrings::DunderRadd.into());

        if let Some(result) = this.try_inplace_dunder(&lhs, &rhs, inplace_id, dunder_id, reflected_id)? {
            lhs.drop_with_heap(this.heap);
            rhs.drop_with_heap(this.heap);
            Ok(result)
        } else {
            let lhs_type = lhs.py_type(this.heap);
            let rhs_type = rhs.py_type(this.heap);
            lhs.drop_with_heap(this.heap);
            rhs.drop_with_heap(this.heap);
            Err(ExcType::binary_type_error("+=", lhs_type, rhs_type))
        }
    }

    /// In-place subtraction with dunder fallback.
    pub(super) fn inplace_sub(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_sub,
            py_sub,
            "-=",
            StaticStrings::DunderIsub,
            StaticStrings::DunderSub,
            Some(StaticStrings::DunderRsub)
        )
    }

    /// In-place multiplication with dunder fallback.
    pub(super) fn inplace_mul(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_mult,
            py_mult,
            "*=",
            StaticStrings::DunderImul,
            StaticStrings::DunderMul,
            Some(StaticStrings::DunderRmul),
            self.interns
        )
    }

    /// In-place division with dunder fallback.
    pub(super) fn inplace_div(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_div,
            py_div,
            "/=",
            StaticStrings::DunderItruediv,
            StaticStrings::DunderTruediv,
            Some(StaticStrings::DunderRtruediv),
            self.interns
        )
    }

    /// In-place floor division with dunder fallback.
    pub(super) fn inplace_floordiv(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_floordiv,
            py_floordiv,
            "//=",
            StaticStrings::DunderIfloordiv,
            StaticStrings::DunderFloordiv,
            Some(StaticStrings::DunderRfloordiv)
        )
    }

    /// In-place modulo with dunder fallback.
    pub(super) fn inplace_mod(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_mod,
            py_mod,
            "%=",
            StaticStrings::DunderImod,
            StaticStrings::DunderMod,
            Some(StaticStrings::DunderRmod)
        )
    }

    /// In-place power with dunder fallback.
    pub(super) fn inplace_pow(&mut self) -> Result<CallResult, RunError> {
        inplace_op_with_dunder!(
            self,
            py_pow,
            py_pow,
            "**=",
            StaticStrings::DunderIpow,
            StaticStrings::DunderPow,
            Some(StaticStrings::DunderRpow)
        )
    }

    /// In-place bitwise operation with dunder fallback.
    pub(super) fn inplace_bitwise(&mut self, op: BitwiseOp) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        match lhs.py_bitwise(&rhs, op, self.heap) {
            Ok(v) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Ok(CallResult::Push(v))
            }
            Err(e) => {
                // Only try dunder dispatch for TypeError (unsupported operand types).
                // Propagate all other errors (MemoryError, ValueError, etc.) immediately.
                let is_type_error = matches!(&e,
                    RunError::Exc(exc) if exc.exc.exc_type() == ExcType::TypeError
                );
                if !is_type_error {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    return Err(e);
                }

                // Try dunder dispatch
                let (inplace_dunder, dunder, reflected) = match op {
                    BitwiseOp::And => (
                        StaticStrings::DunderIand,
                        StaticStrings::DunderAnd,
                        Some(StaticStrings::DunderRand),
                    ),
                    BitwiseOp::Or => (
                        StaticStrings::DunderIor,
                        StaticStrings::DunderOr,
                        Some(StaticStrings::DunderRor),
                    ),
                    BitwiseOp::Xor => (
                        StaticStrings::DunderIxor,
                        StaticStrings::DunderXor,
                        Some(StaticStrings::DunderRxor),
                    ),
                    BitwiseOp::LShift => (
                        StaticStrings::DunderIlshift,
                        StaticStrings::DunderLshift,
                        Some(StaticStrings::DunderRlshift),
                    ),
                    BitwiseOp::RShift => (
                        StaticStrings::DunderIrshift,
                        StaticStrings::DunderRshift,
                        Some(StaticStrings::DunderRrshift),
                    ),
                };
                let inplace_id: crate::intern::StringId = inplace_dunder.into();
                let dunder_id: crate::intern::StringId = dunder.into();
                let reflected_id: Option<crate::intern::StringId> = reflected.map(std::convert::Into::into);

                if let Some(result) = self.try_inplace_dunder(&lhs, &rhs, inplace_id, dunder_id, reflected_id)? {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    Ok(result)
                } else {
                    let lhs_type = lhs.py_type(self.heap);
                    let rhs_type = rhs.py_type(self.heap);
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    let op_str = match op {
                        BitwiseOp::And => "&=",
                        BitwiseOp::Or => "|=",
                        BitwiseOp::Xor => "^=",
                        BitwiseOp::LShift => "<<=",
                        BitwiseOp::RShift => ">>=",
                    };
                    Err(ExcType::binary_type_error(op_str, lhs_type, rhs_type))
                }
            }
        }
    }
}
