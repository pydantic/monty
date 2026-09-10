//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    expressions::CmpOperator,
    types::{CmpOrder, PyTrait},
    value::Value,
};

impl VM<'_> {
    /// Evaluates a comparison as a boolean without consuming its operands.
    ///
    /// Shared by fused asserts and comparison helpers that need truth rather
    /// than the arbitrary value a direct `==` expression may produce.
    #[inline]
    pub(super) fn cmp_values(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        match op {
            CmpOperator::Eq | CmpOperator::NotEq => {
                let result = lhs.py_rich_eq(rhs, self)?;
                let is_equal = result.py_bool(self);
                result.drop_with(self);
                let is_equal = is_equal?;
                Ok(if op == CmpOperator::NotEq { !is_equal } else { is_equal })
            }
            CmpOperator::Is => Ok(lhs.is(rhs)),
            CmpOperator::IsNot => Ok(!lhs.is(rhs)),
            // `in` tests membership of the *left* operand in the right one.
            CmpOperator::In => rhs.py_contains(lhs, self),
            CmpOperator::NotIn => Ok(!rhs.py_contains(lhs, self)?),
            CmpOperator::Lt | CmpOperator::LtE | CmpOperator::Gt | CmpOperator::GtE => self.cmp_ordering(op, lhs, rhs),
        }
    }

    /// Executes direct `==`, preserving an arbitrary value returned by `__eq__`.
    pub(super) fn compare_eq(&mut self) -> Result<(), RunError> {
        let this = self;
        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.py_rich_eq(rhs, this)?;
        this.push(result);
        Ok(())
    }

    /// Executes direct `!=`; user `__ne__` dispatch is not yet supported.
    pub(super) fn compare_ne(&mut self) -> Result<(), RunError> {
        let this = self;
        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.py_rich_eq(rhs, this)?;
        defer_drop!(result, this);
        let is_not_equal = !result.py_bool(this)?;
        this.push(Value::Bool(is_not_equal));
        Ok(())
    }

    /// Evaluates an ordering comparison, preserving CPython's behavior for
    /// unordered values such as `NaN` and incomparable operand types.
    #[inline]
    fn cmp_ordering(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        // A type whose ordering no `CmpOrder` describes (a `Counter` compares as
        // a multiset) answers the operator itself. Hooked in here rather than at
        // the opcode so the fused-assert path, which calls `cmp_values` directly,
        // gets the same semantics.
        if let Some(result) = lhs.py_cmp_op(rhs, op, self)? {
            return Ok(result);
        }
        match lhs.py_cmp(rhs, self)? {
            CmpOrder::Ordered(ordering) => Ok(match op {
                CmpOperator::Lt => ordering.is_lt(),
                CmpOperator::LtE => ordering.is_le(),
                CmpOperator::Gt => ordering.is_gt(),
                CmpOperator::GtE => ordering.is_ge(),
                // `cmp_values` calls this only for ordering operators.
                _ => unreachable!("cmp_ordering reached with a non-ordering operator"),
            }),
            CmpOrder::Unordered => Ok(false),
            CmpOrder::Incomparable => {
                let left_type = lhs.py_type_name(self);
                let right_type = rhs.py_type_name(self);
                Err(ExcType::type_error_ordering(op.as_str(), &left_type, &right_type))
            }
        }
    }

    /// Pops both operands and pushes a boolean comparison result.
    /// The const operator lets dispatch specialize the implementation per opcode.
    fn compare_op<const OP: u8>(&mut self) -> Result<(), RunError> {
        // Rejects a bad `OP` at compile time, which makes the `else` dead.
        const { assert!(CmpOperator::from_repr(OP).is_some(), "invalid CmpOperator operand") };
        let op = CmpOperator::from_repr(OP).expect("invalid CmpOperator operand");
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = this.cmp_values(op, lhs, rhs)?;
        this.push(Value::Bool(result));
        Ok(())
    }
}

/// Defines a specialized entry point for each boolean comparison opcode.
macro_rules! compare_opcodes {
    ($($name:ident => $op:ident,)*) => {
        impl VM<'_> {
            $(
                pub(super) fn $name(&mut self) -> Result<(), RunError> {
                    self.compare_op::<{ CmpOperator::$op.as_operand() }>()
                }
            )*
        }
    };
}

compare_opcodes! {
    compare_lt => Lt,
    compare_le => LtE,
    compare_gt => Gt,
    compare_ge => GtE,
    compare_is => Is,
    compare_is_not => IsNot,
    compare_in => In,
    compare_not_in => NotIn,
}
