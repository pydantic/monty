//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{RunError, RunResult},
    expressions::CmpOperator,
    types::RichCmpOp,
    value::Value,
};

impl VM<'_> {
    /// Evaluates a comparison as a boolean without consuming its operands.
    ///
    /// Fused asserts use this path because they need the truth of an expression,
    /// rather than the arbitrary value returned by a direct rich comparison.
    #[inline]
    pub(super) fn cmp_values(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        if let Some(op) = RichCmpOp::from_cmp_operator(op) {
            lhs.py_rich_compare_bool(rhs, op, self)
        } else {
            match op {
                CmpOperator::Is => Ok(lhs.is(rhs)),
                CmpOperator::IsNot => Ok(!lhs.is(rhs)),
                // `in` tests membership of the left operand in the right one.
                CmpOperator::In => rhs.py_contains(lhs, self),
                CmpOperator::NotIn => Ok(!rhs.py_contains(lhs, self)?),
                _ => unreachable!("rich comparisons handled above"),
            }
        }
    }

    /// Pops two operands and pushes their arbitrary rich-comparison result.
    fn compare_rich_op<const OP: u8>(&mut self) -> Result<(), RunError> {
        const { assert!(CmpOperator::from_repr(OP).is_some(), "invalid CmpOperator operand") };
        let cmp_op = CmpOperator::from_repr(OP).expect("invalid CmpOperator operand");
        let op = RichCmpOp::from_cmp_operator(cmp_op).expect("compare_rich_op requires a rich comparison");
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.py_rich_compare(rhs, op, this)?;
        this.push(result);
        Ok(())
    }

    /// Pops two operands and pushes a boolean identity or containment result.
    fn compare_bool_op<const OP: u8>(&mut self) -> Result<(), RunError> {
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

/// Defines specialized entry points for rich-comparison opcodes.
macro_rules! rich_compare_opcodes {
    ($($name:ident => $op:ident,)*) => {
        impl VM<'_> {
            $(
                pub(super) fn $name(&mut self) -> Result<(), RunError> {
                    self.compare_rich_op::<{ CmpOperator::$op.as_operand() }>()
                }
            )*
        }
    };
}

rich_compare_opcodes! {
    compare_eq => Eq,
    compare_ne => NotEq,
    compare_lt => Lt,
    compare_le => LtE,
    compare_gt => Gt,
    compare_ge => GtE,
}

/// Defines specialized entry points for non-rich comparison opcodes.
macro_rules! bool_compare_opcodes {
    ($($name:ident => $op:ident,)*) => {
        impl VM<'_> {
            $(
                pub(super) fn $name(&mut self) -> Result<(), RunError> {
                    self.compare_bool_op::<{ CmpOperator::$op.as_operand() }>()
                }
            )*
        }
    };
}

bool_compare_opcodes! {
    compare_is => Is,
    compare_is_not => IsNot,
    compare_in => In,
    compare_not_in => NotIn,
}
