//! Comparison operation helpers for the VM.

use std::cmp::Ordering;

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError},
    resource::ResourceTracker,
    types::{CmpOrder, PyTrait},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Equality comparison.
    pub(super) fn compare_eq(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

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

        let result = !lhs.py_eq(rhs, this)?;
        this.push(Value::Bool(result));
        Ok(())
    }

    /// Ordering comparison (`<`, `<=`, `>`, `>=`) with a predicate.
    ///
    /// `operator` is the source symbol, used only for the error message when the
    /// operands are of incomparable types. The three [`CmpOrder`] outcomes map
    /// to CPython behaviour:
    /// - [`Ordered`](CmpOrder::Ordered) — apply `check` to the ordering.
    /// - [`Unordered`](CmpOrder::Unordered) — a `NaN` is involved
    ///   (`float('nan') < 1`, `[nan] < [1]`, …); CPython yields `False` for
    ///   every ordering operator, so push `False` rather than raising.
    /// - [`Incomparable`](CmpOrder::Incomparable) — `1 < 'a'`, `None < None`,
    ///   user-class instances without comparison dunders, etc.; raise
    ///   `TypeError: '{op}' not supported between instances of ...`.
    pub(super) fn compare_ord<F>(&mut self, operator: &str, check: F) -> Result<(), RunError>
    where
        F: FnOnce(Ordering) -> bool,
    {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_cmp(rhs, this)? {
            CmpOrder::Ordered(ordering) => {
                this.push(Value::Bool(check(ordering)));
                Ok(())
            }
            CmpOrder::Unordered => {
                this.push(Value::Bool(false));
                Ok(())
            }
            CmpOrder::Incomparable => {
                let left_type = lhs.py_type_name(this);
                let right_type = rhs.py_type_name(this);
                Err(ExcType::type_error_ordering(operator, &left_type, &right_type))
            }
        }
    }

    /// Identity comparison (is/is not).
    pub(super) fn compare_is(&mut self, negate: bool) {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.is(rhs, this);
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

    /// Executes the legacy modulo-equality opcode as its component operations.
    ///
    /// TODO: remove this opcode once serialized bytecode compatibility no longer
    /// requires it; new compilation should emit the three ordinary operations.
    pub(super) fn compare_mod_eq(&mut self, k: &Value) -> Result<(), RunError> {
        let this = self;

        this.binary_mod()?;
        this.push(k.clone_with_heap(this.heap));
        this.compare_eq()
    }
}
