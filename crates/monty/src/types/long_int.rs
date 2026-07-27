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
    cmp::Ordering,
    fmt::{self, Display, Write},
    mem,
    ops::{Add, Mul, Neg, Sub},
    sync::OnceLock,
};

use monty_types::ResourceError;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{FromPrimitive, Signed, ToPrimitive, Zero};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_python_long_int},
    heap::{Heap, HeapData, HeapId, HeapRead},
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

/// Converts an `i128` into the compact integer representation.
pub(crate) fn i128_into_value(value: i128, heap: &Heap) -> Result<Value, ResourceError> {
    if let Ok(value) = i64::try_from(value) {
        Ok(Value::Int(value))
    } else {
        LongInt::new(BigInt::from(value)).into_value(heap)
    }
}

/// Converts a `u128` into the compact integer representation.
pub(crate) fn u128_into_value(value: u128, heap: &Heap) -> Result<Value, ResourceError> {
    if let Ok(value) = i64::try_from(value) {
        Ok(Value::Int(value))
    } else {
        LongInt::new(BigInt::from(value)).into_value(heap)
    }
}

impl LongInt {
    /// Creates a new `LongInt` from a `BigInt`.
    pub fn new(bi: BigInt) -> Self {
        Self(bi)
    }

    /// Converts to a `Value`, demoting to i64 if it fits.
    ///
    /// For performance, we want to keep values as `Value::Int(i64)` whenever possible.
    /// This method checks if the value fits in an i64 and returns `Value::Int` if so,
    /// otherwise allocates a `HeapData::LongInt` on the heap.
    pub fn into_value(self, heap: &Heap) -> Result<Value, ResourceError> {
        // Try to demote back to i64 for performance
        if let Some(i) = self.0.to_i64() {
            Ok(Value::Int(i))
        } else {
            let heap_id = heap.allocate(HeapData::LongInt(self))?;
            Ok(Value::Ref(heap_id))
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

    /// Estimates memory size in bytes.
    ///
    /// Used for resource tracking. The actual size includes the Vec overhead
    /// plus the digit storage. Rounds up bits to bytes to avoid underestimating
    /// (e.g., 1 bit = 1 byte, not 0 bytes).
    pub fn estimate_size(&self) -> usize {
        // Each BigInt digit is typically a u32 or u64
        // We estimate based on the number of significant bits
        let bits = self.0.bits();
        // Convert bits to bytes (round up), add overhead for Vec and sign
        // On 32-bit platforms, truncate to usize::MAX if bits is too large
        let bit_bytes = usize::try_from(bits).unwrap_or(usize::MAX).saturating_add(7) / 8;
        bit_bytes + mem::size_of::<BigInt>()
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

    /// Tries to convert to u32.
    ///
    /// Returns `Some(u32)` if the value fits, `None` otherwise.
    pub fn to_u32(&self) -> Option<u32> {
        self.0.to_u32()
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
            self.to_usize().ok_or_else(|| ExcType::overflow_repeat_count().into())
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
        check_lshift_size(bits, shift, vm.heap.tracker())?;
        let input = i128::from(value);
        if let Ok(shift) = u32::try_from(shift)
            && shift < i128::BITS
            && let Some(value) = input.checked_shl(shift)
            && (value >> shift) == input
        {
            Ok(i128_into_value(value, vm.heap)?)
        } else {
            Ok(Self::new(BigInt::from(value) << shift).into_value(vm.heap)?)
        }
    }
}

/// Extracts a Python integer as a sequence repetition count.
pub(crate) fn repeat_count(value: &Value, vm: &VM<'_>) -> RunResult<Option<usize>> {
    match value {
        Value::Int(value) => Ok(Some(if *value <= 0 {
            0
        } else {
            usize::try_from(*value).map_err(|_| ExcType::overflow_repeat_count())?
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

impl<'h> PyTrait<'h> for HeapRead<'h, LongInt> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Int
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> bool {
        !self.get(vm.heap).is_zero()
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(eq_bigint(self.get(vm.heap).inner(), other, vm))
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
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
        Ok(allocate_string(value.to_string(), vm.heap)?)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap).inner();
        let result = match other {
            Value::Int(rhs) => lhs + rhs,
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => lhs + rhs.inner(),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_add_impl(other, vm)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap).inner();
        let result = match other {
            Value::Int(rhs) => lhs - rhs,
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => lhs - rhs.inner(),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap).inner();
        let result = match other {
            Value::Int(lhs) => BigInt::from(*lhs) - rhs,
            Value::Ref(id) if let HeapData::LongInt(lhs) = vm.heap.get(*id) => lhs.inner() - rhs,
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap).inner();
        let result = match other {
            Value::Int(0) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => lhs.mod_floor(&BigInt::from(*rhs)),
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                lhs.mod_floor(rhs.inner())
            }
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_rmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let result = match other {
            Value::Int(lhs) => BigInt::from(*lhs).mod_floor(rhs.inner()),
            Value::Ref(id) if let HeapData::LongInt(lhs) = vm.heap.get(*id) => lhs.inner().mod_floor(rhs.inner()),
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(rhs) => {
                check_mult_size(lhs.bits(), i64_bits(*rhs), vm.heap.tracker())?;
                Some(LongInt::new(lhs.inner() * rhs).into_value(vm.heap)?)
            }
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                check_mult_size(lhs.bits(), rhs.bits(), vm.heap.tracker())?;
                Some(LongInt::new(lhs.inner() * rhs.inner()).into_value(vm.heap)?)
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
            Value::Int(0) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => *rhs as f64,
            Value::Float(0.0) => return Err(ExcType::zero_division().into()),
            Value::Float(rhs) => *rhs,
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                rhs.to_f64().unwrap_or(f64::INFINITY)
            }
            _ => return Ok(None),
        };
        Ok(Some(Value::Float(lhs.to_f64().unwrap_or(f64::INFINITY) / rhs)))
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let lhs = match other {
            Value::Int(lhs) => *lhs as f64,
            Value::Float(lhs) => *lhs,
            _ => return Ok(None),
        };
        Ok(Some(Value::Float(lhs / rhs.to_f64().unwrap_or(f64::INFINITY))))
    }

    fn py_floordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let lhs = self.get(vm.heap);
        let result = match other {
            Value::Int(0) => return Err(ExcType::zero_division().into()),
            Value::Int(rhs) => {
                check_div_size(lhs.bits(), vm.heap.tracker())?;
                lhs.inner().div_floor(&BigInt::from(*rhs))
            }
            Value::Ref(id) if let HeapData::LongInt(rhs) = vm.heap.get(*id) => {
                if rhs.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                check_div_size(lhs.bits(), vm.heap.tracker())?;
                lhs.inner().div_floor(rhs.inner())
            }
            _ => return Ok(None),
        };
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_rfloordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let rhs = self.get(vm.heap);
        if rhs.is_zero() {
            return Err(ExcType::zero_division().into());
        }
        let Value::Int(lhs) = other else {
            return Ok(None);
        };
        check_div_size(i64_bits(*lhs), vm.heap.tracker())?;
        let result = BigInt::from(*lhs).div_floor(rhs.inner());
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if modulus.is_some() {
            return Ok(None);
        }
        let Value::Int(exp) = other else {
            return Ok(None);
        };
        let base = self.get(vm.heap);
        if base.is_zero() && *exp < 0 {
            Err(ExcType::zero_negative_power())
        } else if *exp >= 0 {
            #[expect(clippy::cast_sign_loss)]
            let exp = *exp as u64;
            check_pow_size(base.bits(), exp, vm.heap.tracker())?;
            let result = bigint_pow(base.inner().clone(), exp);
            Ok(Some(LongInt::new(result).into_value(vm.heap)?))
        } else {
            Ok(Some(Value::Float(
                base.to_f64().map_or(0.0, |base| base.powf(*exp as f64)),
            )))
        }
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if modulus.is_some() {
            return Ok(None);
        }
        let Value::Int(base) = other else {
            return Ok(None);
        };
        let exp = self.get(vm.heap);
        if *base == 0 && exp.is_negative() {
            Err(ExcType::zero_negative_power())
        } else if exp.is_negative() {
            Ok(Some(Value::Float(
                exp.to_f64().map_or(0.0, |exp| (*base as f64).powf(exp)),
            )))
        } else if exp.is_zero() || *base == 1 {
            Ok(Some(Value::Int(1)))
        } else if *base == 0 {
            Ok(Some(Value::Int(0)))
        } else if *base == -1 {
            Ok(Some(Value::Int(if (exp.inner() % 2i32).is_zero() { 1 } else { -1 })))
        } else if let Some(exp) = exp.to_u32() {
            check_pow_size(i64_bits(*base), u64::from(exp), vm.heap.tracker())?;
            Ok(Some(LongInt::new(BigInt::from(*base).pow(exp)).into_value(vm.heap)?))
        } else {
            Err(ExcType::overflow_exponent_too_large())
        }
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| lhs & rhs)
    }

    fn py_rand_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| rhs & lhs)
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| lhs | rhs)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| rhs | lhs)
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| lhs ^ rhs)
    }

    fn py_rxor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.bitwise_value(other, vm, |lhs, rhs| rhs ^ lhs)
    }

    fn py_lshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(shift) = shift_amount(other, vm)? else {
            return Ok(None);
        };
        let value = self.get(vm.heap);
        check_lshift_size(value.bits(), shift, vm.heap.tracker())?;
        Ok(Some(LongInt::new(value.inner() << shift).into_value(vm.heap)?))
    }

    fn py_rlshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Value::Int(lhs) = other else {
            return Ok(None);
        };
        let shift = self.get(vm.heap);
        if shift.is_negative() {
            Err(ExcType::value_error_negative_shift_count())
        } else if let Some(shift) = shift.inner().to_u64() {
            Ok(Some(LongInt::left_shift_i64(*lhs, shift, vm)?))
        } else {
            Err(ExcType::overflow_c_ssize_t())
        }
    }

    fn py_rshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(shift) = shift_amount(other, vm)? else {
            return Ok(None);
        };
        Ok(Some(
            LongInt::new(self.get(vm.heap).inner() >> shift).into_value(vm.heap)?,
        ))
    }

    fn py_rrshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Value::Int(lhs) = other else {
            return Ok(None);
        };
        let shift = self.get(vm.heap);
        if shift.is_negative() {
            Err(ExcType::value_error_negative_shift_count())
        } else {
            let result = shift
                .inner()
                .to_u32()
                .filter(|shift| *shift < 64)
                .map_or_else(|| if *lhs < 0 { -1 } else { 0 }, |shift| lhs >> shift);
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
    ) -> RunResult<Option<Value>> {
        let rhs = match other {
            Value::Int(value) => BigInt::from(*value),
            Value::Bool(value) => BigInt::from(*value),
            Value::Ref(id) if let HeapData::LongInt(value) = vm.heap.get(*id) => value.inner().clone(),
            _ => return Ok(None),
        };
        let result = operation(self.get(vm.heap).inner().clone(), rhs);
        Ok(Some(LongInt::new(result).into_value(vm.heap)?))
    }
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
