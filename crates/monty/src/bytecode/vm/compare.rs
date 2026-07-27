//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    expressions::CmpOperator,
    heap::HeapData,
    modules::collections::counter::{CounterCmp, counter_compare},
    types::{CmpOrder, PyTrait},
    value::Value,
};

impl VM<'_> {
    /// Evaluates a comparison without consuming its operands.
    /// Shared by `Compare*` opcodes and fused asserts to keep their semantics aligned.
    #[inline]
    pub(super) fn cmp_values(&mut self, op: CmpOperator, lhs: &Value, rhs: &Value) -> RunResult<bool> {
        match op {
            // The bare operator, so `py_eq_operator`: unlike container
            // comparison it must not shortcut `x == x` past a user `__eq__`.
            CmpOperator::Eq => lhs.py_eq_operator(rhs, self),
            CmpOperator::NotEq => Ok(!lhs.py_eq_operator(rhs, self)?),
            CmpOperator::Is => Ok(lhs.is(rhs)),
            CmpOperator::IsNot => Ok(!lhs.is(rhs)),
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
        // Two Counters compare as multisets rather than by ordering. Hooked in
        // here rather than at the opcode so the fused-assert path, which calls
        // `cmp_values` directly, gets the same semantics.
        if let Some(result) = self.counter_compare_op(lhs, rhs, op)? {
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

    /// Handles a multiset comparison when both operands are Counters.
    ///
    /// Any other operand pair returns `None` so the caller falls through to the
    /// ordinary ordering path — CPython's `Counter.__lt__` returns
    /// `NotImplemented` for a non-Counter, which is why `Counter(a=1) < {'a': 2}`
    /// ends as a `TypeError` rather than a dict comparison.
    fn counter_compare_op(&mut self, lhs: &Value, rhs: &Value, op: CmpOperator) -> RunResult<Option<bool>> {
        let cmp = match op {
            CmpOperator::Lt => CounterCmp::Lt,
            CmpOperator::LtE => CounterCmp::Le,
            CmpOperator::Gt => CounterCmp::Gt,
            CmpOperator::GtE => CounterCmp::Ge,
            // Only reached from `cmp_ordering`, which handles ordering operators.
            _ => return Ok(None),
        };
        let (Value::Ref(l), Value::Ref(r)) = (lhs, rhs) else {
            return Ok(None);
        };
        let (l, r) = (*l, *r);
        let both_counters = matches!(self.heap.get(l), HeapData::Dict(d) if d.is_counter())
            && matches!(self.heap.get(r), HeapData::Dict(d) if d.is_counter());
        if both_counters {
            Ok(Some(counter_compare(l, r, cmp, self)?))
        } else {
            Ok(None)
        }
    }

    /// Pops both operands and pushes the comparison result.
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
