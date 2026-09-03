//! LongInt wrapper for arbitrary precision integer support.
//!
//! This module provides the `LongInt` wrapper type around `num_bigint::BigInt`.
//! Named `LongInt` to avoid confusion with the external `BigInt` type. Python has
//! one `int` type, and LongInt is an implementation detail - we use i64 for performance
//! when values fit, and promote to LongInt on overflow.
//!
//! The design centralizes BigInt-related logic into methods on `LongInt` rather than
//! having freestanding functions scattered across the codebase.

use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::{self, Display, Write},
    ops::{Add, Mul, Neg, Sub},
    sync::OnceLock,
};

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{FromPrimitive, One, Signed, ToPrimitive, Zero};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_python_long_int},
    heap::{Heap, HeapData, HeapObjectRead, HeapRead},
    resource_checks::{check_div_size, check_lshift_size, check_mult_size, check_pow_size},
    types::{LazyHeapSet, PyTrait, Type, str::allocate_string},
    value::{Value, eq_bigint},
};

/// Maximum number of decimal digits allowed for integer-string conversion.
///
/// Matches CPython 3.11+'s `sys.int_max_str_digits` default (4300).
/// This limit prevents O(n^2) DoS attacks when converting very large integers
/// to/from decimal strings. The limit only applies to base-10 conversions;
/// bin/hex/oct use O(n) algorithms and are unrestricted.
///
/// This is a hardcoded safety limit, not configurable from Python code.
pub(crate) const INT_MAX_STR_DIGITS: usize = 4300;

/// Cached decimal threshold used for `INT_MAX_STR_DIGITS` comparisons.
///
/// Any integer with absolute value greater than or equal to `10**4300` has more
/// than 4300 decimal digits and must raise before string conversion.
static INT_MAX_STR_DIGITS_THRESHOLD: OnceLock<BigInt> = OnceLock::new();

/// Wrapper around `num_bigint::BigInt` for arbitrary precision integers.
///
/// Named `LongInt` to avoid confusion with the external `BigInt` type from `num_bigint`.
/// The inner `BigInt` is accessible via `.0` for arithmetic operations that need direct
/// access to the underlying type.
///
/// Python treats all integers as one type - we use `Value::Int(i64)` for values that fit
/// and `LongInt` for larger values. The `into_value()` method automatically demotes to
/// i64 when the value fits, maintaining this optimization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct LongInt(pub BigInt);

/// Allocates an `i128` already known not to fit the immediate integer representation.
pub(crate) fn wide_i128_into_value(value: i128, heap: &Heap) -> Value {
    debug_assert!(i64::try_from(value).is_err());
    let id = heap.allocate(HeapData::LongInt(LongInt::new(BigInt::from(value))));
    Value::Ref(id)
}

impl LongInt {
    /// Creates a new `LongInt` from a `BigInt`.
    pub fn new(bi: BigInt) -> Self {
        Self(bi)
    }

    /// Converts a nonnegative `u128` to its most compact Python integer representation.
    ///
    /// The common immediate path avoids constructing a temporary `BigInt`.
    pub(crate) fn value_from_u128(value: u128, heap: &Heap) -> Value {
        if let Ok(value) = i64::try_from(value) {
            Value::Int(value)
        } else {
            let long_int = Self::new(BigInt::from(value));
            let heap_id = heap.allocate(HeapData::LongInt(long_int));
            Value::Ref(heap_id)
        }
    }

    /// Truncates a float into its most compact Python integer representation.
    ///
    /// Finite values outside the immediate range become arbitrary-precision integers;
    /// infinity and NaN raise the exceptions required by Python.
    pub(crate) fn value_from_f64(value: f64, heap: &Heap) -> RunResult<Value> {
        if value.is_infinite() {
            Err(ExcType::overflow_float_infinity_to_integer())
        } else if value.is_nan() {
            Err(ExcType::value_error_float_nan_to_integer())
        } else if value >= i64::MIN as f64 && value < i64::MAX as f64 {
            // `i64::MAX as f64` rounds up to 2**63, so the upper bound is strict.
            #[expect(clippy::cast_possible_truncation, reason = "finite value is within the i64 range")]
            Ok(Value::Int(value as i64))
        } else {
            let value = BigInt::from_f64(value).expect("finite f64 converts to BigInt");
            Ok(Self::new(value).into_value(heap))
        }
    }

    /// Converts to a `Value`, demoting to i64 if it fits.
    ///
    /// For performance, we want to keep values as `Value::Int(i64)` whenever possible.
    /// This method checks if the value fits in an i64 and returns `Value::Int` if so,
    /// otherwise allocates a `HeapData::LongInt` on the heap.
    pub fn into_value(self, heap: &Heap) -> Value {
        // Try to demote back to i64 for performance
        if let Some(i) = self.0.to_i64() {
            Value::Int(i)
        } else {
            let heap_id = heap.allocate(HeapData::LongInt(self));
            Value::Ref(heap_id)
        }
    }

    /// Computes a hash consistent with i64 hashing.
    ///
    /// Critical: For values that fit in i64, this must return the same hash as
    /// hashing the i64 directly. This ensures dict key consistency - e.g.,
    /// `hash(5)` must equal `hash(LongInt(5))`. Delegates to the canonical
    /// helper so that interned and heap `int` values hash identically.
    pub fn hash(&self) -> HashValue {
        hash_python_long_int(&self.0)
    }

    /// Returns a reference to the inner `BigInt`.
    ///
    /// Use this when you need read-only access to the underlying `BigInt`
    /// for operations like formatting or comparison.
    pub fn inner(&self) -> &BigInt {
        &self.0
    }

    /// Checks if the value is zero.
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Checks if the value is negative.
    pub fn is_negative(&self) -> bool {
        self.0.is_negative()
    }

    /// Tries to convert to i64.
    ///
    /// Returns `Some(i64)` if the value fits, `None` otherwise.
    pub fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    /// Tries to convert to f64.
    ///
    /// Returns `Some(f64)` if the conversion is possible, `None` if the value
    /// is too large to represent as f64.
    pub fn to_f64(&self) -> Option<f64> {
        self.0.to_f64()
    }

    /// Compares this integer against an `f64` *exactly* (no precision loss).
    ///
    /// Thin wrapper around [`bigint_cmp_f64`]; see it for the semantics. The
    /// result is the ordering of `self` relative to `f`.
    pub fn partial_cmp_f64(&self, f: f64) -> Option<Ordering> {
        bigint_cmp_f64(&self.0, f)
    }

    /// Tries to convert to usize.
    ///
    /// Returns `Some(usize)` if the value fits, `None` otherwise.
    /// Useful for sequence repetition counts.
    pub fn to_usize(&self) -> Option<usize> {
        self.0.to_usize()
    }

    /// Converts this integer to a Python sequence repetition count.
    pub(crate) fn repeat_count(&self) -> RunResult<usize> {
        if self.is_negative() {
            Ok(0)
        } else {
            self.to_usize()
                .ok_or_else(|| ExcType::overflow_index_sized_int().into())
        }
    }

    /// Returns the absolute value as a new `LongInt`.
    pub fn abs(&self) -> Self {
        Self(self.0.abs())
    }

    /// Returns the number of significant bits in this LongInt.
    ///
    /// Zero returns 0 bits. For non-zero values, this is the position of the
    /// highest set bit plus one.
    pub fn bits(&self) -> u64 {
        self.0.bits()
    }

    /// Checks whether converting this LongInt to a decimal string would exceed
    /// the `INT_MAX_STR_DIGITS` limit.
    ///
    /// This compares the absolute value against the cached `10**4300`
    /// threshold so values with exactly 4300 digits still stringify while
    /// 4301-digit values reliably raise the same error as CPython.
    pub fn check_str_digits_limit(&self) -> RunResult<()> {
        check_bigint_str_digits_limit(&self.0)
    }

    /// Left-shifts an immediate integer, promoting only when the result requires it.
    pub(crate) fn left_shift_i64(value: i64, shift: u64, vm: &mut VM<'_>) -> RunResult<Value> {
        let bits = u64::from(i64::BITS - value.unsigned_abs().leading_zeros());
        check_lshift_size(bits, shift, &vm.heap.tracker)?;
        if value == 0 {
            Ok(Value::Int(0))
        } else if let Ok(shift) = u32::try_from(shift)
            && shift < i64::BITS
            && let Some(result) = value.checked_shl(shift)
            && (result >> shift) == value
        {
            Ok(Value::Int(result))
        } else {
            let input = i128::from(value);
            if let Ok(shift) = u32::try_from(shift)
                && shift < i128::BITS
                && let Some(result) = input.checked_shl(shift)
                && (result >> shift) == input
            {
                Ok(wide_i128_into_value(result, vm.heap))
            } else {
                Ok(Self::new(BigInt::from(value) << shift).into_value(vm.heap))
            }
        }
    }
}

/// Extracts a Python integer as a sequence repetition count.
pub(crate) fn repeat_count(value: &Value, vm: &VM<'_>) -> RunResult<Option<usize>> {
    match value {
        Value::Int(value) => Ok(Some(if *value <= 0 {
            0
        } else {
            usize::try_from(*value).map_err(|_| ExcType::overflow_index_sized_int())?
        })),
        Value::Bool(value) => Ok(Some(usize::from(*value))),
        Value::Ref(id) if let HeapData::LongInt(value) = vm.heap.get(*id) => Ok(Some(value.repeat_count()?)),
        _ => Ok(None),
    }
}

/// Compares a `BigInt` against an `f64` *exactly*, matching CPython's mixed
/// `int`/`float` comparison with no precision loss in either direction
/// (neither operand is rounded to the other's type).
///
/// The result is the ordering of `b` relative to `f` (e.g. `Some(Ordering::Less)`
/// means `b < f`). Returns `None` only when `f` is NaN (unordered); an infinite
/// `f` yields a definite ordering. Equality is `== Some(Ordering::Equal)`.
pub fn bigint_cmp_f64(b: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        None
    } else if f.is_infinite() {
        // +inf is greater than any finite integer, -inf is less.
        Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater })
    } else {
        // `f` is finite. Split it into its integer part `trunc` and the
        // fractional remainder `f - trunc` in (-1, 1). `trunc` is integral and
        // finite, so it converts to `BigInt` without loss.
        let trunc = f.trunc();
        let ord = if let Some(f_int) = trunc.to_i64() {
            bigint_cmp_i64(b, f_int)
        } else {
            let f_int = BigInt::from_f64(trunc).expect("finite f64 converts to BigInt");
            b.cmp(&f_int)
        };
        match ord {
            // Integer parts match: the sign of `f`'s fractional part breaks the
            // tie. A positive fraction makes `f` larger, so `b < f`.
            Ordering::Equal => (f - trunc).partial_cmp(&0.0).map(Ordering::reverse),
            ord => Some(ord),
        }
    }
}

/// Checks bigint/float equality exactly, avoiding allocation for common floats.
pub fn bigint_eq_f64(b: &BigInt, f: f64) -> bool {
    f.is_finite()
        && f.fract() == 0.0
        && if let Some(i) = f.to_i64() {
            bigint_eq_i64(b, i)
        } else {
            bigint_cmp_f64(b, f) == Some(Ordering::Equal)
        }
}

/// Compares a borrowed bigint with an i64 without allocating a temporary bigint.
pub(crate) fn bigint_eq_i64(b: &BigInt, i: i64) -> bool {
    b.to_i64() == Some(i)
}

/// Orders a borrowed bigint against an i64 without allocating a temporary bigint.
pub(crate) fn bigint_cmp_i64(b: &BigInt, i: i64) -> Ordering {
    if let Some(value) = b.to_i64() {
        value.cmp(&i)
    } else if b.sign() == num_bigint::Sign::Minus {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// Compares an `i64` against an `f64` *exactly* (no precision loss), matching
/// CPython's mixed `int`/`float` comparison.
///
/// Equivalent to [`bigint_cmp_f64`] but avoids a `BigInt` allocation for the
/// common machine-integer case. The result is the ordering of `a` relative to
/// `f`; `None` only for NaN.
pub fn i64_cmp_f64(a: i64, f: f64) -> Option<Ordering> {
    // 2^63 as f64 (exactly representable): the first power of two past i64::MAX.
    const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
    if f.is_nan() {
        None
    } else if f >= TWO_POW_63 {
        Some(Ordering::Less) // f (incl. +inf) exceeds i64::MAX ≥ a
    } else if f < -TWO_POW_63 {
        Some(Ordering::Greater) // f (incl. -inf) is below i64::MIN ≤ a
    } else {
        // -2^63 ≤ f < 2^63 and finite, so `trunc` fits in i64 exactly.
        let trunc = f.trunc();
        #[expect(clippy::cast_possible_truncation, reason = "bounds-checked: -2^63 ≤ trunc < 2^63")]
        match a.cmp(&(trunc as i64)) {
            Ordering::Equal => (f - trunc).partial_cmp(&0.0).map(Ordering::reverse),
            ord => Some(ord),
        }
    }
}

/// Checks whether a decimal digit count exceeds `INT_MAX_STR_DIGITS`.
///
/// This is used by parsing code paths that can count decimal digits directly
/// from the original source text before constructing a `BigInt`.
pub fn check_decimal_digit_count(digit_count: usize) -> RunResult<()> {
    if digit_count > INT_MAX_STR_DIGITS {
        return Err(ExcType::value_error_int_str_too_large(digit_count));
    }
    Ok(())
}

/// Counts the decimal digits in an ASCII integer representation.
///
/// Leading `+` or `-` signs are ignored so the return value matches CPython's
/// `value has N digits` wording.
pub fn decimal_digit_count_ascii(value: &[u8]) -> usize {
    value.iter().filter(|byte| byte.is_ascii_digit()).count()
}

/// Checks whether a `BigInt` would exceed the decimal digit limit when
/// converted to a string.
///
/// Values are compared against `10**4300` rather than using an upper-bound bit
/// estimate so boundary values like `10**4300 - 1` remain allowed.
pub fn check_bigint_str_digits_limit(value: &BigInt) -> RunResult<()> {
    let threshold = int_max_str_digits_threshold();
    let abs_value = value.abs();
    if abs_value.bits() > threshold.bits() || (abs_value.bits() == threshold.bits() && abs_value >= *threshold) {
        return Err(ExcType::value_error_int_too_large_for_str());
    }
    Ok(())
}

/// Checks whether an integer with the given bit count might exceed the decimal
/// digit limit when converted to a string.
///
/// This remains as a cheap preflight helper for code that only needs a fast
/// upper-bound check and does not require the exact boundary behavior.
pub fn check_bits_str_digits_limit(bits: u64) -> RunResult<()> {
    // log10(2) ≈ 0.30103 = 30_103/100_000
    // estimated_digits is an upper bound on the actual decimal digit count.
    let estimated_digits = bits.saturating_mul(30_103) / 100_000 + 1;
    if estimated_digits > INT_MAX_STR_DIGITS as u64 {
        return Err(ExcType::value_error_int_too_large_for_str());
    }
    Ok(())
}

/// Returns the cached `10**INT_MAX_STR_DIGITS` threshold used by decimal
/// string-conversion guards.
fn int_max_str_digits_threshold() -> &'static BigInt {
    INT_MAX_STR_DIGITS_THRESHOLD.get_or_init(|| {
        BigInt::from(10u8).pow(u32::try_from(INT_MAX_STR_DIGITS).expect("INT_MAX_STR_DIGITS should fit in u32"))
    })
}

// === Trait Implementations ===

impl<'h> PyTrait<'h> for HeapObjectRead<'h, LongInt> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Int
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).is_zero())
    }

    /// A `LongInt` *is* an int, so it indexes as itself — the caller narrows it
    /// (or reports the overflow its own way).
    fn py_index_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(Some(self.clone_value(vm.heap)))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(eq_bigint(self.get(vm.heap).inner(), other, vm))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(self.get(vm.heap).hash()))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let value = self.get(vm.heap);
        value.check_str_digits_limit()?;
        Ok(write!(f, "{value}")?)
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let value = self.get(vm.heap);
        value.check_str_digits_limit()?;
        Ok(allocate_string(value.to_string(), vm.heap))
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(rhs) => lhs.inner() + rhs,
            Value::Bool(rhs) => lhs.inner() + i64::from(*rhs),
            Value::Float(rhs) => return Ok(Some(Value::Float(long_int_to_f64(lhs) + rhs))),
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => lhs.inner() + rhs.inner(),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        // `+` is commutative here, and the id is unused by the direct form.
        self.py_add_impl(other, vm)
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let negated = -LongInt::new(self.get(vm.heap).inner().clone());
        // A negated LongInt may fit back in an `i64`, which `into_value` demotes.
        Ok(Some(negated.into_value(vm.heap)))
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        // `+x` on an int is the identity, so hand back this same LongInt rather
        // than allocating a copy of its digits. The caller owns the returned
        // value, hence the extra reference.
        Ok(Some(self.clone_value(vm.heap)))
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(rhs) => lhs.inner() - rhs,
            Value::Bool(rhs) => lhs.inner() - i64::from(*rhs),
            Value::Float(rhs) => return Ok(Some(Value::Float(long_int_to_f64(lhs) - rhs))),
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => lhs.inner() - rhs.inner(),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        let result = match other {
            Value::Int(lhs) => BigInt::from(*lhs) - rhs.inner(),
            Value::Bool(lhs) => BigInt::from(*lhs) - rhs.inner(),
            Value::Float(lhs) => return Ok(Some(Value::Float(lhs - long_int_to_f64(rhs)))),
            Value::Ref(id) if let HeapData::LongInt(lhs) = vm.heap.get(*id) => lhs.inner() - rhs.inner(),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap).inner();
        let result = match other {
            Value::Int(0) | Value::Bool(false) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => lhs.mod_floor(&BigInt::from(*rhs)),
            Value::Bool(true) => BigInt::ZERO,
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                lhs.mod_floor(rhs.inner())
            }
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_rmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let result = match other {
            Value::Int(lhs) => BigInt::from(*lhs).mod_floor(rhs.inner()),
            Value::Bool(lhs) => BigInt::from(*lhs).mod_floor(rhs.inner()),
            Value::Ref(id) if let HeapData::LongInt(lhs) = vm.heap.get(*id) => lhs.inner().mod_floor(rhs.inner()),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(rhs) => {
                check_mult_size(lhs.bits(), i64_bits(*rhs), &vm.heap.tracker)?;
                Some(LongInt::new(lhs.inner() * rhs).into_value(vm.heap))
            }
            Value::Bool(rhs) => Some(if *rhs {
                LongInt::new(lhs.inner().clone()).into_value(vm.heap)
            } else {
                Value::Int(0)
            }),
            Value::Float(rhs) => Some(Value::Float(long_int_to_f64(lhs) * rhs)),
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                check_mult_size(lhs.bits(), rhs.bits(), &vm.heap.tracker)?;
                Some(LongInt::new(lhs.inner() * rhs.inner()).into_value(vm.heap))
            }
            _ => None,
        };
        Ok(result)
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_truediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let rhs = match other {
            Value::Int(0) | Value::Bool(false) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => *rhs as f64,
            Value::Bool(true) => 1.0,
            Value::Float(0.0) => return Err(ExcType::zero_division().into()),
            Value::Float(rhs) => *rhs,
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                long_int_to_f64(rhs)
            }
            _ => return Ok(None),
        };
        Ok(Some(Value::Float(long_int_to_f64(lhs) / rhs)))
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let lhs = match other {
            Value::Int(lhs) => *lhs as f64,
            Value::Bool(lhs) => f64::from(*lhs),
            Value::Float(lhs) => *lhs,
            _ => return Ok(None),
        };
        Ok(Some(Value::Float(lhs / long_int_to_f64(rhs))))
    }

    fn py_floordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(0) | Value::Bool(false) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => {
                check_div_size(lhs.bits(), &vm.heap.tracker)?;
                lhs.inner().div_floor(&BigInt::from(*rhs))
            }
            Value::Bool(true) => lhs.inner().clone(),
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                check_div_size(lhs.bits(), &vm.heap.tracker)?;
                lhs.inner().div_floor(rhs.inner())
            }
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_rfloordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let lhs = match other {
            Value::Int(lhs) => *lhs,
            Value::Bool(lhs) => i64::from(*lhs),
            _ => return Ok(None),
        };
        check_div_size(i64_bits(lhs), &vm.heap.tracker)?;
        let result = BigInt::from(lhs).div_floor(rhs.inner());
        Ok(Some(LongInt::new(result).into_value(vm.heap)))
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let base = self.get(vm.heap);
        if let Some(modulus) = modulus {
            modular_pow(base.inner(), other, modulus, vm.heap)
        } else {
            long_int_pow(base, other, vm.heap)
        }
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if modulus.is_some() {
            Ok(None)
        } else {
            let Some(base) = integer_value(other, vm.heap) else {
                return Ok(None);
            };
            long_int_pow_value(base.as_ref(), self.get(vm.heap).inner(), vm.heap)
        }
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| lhs & rhs))
    }

    fn py_rand_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| rhs & lhs))
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| lhs | rhs))
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| rhs | lhs))
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| lhs ^ rhs))
    }

    fn py_rxor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.bitwise_value(other, vm, |lhs, rhs| rhs ^ lhs))
    }

    fn py_lshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(shift) = shift_amount(other, vm)? else {
            return Ok(None);
        };
        let value = self.get(vm.heap);
        check_lshift_size(value.bits(), shift, &vm.heap.tracker)?;
        Ok(Some(LongInt::new(value.inner() << shift).into_value(vm.heap)))
    }

    fn py_rlshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = match other {
            Value::Int(lhs) => *lhs,
            Value::Bool(lhs) => i64::from(*lhs),
            _ => return Ok(None),
        };
        let shift = self.get(vm.heap);
        if shift.is_negative() {
            Err(ExcType::value_error_negative_shift_count())
        } else if let Some(shift) = shift.inner().to_u64() {
            Ok(Some(LongInt::left_shift_i64(lhs, shift, vm)?))
        } else {
            Err(ExcType::overflow_c_ssize_t())
        }
    }

    fn py_rshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(shift) = shift_amount(other, vm)? else {
            return Ok(None);
        };
        Ok(Some(
            LongInt::new(self.get(vm.heap).inner() >> shift).into_value(vm.heap),
        ))
    }

    fn py_rrshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = match other {
            Value::Int(lhs) => *lhs,
            Value::Bool(lhs) => i64::from(*lhs),
            _ => return Ok(None),
        };
        let shift = self.get(vm.heap);
        if shift.is_negative() {
            Err(ExcType::value_error_negative_shift_count())
        } else {
            let result = shift
                .inner()
                .to_u32()
                .filter(|shift| *shift < 64)
                .map_or_else(|| if lhs < 0 { -1 } else { 0 }, |shift| lhs >> shift);
            Ok(Some(Value::Int(result)))
        }
    }
}

impl<'h> HeapRead<'h, LongInt> {
    /// Applies a two-operand bitwise operation using this long integer as the left operand.
    fn bitwise_value(
        &self,
        other: &Value,
        vm: &mut VM<'h>,
        operation: impl FnOnce(BigInt, BigInt) -> BigInt,
    ) -> Option<Value> {
        let rhs = match other {
            Value::Int(value) => BigInt::from(*value),
            Value::Bool(value) => BigInt::from(*value),
            Value::Ref(id) if let HeapData::LongInt(value) = vm.heap.get(*id) => value.inner().clone(),
            _ => return None,
        };
        let result = operation(self.get(vm.heap).inner().clone(), rhs);
        Some(LongInt::new(result).into_value(vm.heap))
    }
}

/// Performs modular exponentiation for integer values of any storage representation.
pub(crate) fn modular_pow(base: &BigInt, exponent: &Value, modulus: &Value, heap: &Heap) -> RunResult<Option<Value>> {
    let Some(exponent) = integer_value(exponent, heap) else {
        return Ok(None);
    };
    let Some(modulus) = integer_value(modulus, heap) else {
        return Ok(None);
    };
    if modulus.is_zero() {
        return Err(ExcType::value_error_pow_modulus_zero());
    }
    if exponent.is_negative() {
        return Err(ExcType::value_error_pow_negative_exponent());
    }

    let modulus_abs = modulus.abs();
    let mut result = base.modpow(exponent.as_ref(), &modulus_abs);
    if modulus.is_negative() && !result.is_zero() {
        result -= modulus_abs;
    }
    Ok(Some(LongInt::new(result).into_value(heap)))
}

/// Raises a long integer to another integer value.
fn long_int_pow(base: &LongInt, exponent: &Value, heap: &Heap) -> RunResult<Option<Value>> {
    if let Value::Float(exponent) = exponent {
        if base.is_zero() && *exponent < 0.0 {
            return Err(ExcType::zero_negative_power());
        }
        return Ok(Some(Value::Float(long_int_to_f64(base).powf(*exponent))));
    }
    let Some(exponent) = integer_value(exponent, heap) else {
        return Ok(None);
    };
    long_int_pow_value(base.inner(), exponent.as_ref(), heap)
}

/// Raises one arbitrary-precision integer to another.
fn long_int_pow_value(base: &BigInt, exponent: &BigInt, heap: &Heap) -> RunResult<Option<Value>> {
    if base.is_zero() && exponent.is_negative() {
        Err(ExcType::zero_negative_power())
    } else if exponent.is_negative() {
        let exponent = exponent
            .to_f64()
            .filter(|exponent| exponent.is_finite())
            .ok_or_else(ExcType::overflow_int_to_float)?;
        Ok(Some(Value::Float(bigint_to_f64(base).powf(exponent))))
    } else if exponent.is_zero() || base.is_one() {
        Ok(Some(Value::Int(1)))
    } else if base.is_zero() {
        Ok(Some(Value::Int(0)))
    } else if *base == BigInt::from(-1) {
        Ok(Some(Value::Int(if (exponent % 2i32).is_zero() { 1 } else { -1 })))
    } else if let Some(exponent) = exponent.to_u64() {
        check_pow_size(base.bits(), exponent, &heap.tracker)?;
        Ok(Some(LongInt::new(bigint_pow(base.clone(), exponent)).into_value(heap)))
    } else {
        Err(ExcType::overflow_exponent_too_large())
    }
}

/// Borrows or promotes an integer value into its arbitrary-precision representation.
fn integer_value<'a>(value: &'a Value, heap: &'a Heap) -> Option<Cow<'a, BigInt>> {
    match value {
        Value::Int(value) => Some(Cow::Owned(BigInt::from(*value))),
        Value::Bool(value) => Some(Cow::Owned(BigInt::from(*value))),
        Value::Ref(id) if let HeapData::LongInt(value) = heap.get(*id) => Some(Cow::Borrowed(value.inner())),
        _ => None,
    }
}

/// Converts a long integer to float, preserving its sign on overflow.
fn long_int_to_f64(value: &LongInt) -> f64 {
    bigint_to_f64(value.inner())
}

/// Converts an arbitrary-precision integer to float, preserving its sign on overflow.
fn bigint_to_f64(value: &BigInt) -> f64 {
    value.to_f64().unwrap_or_else(|| {
        if value.is_negative() {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }
    })
}

/// Raises a `BigInt` to a `u64` exponent without truncating the exponent.
fn bigint_pow(mut base: BigInt, mut exp: u64) -> BigInt {
    let mut result = BigInt::from(1);
    while exp > 0 {
        if exp & 1 == 1 {
            result *= &base;
        }
        exp >>= 1;
        if exp > 0 {
            base = &base * &base;
        }
    }
    result
}

/// Returns the significant bit count of an immediate integer.
fn i64_bits(value: i64) -> u64 {
    u64::from(i64::BITS - value.unsigned_abs().leading_zeros())
}

/// Extracts a validated non-negative shift amount from an integer value.
fn shift_amount(value: &Value, vm: &VM<'_>) -> RunResult<Option<u64>> {
    let value = match value {
        Value::Int(value) => return shift_i64(*value).map(Some),
        Value::Bool(value) => return Ok(Some(u64::from(*value))),
        Value::Ref(id) if let HeapData::LongInt(value) = vm.heap.get(*id) => value,
        _ => return Ok(None),
    };
    if value.is_negative() {
        Err(ExcType::value_error_negative_shift_count())
    } else {
        value.inner().to_u64().map(Some).ok_or_else(ExcType::overflow_c_ssize_t)
    }
}

/// Validates an immediate integer shift amount.
fn shift_i64(value: i64) -> RunResult<u64> {
    if value < 0 {
        Err(ExcType::value_error_negative_shift_count())
    } else {
        #[expect(clippy::cast_sign_loss)]
        Ok(value as u64)
    }
}

impl From<BigInt> for LongInt {
    fn from(bi: BigInt) -> Self {
        Self(bi)
    }
}

impl From<i64> for LongInt {
    fn from(i: i64) -> Self {
        Self(BigInt::from(i))
    }
}

impl Add for LongInt {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for LongInt {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl Mul for LongInt {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self(self.0 * rhs.0)
    }
}

impl Neg for LongInt {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Display for LongInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
