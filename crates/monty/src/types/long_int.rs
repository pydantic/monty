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
    fmt::{self, Display},
    mem,
    ops::{Add, Mul, Neg, Sub},
    sync::OnceLock,
};

use num_bigint::BigInt;
use num_traits::{FromPrimitive, Signed, ToPrimitive, Zero};

use crate::{
    exception_private::{ExcType, RunResult},
    hash::{HashValue, hash_python_long_int},
    heap::{Heap, HeapData},
    resource::{ResourceError, ResourceTracker},
    value::Value,
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
    pub fn into_value(self, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
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
