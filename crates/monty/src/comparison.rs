//! Python rich comparison dispatch and exact builtin scalar comparisons.

use std::cmp::Ordering;

use num_bigint::BigInt;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    types::{
        PyTrait,
        long_int::{bigint_cmp_f64, bigint_cmp_i64, i64_cmp_f64},
    },
    value::Value,
};

/// The six rich comparisons, excluding identity and membership operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RichCompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl RichCompareOp {
    /// The operation to ask the other operand when this side declines.
    pub fn reflected(self) -> Self {
        match self {
            Self::Eq | Self::Ne => self,
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
        }
    }

    /// The class member implementing this one-sided comparison.
    pub fn dunder(self) -> &'static str {
        match self {
            Self::Eq => "__eq__",
            Self::Ne => "__ne__",
            Self::Lt => "__lt__",
            Self::Le => "__le__",
            Self::Gt => "__gt__",
            Self::Ge => "__ge__",
        }
    }

    /// The source spelling used in unsupported-ordering errors.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// Whether a builtin can answer this operation using its equality implementation.
    pub fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }

    /// Adapts builtin equality, preserving a declined operand pairing.
    pub fn equality_result(self, equal: Option<bool>) -> Value {
        debug_assert!(self.is_equality());
        equal.map_or(Value::NotImplemented, |equal| Value::Bool(equal == (self == Self::Eq)))
    }

    /// Applies an operator to a builtin ordering; `None` means numeric NaN, not unsupported types.
    pub fn apply(self, ordering: Option<Ordering>) -> Value {
        Value::Bool(match self {
            Self::Eq => ordering == Some(Ordering::Equal),
            Self::Ne => ordering != Some(Ordering::Equal),
            Self::Lt => ordering == Some(Ordering::Less),
            Self::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
            Self::Gt => ordering == Some(Ordering::Greater),
            Self::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        })
    }
}

impl Value {
    /// Python rich comparison, preserving arbitrary results and honoring reflected dispatch.
    /// Identity is only the final equality fallback after both operands decline.
    pub(crate) fn py_richcompare(&self, other: &Self, op: RichCompareOp, vm: &mut VM<'_>) -> RunResult<Self> {
        let left_type = self.py_type(vm);
        let right_type = other.py_type(vm);
        let right_first = left_type != right_type && right_type.is_instance_of(left_type);
        if right_first {
            let result = other.py_richcompare_impl(self, op.reflected(), vm)?;
            if !result.is_not_implemented() {
                return Ok(result);
            }
        }
        let result = self.py_richcompare_impl(other, op, vm)?;
        if !result.is_not_implemented() {
            return Ok(result);
        }
        if !right_first {
            let result = other.py_richcompare_impl(self, op.reflected(), vm)?;
            if !result.is_not_implemented() {
                return Ok(result);
            }
        }
        match op {
            RichCompareOp::Eq => Ok(Self::Bool(self.is(other))),
            RichCompareOp::Ne => Ok(Self::Bool(!self.is(other))),
            _ => Err(ExcType::type_error_ordering(
                op.symbol(),
                &self.py_type_name(vm),
                &other.py_type_name(vm),
            )),
        }
    }

    /// Truth-tests a rich comparison without an identity shortcut, releasing its owned result.
    pub(crate) fn py_richcompare_bool(&self, other: &Self, op: RichCompareOp, vm: &mut VM<'_>) -> RunResult<bool> {
        let result = self.py_richcompare(other, op, vm)?;
        defer_drop!(result, vm);
        result.py_bool(vm)
    }
}

/// Defines named conveniences without duplicating the comparison protocol.
macro_rules! comparison_methods {
    ($($rich:ident, $boolean:ident => $op:ident;)*) => {
        impl Value {
            $(
                #[doc = concat!("Python `", stringify!($op), "` comparison, preserving arbitrary dunder results.")]
                pub(crate) fn $rich(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
                    self.py_richcompare(other, RichCompareOp::$op, vm)
                }

                #[doc = concat!("Truth-tested `", stringify!($op), "` comparison, without an identity shortcut.")]
                pub(crate) fn $boolean(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
                    self.py_richcompare_bool(other, RichCompareOp::$op, vm)
                }
            )*
        }
    };
}

comparison_methods! {
    py_rich_eq, py_eq_operator => Eq;
    py_rich_ne, py_ne_operator => Ne;
    py_rich_lt, py_lt_operator => Lt;
    py_rich_le, py_le_operator => Le;
    py_rich_gt, py_gt_operator => Gt;
    py_rich_ge, py_ge_operator => Ge;
}

/// A borrowed real number, preserving precision across all integer representations.
pub(crate) enum Number<'a> {
    Int(i64),
    Float(f64),
    BigInt(&'a BigInt),
}

impl Number<'_> {
    /// Compares against any real numeric operand; unrelated types get reflected dispatch.
    pub fn richcompare(self, other: &Value, op: RichCompareOp, vm: &VM<'_>) -> Value {
        let other = match other {
            Value::Bool(b) => Number::Int(i64::from(*b)),
            Value::Int(i) => Number::Int(*i),
            Value::Float(f) => Number::Float(*f),
            Value::InternLongInt(id) => Number::BigInt(vm.interns.get_long_int(*id)),
            Value::Ref(id) if let HeapData::LongInt(i) = vm.heap.get(*id) => Number::BigInt(i.inner()),
            _ => return Value::NotImplemented,
        };
        let ordering = match (self, other) {
            (Number::Int(a), Number::Int(b)) => Some(a.cmp(&b)),
            (Number::Float(a), Number::Float(b)) => a.partial_cmp(&b),
            (Number::Int(a), Number::Float(b)) => i64_cmp_f64(a, b),
            (Number::Float(a), Number::Int(b)) => i64_cmp_f64(b, a).map(Ordering::reverse),
            (Number::BigInt(a), Number::BigInt(b)) => Some(a.cmp(b)),
            (Number::BigInt(a), Number::Int(b)) => Some(bigint_cmp_i64(a, b)),
            (Number::Int(a), Number::BigInt(b)) => Some(bigint_cmp_i64(b, a).reverse()),
            (Number::BigInt(a), Number::Float(b)) => bigint_cmp_f64(a, b),
            (Number::Float(a), Number::BigInt(b)) => bigint_cmp_f64(b, a).map(Ordering::reverse),
        };
        op.apply(ordering)
    }
}

/// Compares a string against either its interned or heap representation.
pub(crate) fn compare_str(s: &str, other: &Value, op: RichCompareOp, vm: &VM<'_>) -> Value {
    match other {
        Value::InternString(id) => op.apply(Some(s.cmp(vm.interns.get_str(*id)))),
        Value::Ref(id) if let HeapData::Str(other) = vm.heap.get(*id) => op.apply(Some(s.cmp(other.as_str()))),
        _ => Value::NotImplemented,
    }
}

/// Compares bytes against either their interned or heap representation.
pub(crate) fn compare_bytes(b: &[u8], other: &Value, op: RichCompareOp, vm: &VM<'_>) -> Value {
    match other {
        Value::InternBytes(id) => op.apply(Some(b.cmp(vm.interns.get_bytes(*id)))),
        Value::Ref(id) if let HeapData::Bytes(other) = vm.heap.get(*id) => op.apply(Some(b.cmp(other.as_slice()))),
        _ => Value::NotImplemented,
    }
}
