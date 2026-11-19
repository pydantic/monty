//! Comparison operation helpers for the VM.

use std::cmp::Ordering;

use super::VM;
use crate::{
    defer_drop,
    exception_private::{RunError, RunResult},
    expressions::CmpOperator,
    types::long_int::i64_cmp_f64,
    value::Value,
};

impl VM<'_> {
    /// Evaluates a comparison as a boolean without consuming its operands.
    /// Fused asserts truth-test rich results exactly as an ordinary branch does.
    #[inline]
    pub(crate) fn cmp_values(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        match op {
            CmpOperator::Eq => lhs.py_eq_operator(rhs, self),
            CmpOperator::NotEq => lhs.py_ne_operator(rhs, self),
            CmpOperator::Lt => lhs.py_lt_operator(rhs, self),
            CmpOperator::LtE => lhs.py_le_operator(rhs, self),
            CmpOperator::Gt => lhs.py_gt_operator(rhs, self),
            CmpOperator::GtE => lhs.py_ge_operator(rhs, self),
            CmpOperator::Is => Ok(lhs.is(rhs)),
            CmpOperator::IsNot => Ok(!lhs.is(rhs)),
            CmpOperator::In => rhs.py_contains(lhs, self),
            CmpOperator::NotIn => Ok(!rhs.py_contains(lhs, self)?),
        }
    }

    /// Compares immediate int/float pairs without owning heap references.
    /// The callback receives `None` for NaN operands.
    #[inline]
    fn compare_numeric(&mut self, compare: impl FnOnce(Option<Ordering>) -> bool) -> bool {
        let len = self.stack.len();
        let ordering = match (&self.stack[len - 2], &self.stack[len - 1]) {
            (Value::Int(lhs), Value::Int(rhs)) => Some(lhs.cmp(rhs)),
            (Value::Float(lhs), Value::Float(rhs)) => lhs.partial_cmp(rhs),
            // Casting integers to floats would lose precision above 2**53.
            (Value::Int(lhs), Value::Float(rhs)) => i64_cmp_f64(*lhs, *rhs),
            (Value::Float(lhs), Value::Int(rhs)) => i64_cmp_f64(*rhs, *lhs).map(Ordering::reverse),
            _ => return false,
        };
        let result = compare(ordering);

        self.stack[len - 2] = Value::Bool(result);
        self.stack.truncate(len - 1);
        true
    }

    /// Pops two operands and preserves rich comparison results; identity and
    /// membership are always boolean. The const operand specializes each opcode.
    fn compare_op<const OP: u8>(&mut self) -> Result<(), RunError> {
        const { assert!(CmpOperator::from_repr(OP).is_some(), "invalid CmpOperator operand") };
        let op = CmpOperator::from_repr(OP).expect("invalid CmpOperator operand");
        let handled = match op {
            CmpOperator::Eq => self.compare_numeric(|ordering| ordering == Some(Ordering::Equal)),
            CmpOperator::NotEq => self.compare_numeric(|ordering| ordering != Some(Ordering::Equal)),
            CmpOperator::Lt => self.compare_numeric(|ordering| ordering.is_some_and(Ordering::is_lt)),
            CmpOperator::LtE => self.compare_numeric(|ordering| ordering.is_some_and(Ordering::is_le)),
            CmpOperator::Gt => self.compare_numeric(|ordering| ordering.is_some_and(Ordering::is_gt)),
            CmpOperator::GtE => self.compare_numeric(|ordering| ordering.is_some_and(Ordering::is_ge)),
            _ => false,
        };
        if handled {
            return Ok(());
        }

        let this = self;
        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = match op {
            CmpOperator::Eq => lhs.py_rich_eq(rhs, this)?,
            CmpOperator::NotEq => lhs.py_rich_ne(rhs, this)?,
            CmpOperator::Lt => lhs.py_rich_lt(rhs, this)?,
            CmpOperator::LtE => lhs.py_rich_le(rhs, this)?,
            CmpOperator::Gt => lhs.py_rich_gt(rhs, this)?,
            CmpOperator::GtE => lhs.py_rich_ge(rhs, this)?,
            _ => Value::Bool(this.cmp_values(op, lhs, rhs)?),
        };
        this.push(result);
        Ok(())
    }
}

/// Defines a specialized entry point for each comparison opcode.
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
    compare_eq => Eq,
    compare_ne => NotEq,
    compare_lt => Lt,
    compare_le => LtE,
    compare_gt => Gt,
    compare_ge => GtE,
    compare_is => Is,
    compare_is_not => IsNot,
    compare_in => In,
    compare_not_in => NotIn,
}
