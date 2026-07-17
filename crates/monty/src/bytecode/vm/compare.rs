//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    expressions::CmpOperator,
    resource::ResourceTracker,
    types::{CmpOrder, PyTrait},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Evaluates a comparison without consuming its operands.
    /// Shared by `Compare*` opcodes and fused asserts to keep their semantics aligned.
    #[inline]
    pub(super) fn cmp_values(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        match op {
            CmpOperator::Eq => lhs.py_eq(rhs, self),
            CmpOperator::NotEq => Ok(!lhs.py_eq(rhs, self)?),
            CmpOperator::Is => Ok(lhs.is(rhs, self)),
            CmpOperator::IsNot => Ok(!lhs.is(rhs, self)),
            // `in` tests membership of the *left* operand in the right one.
            CmpOperator::In => rhs.py_contains(lhs, self),
            CmpOperator::NotIn => Ok(!rhs.py_contains(lhs, self)?),
            CmpOperator::Lt | CmpOperator::LtE | CmpOperator::Gt | CmpOperator::GtE => self.cmp_ordering(op, lhs, rhs),
        }
    }

    /// Evaluates an ordering comparison, preserving CPython's behavior for
    /// unordered values such as `NaN` and incomparable operand types.
    #[inline]
    fn cmp_ordering(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
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

    /// Pops both operands and pushes the comparison result.
    /// The const operator lets dispatch specialize the implementation per opcode.
    fn compare_op<const OP: u8>(&mut self) -> Result<(), RunError> {
        // Rejects a bad `OP` at compile time, which makes the `else` dead.
        const { assert!(CmpOperator::from_operand(OP).is_some(), "invalid CmpOperator operand") };
        let op = CmpOperator::from_operand(OP).expect("invalid CmpOperator operand");
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = this.cmp_values(op, lhs, rhs)?;
        this.push(Value::Bool(result));
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

/// Defines a specialized entry point for each comparison opcode.
macro_rules! compare_opcodes {
    ($($name:ident => $op:ident,)*) => {
        impl<T: ResourceTracker> VM<'_, T> {
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
