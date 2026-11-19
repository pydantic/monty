//! Rounding helpers shared by immediate and heap-allocated numbers.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Pow, ToPrimitive, float::FloatCore};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::Heap,
    types::{LongInt, long_int::bigint_true_divide},
    value::Value,
};

/// Resolves numeric rounding precision through `__index__`, clamping huge integers.
/// User `__round__` methods receive the original argument instead of using this helper.
pub(crate) fn round_digits(ndigits: Option<&Value>, vm: &mut VM<'_>) -> RunResult<Option<i64>> {
    match ndigits {
        None | Some(Value::None) => Ok(None),
        Some(value) => value.saturating_index(vm).map(Some),
    }
}

/// Rounds a small integer without losing precision, promoting an overflowing result.
pub(crate) fn round_int(n: i64, digits: Option<i64>, heap: &Heap) -> Value {
    if let Some(d) = digits.filter(|d| *d < 0) {
        // Any divisor >= 10**20 rounds an i64 to zero; smaller intermediates fit i128.
        let result = match u32::try_from(d.unsigned_abs()) {
            Ok(exp @ ..=19) => {
                let factor = 10_i128.pow(exp);
                let n = i128::from(n);
                let mut q = n / factor;
                let r2 = (n % factor).abs() * 2;
                if r2 > factor || (r2 == factor && q % 2 != 0) {
                    q += if n < 0 { -1 } else { 1 };
                }
                q * factor
            }
            _ => 0,
        };
        match i64::try_from(result) {
            Ok(i) => Value::Int(i),
            Err(_) => LongInt::new(BigInt::from(result)).into_value(heap),
        }
    } else {
        Value::Int(n)
    }
}

/// Rounds a wide integer to a multiple of `10**k`, with ties to even.
pub(crate) fn round_bigint(n: &BigInt, k: u64, heap: &Heap) -> Value {
    let digit_bound = n.bits().saturating_mul(30_103) / 100_000 + 1;
    if k > digit_bound {
        return Value::Int(0);
    }
    let factor = Pow::pow(BigInt::from(10u8), k);
    let (quotient, remainder) = n.div_mod_floor(&factor);
    let twice_remainder = remainder << 1u32;
    let quotient = if twice_remainder > factor || (twice_remainder == factor && quotient.is_odd()) {
        quotient + 1
    } else {
        quotient
    };
    LongInt::new(quotient * factor).into_value(heap)
}

/// Rounds a float, returning an integer only when precision is omitted or `None`.
pub(crate) fn round_float(value: f64, digits: Option<i64>, heap: &Heap) -> RunResult<Value> {
    if let Some(digits) = digits {
        round_float_to_digits(value, digits, heap).map(Value::Float)
    } else {
        LongInt::value_from_f64(bankers_round(value), heap)
    }
}

/// Rounds to the nearest integer, breaking ties toward an even integer.
fn bankers_round(value: f64) -> f64 {
    let floor = value.floor();
    let frac = value - floor;
    if frac < 0.5 {
        floor
    } else if frac > 0.5 {
        floor + 1.0
    } else if floor % 2.0 == 0.0 {
        floor
    } else {
        floor + 1.0
    }
}

/// Rounds the exact binary value on a decimal grid before converting back to `f64`.
fn round_float_to_digits(value: f64, digits: i64, heap: &Heap) -> RunResult<f64> {
    // Beyond binary64's decimal precision bounds, rounding cannot change a finite value
    // or must produce zero. Bound the powers before handling arbitrary-sized precision.
    if !value.is_finite() || value == 0.0 || digits > 323 {
        return Ok(value);
    }
    if digits < -308 {
        return Ok(0.0_f64.copysign(value));
    }

    let (mantissa, exponent, _) = value.integer_decode();
    if let Some(rounded) = round_float_to_digits_u128(mantissa, exponent, digits) {
        return Ok(rounded.copysign(value));
    }

    let mut numerator = BigInt::from(mantissa);
    let mut denominator = BigInt::from(1u8);
    if exponent >= 0 {
        numerator <<= u32::from(exponent.unsigned_abs());
    } else {
        denominator <<= u32::from(exponent.unsigned_abs());
    }
    let precision = u32::try_from(digits.unsigned_abs()).expect("rounding precision is bounded");
    let factor = BigInt::from(10u8).pow(precision);
    if digits >= 0 {
        numerator *= &factor;
    } else {
        denominator *= &factor;
    }

    // Scaling binary floats first can turn a below-half value into an exact tie.
    // These exact operands stay below 2100 bits for every finite binary64 input.
    let (mut quotient, remainder) = numerator.div_rem(&denominator);
    let twice_remainder = remainder << 1u32;
    if twice_remainder > denominator || (twice_remainder == denominator && quotient.is_odd()) {
        quotient += 1u8;
    }
    let rounded = if digits >= 0 {
        bigint_true_divide(&quotient, &factor, &heap.tracker)?
    } else {
        (quotient * factor)
            .to_f64()
            .filter(|result| result.is_finite())
            .ok_or_else(|| ExcType::overflow_error("rounded value too large to represent"))?
    };
    Ok(rounded.copysign(value))
}

/// Uses fixed-width integers when the result needs at most one floating-point rounding.
fn round_float_to_digits_u128(mantissa: u64, exponent: i16, digits: i64) -> Option<f64> {
    let factor = 10_u128.checked_pow(u32::try_from(digits.unsigned_abs()).ok()?)?;
    let binary_scale = 1_u128.checked_shl(u32::from(exponent.unsigned_abs()))?;
    let mut numerator = u128::from(mantissa);
    let mut denominator = 1_u128;
    if exponent >= 0 {
        numerator = numerator.checked_mul(binary_scale)?;
    } else {
        denominator = binary_scale;
    }
    if digits >= 0 {
        numerator = numerator.checked_mul(factor)?;
    } else {
        denominator = denominator.checked_mul(factor)?;
    }

    let mut quotient = numerator / denominator;
    let remainder = numerator % denominator;
    // Compare with one half without doubling the remainder.
    let complement = denominator - remainder;
    if remainder > complement || (remainder == complement && quotient.is_odd()) {
        quotient = quotient.checked_add(1)?;
    }
    if digits < 0 {
        return Some(quotient.checked_mul(factor)? as f64);
    }

    let common = quotient.gcd(&factor);
    let numerator = quotient / common;
    let denominator = factor / common;
    // Both operands must be exact floats so only division rounds the result.
    let exact_float = |integer: u128| {
        (integer.leading_zeros() + integer.trailing_zeros() >= u128::BITS - f64::MANTISSA_DIGITS)
            .then_some(integer as f64)
    };
    Some(exact_float(numerator)? / exact_float(denominator)?)
}
