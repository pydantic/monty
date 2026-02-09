//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    exception_private::{ExcType, RunError},
    io::PrintWriter,
    resource::{DepthGuard, ResourceTracker},
    types::{LongInt, PyTrait},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Equality comparison.
    pub(super) fn compare_eq(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let mut guard = DepthGuard::default();
        let result = lhs.py_eq(&rhs, self.heap, &mut guard, self.interns)?;
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
        Ok(())
    }

    /// Inequality comparison.
    pub(super) fn compare_ne(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let mut guard = DepthGuard::default();
        let result = !lhs.py_eq(&rhs, self.heap, &mut guard, self.interns)?;
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
        Ok(())
    }

    /// Ordering comparison with a predicate.
    pub(super) fn compare_ord<F>(&mut self, check: F) -> Result<(), RunError>
    where
        F: FnOnce(std::cmp::Ordering) -> bool,
    {
        let rhs = self.pop();
        let lhs = self.pop();
        let mut guard = DepthGuard::default();
        let result = lhs
            .py_cmp(&rhs, self.heap, &mut guard, self.interns)?
            .is_some_and(check);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
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
        let rhs = self.pop();
        let lhs = self.pop();

        let result = lhs.is(&rhs);

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in).
    pub(super) fn compare_in(&mut self, negate: bool) -> Result<(), RunError> {
        let container = self.pop(); // container (rhs)
        let item = self.pop(); // item to find (lhs)

        let result = container.py_contains(&item, self.heap, self.interns);

        item.drop_with_heap(self.heap);
        container.drop_with_heap(self.heap);

        let contained = result?;
        self.push(Value::Bool(if negate { !contained } else { contained }));
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
        let rhs = self.pop(); // divisor (b)
        let lhs = self.pop(); // dividend (a)

        // Try fast path for Int/Float types
        let mod_result = match k {
            Value::Int(k_val) => lhs.py_mod_eq(&rhs, *k_val),
            _ => None,
        };

        if let Some(is_equal) = mod_result {
            // Fast path succeeded
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            self.push(Value::Bool(is_equal));
            Ok(())
        } else {
            // Fallback: compute py_mod then compare with py_eq
            // This handles LongInt and other Ref types
            let mod_value = lhs.py_mod(&rhs, self.heap);
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);

            match mod_value {
                Ok(Some(v)) => {
                    // Handle InternLongInt by converting to heap LongInt for comparison
                    let (k_value, k_needs_drop) = if let Value::InternLongInt(id) = k {
                        let bi = self.interns.get_long_int(*id).clone();
                        (LongInt::new(bi).into_value(self.heap)?, true)
                    } else {
                        (k.copy_for_extend(), false)
                    };

                    let mut guard = DepthGuard::default();
                    let is_equal = v.py_eq(&k_value, self.heap, &mut guard, self.interns)?;
                    v.drop_with_heap(self.heap);
                    if k_needs_drop {
                        k_value.drop_with_heap(self.heap);
                    }
                    self.push(Value::Bool(is_equal));
                    Ok(())
                }
                Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for %")),
                Err(e) => Err(e),
            }
        }
    }
}
