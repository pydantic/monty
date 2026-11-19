//! Payload-level arithmetic shared by immediate and arbitrary-precision integers.
//! Static dispatch specializes the small-integer fast paths without another value enum.

use std::borrow::Cow;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

use super::{
    LongInt, bigint_divmod_tuple, bigint_to_f64_checked, bigint_true_divide, integer_value, long_int_pow_value,
    wide_i128_into_value,
};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    resource_checks::{check_div_size, check_lshift_size, check_pow_size},
    value::{
        Value, float_divmod_tuple, float_pow_value, floor_divmod, int_divmod_tuple, py_float_divmod, py_float_mod,
    },
};

/// Integer arithmetic on borrowed payloads; none of these operations may re-enter Python.
/// Implementations for `i64` and `BigInt` specialize away the storage tests.
#[expect(
    clippy::inline_always,
    reason = "hot operators inline so the `i64` instantiation's small-int path has no call overhead"
)]
pub(super) trait IntArithmetic {
    /// Returns an immediate payload without allocating a temporary big integer.
    fn small(&self) -> Option<i64>;
    /// Borrows big digits, promoting small values only on the arbitrary-precision path.
    fn big(&self) -> Cow<'_, BigInt>;
    /// Converts mixed float operands, rejecting integer overflow.
    fn float(&self) -> RunResult<f64>;

    /// Subtracts in operand order, promoting only on overflow or a big operand.
    #[inline(always)]
    fn sub(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            Ok(Some(match lhs.checked_sub(rhs) {
                Some(value) => Value::Int(value),
                None => wide_i128_into_value(i128::from(lhs) - i128::from(rhs), vm.heap),
            }))
        } else if let Value::Float(rhs) = other {
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            Ok(Some(Value::Float(lhs - rhs)))
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            Ok(Some(LongInt::new(lhs - rhs).into_value(vm.heap)))
        } else {
            Ok(None)
        }
    }

    /// Divides exact integers with one rounding step, including beyond the float range.
    #[inline(always)]
    fn truediv(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            if rhs == 0 {
                return Err(ExcType::zero_division().into());
            }
            if lhs.unsigned_abs() <= 1 << f64::MANTISSA_DIGITS && rhs.unsigned_abs() <= 1 << f64::MANTISSA_DIGITS {
                return Ok(Some(Value::Float(lhs as f64 / rhs as f64)));
            }
        }
        if let Value::Float(rhs) = other {
            check_float_divisor(self, *rhs, reflected)?;
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            Ok(Some(Value::Float(lhs / rhs)))
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            bigint_true_divide(lhs, rhs, &vm.heap.tracker).map(|value| Some(Value::Float(value)))
        } else {
            Ok(None)
        }
    }

    /// Computes floor division with an allocation-free machine-integer fast path.
    #[inline(always)]
    fn floordiv(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            if rhs == 0 {
                return Err(ExcType::zero_division().into());
            }
            Ok(Some(match floor_divmod(lhs, rhs) {
                Some((quotient, _)) => Value::Int(quotient),
                None => wide_i128_into_value(i128::from(lhs).div_euclid(i128::from(rhs)), vm.heap),
            }))
        } else if let Value::Float(rhs) = other {
            check_float_divisor(self, *rhs, reflected)?;
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            Ok(Some(Value::Float(py_float_divmod(lhs, rhs).0)))
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            if rhs.is_zero() {
                return Err(ExcType::zero_division().into());
            }
            check_div_size(lhs.bits(), &vm.heap.tracker)?;
            Ok(Some(LongInt::new(lhs.div_floor(rhs)).into_value(vm.heap)))
        } else {
            Ok(None)
        }
    }

    /// Computes the remainder with the divisor's sign, including signed float zero.
    #[inline(always)]
    fn modulo(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            if rhs == 0 {
                return Err(ExcType::zero_division().into());
            }
            Ok(Some(Value::Int(match lhs.checked_rem(rhs) {
                Some(rem) if rem != 0 && (lhs < 0) != (rhs < 0) => rem + rhs,
                Some(rem) => rem,
                None => 0,
            })))
        } else if let Value::Float(rhs) = other {
            check_float_divisor(self, *rhs, reflected)?;
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            Ok(Some(Value::Float(py_float_mod(lhs, rhs))))
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            if rhs.is_zero() {
                return Err(ExcType::zero_division().into());
            }
            Ok(Some(LongInt::new(lhs.mod_floor(rhs)).into_value(vm.heap)))
        } else {
            Ok(None)
        }
    }

    /// Computes quotient and remainder together using division's promotion rules.
    fn divmod(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            int_divmod_tuple(lhs, rhs, vm.heap).map(Some)
        } else if let Value::Float(rhs) = other {
            check_float_divisor(self, *rhs, reflected)?;
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            float_divmod_tuple(lhs, rhs, vm.heap).map(Some)
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            if rhs.is_zero() {
                return Err(ExcType::zero_division().into());
            }
            check_div_size(lhs.bits(), &vm.heap.tracker)?;
            Ok(Some(bigint_divmod_tuple(lhs, rhs, vm.heap)))
        } else {
            Ok(None)
        }
    }

    /// Keeps machine-sized powers inline and preflights amplified big results.
    fn pow(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(base), Some(exp)) = (self.small(), small_int(other)) {
            let (base, exp) = ordered(base, exp, reflected);
            if exp < 0 {
                return float_pow_value(base as f64, exp as f64, vm.heap).map(Some);
            }
            if let Ok(exp) = u32::try_from(exp) {
                let result = if let Some(result) = base.checked_pow(exp) {
                    Value::Int(result)
                } else if let Some(result) = i128::from(base).checked_pow(exp) {
                    wide_i128_into_value(result, vm.heap)
                } else {
                    check_pow_size(
                        u64::from(64 - base.unsigned_abs().leading_zeros()),
                        u64::from(exp),
                        &vm.heap.tracker,
                    )?;
                    LongInt::new(BigInt::from(base).pow(exp)).into_value(vm.heap)
                };
                return Ok(Some(result));
            }
        }
        if let Value::Float(rhs) = other {
            let (lhs, rhs) = ordered(self.float()?, *rhs, reflected);
            float_pow_value(lhs, rhs, vm.heap).map(Some)
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            long_int_pow_value(lhs, rhs, vm.heap)
        } else {
            Ok(None)
        }
    }

    /// Shares exact bitwise arithmetic while rejecting non-integer operands.
    fn bitwise(
        &self,
        other: &Value,
        vm: &VM<'_>,
        small: impl FnOnce(i64, i64) -> i64,
        big: impl FnOnce(&BigInt, &BigInt) -> BigInt,
    ) -> Option<Value> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            Some(Value::Int(small(lhs, rhs)))
        } else {
            let rhs = integer_value(other, vm)?;
            Some(LongInt::new(big(&self.big(), &rhs)).into_value(vm.heap))
        }
    }

    /// Validates the shift before preflighting an amplified allocation.
    fn lshift(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            if rhs < 0 {
                return Err(ExcType::value_error_negative_shift_count());
            }
            LongInt::left_shift_i64(lhs, rhs.cast_unsigned(), vm).map(Some)
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            if rhs.is_negative() {
                return Err(ExcType::value_error_negative_shift_count());
            }
            let shift = rhs.to_u64().ok_or_else(ExcType::overflow_c_ssize_t)?;
            check_lshift_size(lhs.bits(), shift, &vm.heap.tracker)?;
            Ok(Some(LongInt::new(lhs << shift).into_value(vm.heap)))
        } else {
            Ok(None)
        }
    }

    /// Right shifts saturate small integers after all significant bits are shifted out.
    fn rshift(&self, other: &Value, vm: &VM<'_>, reflected: bool) -> RunResult<Option<Value>> {
        let small = if reflected { small_int(other) } else { self.small() };
        if let (Some(lhs), Some(rhs)) = (self.small(), small_int(other)) {
            let (lhs, rhs) = ordered(lhs, rhs, reflected);
            if rhs < 0 {
                return Err(ExcType::value_error_negative_shift_count());
            }
            Ok(Some(Value::Int(shift_small(lhs, u32::try_from(rhs).ok()))))
        } else if let Some(rhs) = integer_value(other, vm) {
            let lhs = self.big();
            let (lhs, rhs) = ordered(lhs.as_ref(), rhs.as_ref(), reflected);
            if rhs.is_negative() {
                return Err(ExcType::value_error_negative_shift_count());
            }
            if let Some(lhs) = small {
                Ok(Some(Value::Int(shift_small(lhs, rhs.to_u32()))))
            } else {
                let shift = rhs.to_u64().ok_or_else(ExcType::overflow_c_ssize_t)?;
                Ok(Some(LongInt::new(lhs >> shift).into_value(vm.heap)))
            }
        } else {
            Ok(None)
        }
    }
}

impl IntArithmetic for i64 {
    fn small(&self) -> Option<i64> {
        Some(*self)
    }
    fn big(&self) -> Cow<'_, BigInt> {
        Cow::Owned(BigInt::from(*self))
    }
    fn float(&self) -> RunResult<f64> {
        Ok(*self as f64)
    }
}

impl IntArithmetic for BigInt {
    fn small(&self) -> Option<i64> {
        None
    }
    fn big(&self) -> Cow<'_, BigInt> {
        Cow::Borrowed(self)
    }
    fn float(&self) -> RunResult<f64> {
        bigint_to_f64_checked(self)
    }
}

/// Applies a float operation after converting only builtin numeric operands.
pub(crate) fn float_binary(
    lhs: f64,
    other: &Value,
    vm: &VM<'_>,
    reflected: bool,
    operation: impl FnOnce(f64, f64) -> RunResult<Value>,
) -> RunResult<Option<Value>> {
    let rhs = match other {
        Value::Int(value) => *value as f64,
        Value::Bool(value) => f64::from(*value),
        Value::Float(value) => *value,
        Value::InternLongInt(id) => bigint_to_f64_checked(vm.interns.get_long_int(*id))?,
        Value::Ref(id) if let HeapData::LongInt(value) = vm.heap.get(*id) => value.to_f64_checked()?,
        _ => return Ok(None),
    };
    let (lhs, rhs) = ordered(lhs, rhs, reflected);
    operation(lhs, rhs).map(Some)
}

/// Rejects a zero divisor before a potentially overflowing integer-to-float conversion.
fn check_float_divisor(lhs: &(impl IntArithmetic + ?Sized), rhs: f64, reflected: bool) -> RunResult<()> {
    let zero = if reflected {
        lhs.small().map_or_else(|| lhs.big().is_zero(), |value| value == 0)
    } else {
        rhs == 0.0
    };
    if zero {
        Err(ExcType::zero_division().into())
    } else {
        Ok(())
    }
}

/// Extracts small integer operands without allocating or invoking `__index__`.
fn small_int(value: &Value) -> Option<i64> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

/// Preserves source operand order for reflected arithmetic.
fn ordered<T>(lhs: T, rhs: T, reflected: bool) -> (T, T) {
    if reflected { (rhs, lhs) } else { (lhs, rhs) }
}

/// Sign-extends a machine integer when the shift exceeds its width.
fn shift_small(value: i64, shift: Option<u32>) -> i64 {
    shift
        .filter(|shift| *shift < 64)
        .map_or_else(|| if value < 0 { -1 } else { 0 }, |shift| value >> shift)
}
