//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    exception_private::{ExcType, RunError},
    io::PrintWriter,
    resource::ResourceTracker,
    types::PyTrait,
    value::BitwiseOp,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Binary addition with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths to avoid
    /// overhead on the success path (99%+ of operations).
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_add(&rhs, self.heap, self.interns);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("+", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e.into())
            }
        }
    }

    /// Binary subtraction with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_sub(&rhs, self.heap);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("-", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e.into())
            }
        }
    }

    /// Binary multiplication with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mult(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_mult(&rhs, self.heap, self.interns);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("*", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Binary division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_div(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_div(&rhs, self.heap);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("/", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Binary floor division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_floordiv(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_floordiv(&rhs, self.heap);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("//", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Binary modulo with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mod(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        if let Some(v) = lhs.py_mod(&rhs) {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            self.push(v);
            Ok(())
        } else {
            let lhs_type = lhs.py_type(self.heap);
            let rhs_type = rhs.py_type(self.heap);
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Err(ExcType::binary_type_error("%", lhs_type, rhs_type))
        }
    }

    /// Binary power with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_pow(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_pow(&rhs, self.heap);
        match result {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                self.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(self.heap);
                let rhs_type = rhs.py_type(self.heap);
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(ExcType::binary_type_error("** or pow()", lhs_type, rhs_type))
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Binary bitwise operation on integers.
    ///
    /// Pops two values, performs the bitwise operation, and pushes the result.
    pub(super) fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Compute result before dropping operands (py_bitwise only reads values)
        let result = lhs.py_bitwise(&rhs, op, self.heap);

        // Drop operands before propagating error
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);

        self.push(result?);
        Ok(())
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// For mutable types like lists, `py_iadd` mutates in place and returns true.
    /// For immutable types, we fall back to regular addition.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let mut lhs = self.pop();

        // Try in-place operation first (for mutable types like lists)
        // py_iadd takes owned `other` and mutates `self` in place
        let lhs_id = lhs.ref_id();

        let succeeded = match lhs.py_iadd(rhs.clone_with_heap(self.heap), self.heap, lhs_id, self.interns) {
            Ok(s) => s,
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                return Err(e.into());
            }
        };

        if succeeded {
            // In-place operation succeeded - drop rhs and push lhs back
            rhs.drop_with_heap(self.heap);
            self.push(lhs);
            Ok(())
        } else {
            // Fall back to regular addition
            let result = lhs.py_add(&rhs, self.heap, self.interns);
            match result {
                Ok(Some(v)) => {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    self.push(v);
                    Ok(())
                }
                Ok(None) => {
                    let lhs_type = lhs.py_type(self.heap);
                    let rhs_type = rhs.py_type(self.heap);
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    Err(ExcType::binary_type_error("+=", lhs_type, rhs_type))
                }
                Err(e) => {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    Err(e.into())
                }
            }
        }
    }
}
