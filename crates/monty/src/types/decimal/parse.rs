//! Exact conversions into `Decimal`: numeric strings, integers and floats.
//!
//! Construction never rounds. Malformed strings report `ConversionSyntax` and
//! exponents outside C `_decimal`'s literal range report `InvalidOperation`; the
//! caller decides from the context's traps whether that raises or yields `NaN`.

use num_bigint::BigInt;

use super::{
    Coeff, Ctx, Decimal, MAX_DIGITS, Special,
    context::{Condition, MAX_EMAX, MIN_ETINY},
};
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::unicode_type::{decimal_value, is_space},
};

/// Parses a numeric string as `Decimal(str)` does.
///
/// Surrounding whitespace and every underscore are ignored, letters are matched
/// case-insensitively and any Unicode decimal digit counts as a digit.
/// Returns the condition to raise for a malformed literal or an exponent out of
/// range; fails outright only for a coefficient beyond [`MAX_DIGITS`].
pub(crate) fn parse_str(text: &str) -> RunResult<Result<Decimal, Condition>> {
    let trimmed = text.trim_matches(is_space);
    let mut chars: Vec<char> = Vec::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c != '_' {
            chars.push(decimal_value(c).map_or(c, |d| char::from_digit(d, 10).expect("digit below ten")));
        }
    }
    let mut pos = 0;
    let sign = match chars.first() {
        Some('-') => {
            pos = 1;
            true
        }
        Some('+') => {
            pos = 1;
            false
        }
        _ => false,
    };
    let rest: String = chars[pos..].iter().collect();
    let lower = rest.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Ok(Ok(Decimal::infinity(sign)));
    }
    if let Some(nan) = lower
        .strip_prefix("snan")
        .map(|d| (d, Special::SignalingNan))
        .or_else(|| lower.strip_prefix("nan").map(|d| (d, Special::QuietNan)))
    {
        let (diag, kind) = nan;
        return Ok(if diag.bytes().all(|b| b.is_ascii_digit()) {
            let payload = diag.trim_start_matches('0');
            check_digit_count(payload.len())?;
            Ok(Decimal::special_nan(sign, Coeff::from_digits(payload), kind))
        } else {
            Err(Condition::ConversionSyntax)
        });
    }
    parse_finite(sign, &lower)
}

/// Parses the finite form `digits[.digits][e[sign]digits]`, already lowercased.
fn parse_finite(sign: bool, text: &str) -> RunResult<Result<Decimal, Condition>> {
    let (mantissa, exponent) = match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text, None),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let is_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if int_part.len() + frac_part.len() == 0 || !is_digits(int_part) || !is_digits(frac_part) {
        return Ok(Err(Condition::ConversionSyntax));
    }
    let exp = match exponent {
        None => 0,
        Some(exponent) => {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if digits.is_empty() || !is_digits(digits) {
                return Ok(Err(Condition::ConversionSyntax));
            }
            // Saturate: anything this large is out of range either way.
            let magnitude = digits.trim_start_matches('0');
            let value = if magnitude.len() > 30 {
                i128::MAX / 4
            } else {
                magnitude.parse::<i128>().unwrap_or(0)
            };
            if exponent.starts_with('-') { -value } else { value }
        }
    };
    let digits = format!("{int_part}{frac_part}");
    let coeff = match digits.trim_start_matches('0') {
        "" => "0",
        trimmed => trimmed,
    };
    check_digit_count(coeff.len())?;
    let exp = exp - i128::try_from(frac_part.len()).expect("length fits in i128");
    Ok(checked_exponent(exp, coeff.len()).map(|exp| Decimal::from_triple(sign, Coeff::from_digits(coeff), exp)))
}

/// Validates a literal exponent against C `_decimal`'s range: the exponent must be at
/// least `MIN_ETINY` and the adjusted exponent at most `MAX_EMAX`.
pub(crate) fn checked_exponent(exp: i128, digits: usize) -> Result<i64, Condition> {
    let adjusted = exp + i128::try_from(digits).expect("length fits in i128") - 1;
    if exp < i128::from(MIN_ETINY) || adjusted > i128::from(MAX_EMAX) {
        Err(Condition::InvalidOperation)
    } else {
        Ok(i64::try_from(exp).expect("exponent is in range"))
    }
}

/// Rejects coefficients longer than [`MAX_DIGITS`], Monty's sandbox cap.
pub(crate) fn check_digit_count(digits: usize) -> RunResult<()> {
    if digits > MAX_DIGITS {
        Err(ExcType::value_error(format!(
            "Exceeds the limit ({MAX_DIGITS} digits) for a Decimal coefficient: value has {digits} digits"
        )))
    } else {
        Ok(())
    }
}

/// `Decimal(int)`: exact.
pub(crate) fn from_int(value: &BigInt) -> RunResult<Decimal> {
    // `bits * log10(2)` slightly overestimates the digit count; check before converting.
    let approx_digits = value.bits().saturating_mul(30_103) / 100_000;
    check_digit_count(usize::try_from(approx_digits).unwrap_or(usize::MAX).saturating_sub(1))?;
    let decimal = Decimal::from_bigint(value);
    check_digit_count(usize::try_from(decimal.coeff().digits()).unwrap_or(usize::MAX))?;
    Ok(decimal)
}

/// `Decimal.from_float(f)`: the exact value of the double, `n * 5**k / 10**k`.
pub(crate) fn from_float(value: f64) -> Decimal {
    if value.is_nan() {
        Decimal::special_nan(value.is_sign_negative(), Coeff::zero(), Special::QuietNan)
    } else if value.is_infinite() {
        Decimal::infinity(value < 0.0)
    } else {
        let sign = value.is_sign_negative();
        let (numerator, k) = float_as_ratio(value.abs());
        let coeff = numerator * num_traits::Pow::pow(BigInt::from(5u8), k);
        Decimal::from_bigint_triple(sign, &coeff, -i64::from(k))
    }
}

/// `abs(f).as_integer_ratio()` as `(n, k)` with denominator `2**k`, lowest terms.
fn float_as_ratio(value: f64) -> (BigInt, u32) {
    if value == 0.0 {
        return (BigInt::from(0u8), 0);
    }
    let bits = value.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).expect("11-bit field");
    let fraction = bits & ((1 << 52) - 1);
    let (mut mantissa, mut exp2) = if exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), exponent - 1075)
    };
    // Reduce to lowest terms: strip the factors of two the denominator shares.
    while exp2 < 0 && mantissa & 1 == 0 {
        mantissa >>= 1;
        exp2 += 1;
    }
    if exp2 >= 0 {
        (BigInt::from(mantissa) << exp2.unsigned_abs(), 0)
    } else {
        (BigInt::from(mantissa), exp2.unsigned_abs())
    }
}

/// Converts a finite `Decimal` to an exact integer, truncating any fraction.
///
/// The caller handles NaNs and infinities.
pub(crate) fn to_integer(value: &Decimal, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    let int = value.int();
    let magnitude = if value.exponent() >= 0 {
        // The result has `digits + exp` digits.
        ctx.check_digits(u64::try_from(value.digits() + value.exponent()).unwrap_or(u64::MAX))?;
        int * super::pow10(value.exponent().unsigned_abs())
    } else if value.exponent() <= -value.digits() {
        BigInt::from(0u8)
    } else {
        int / super::pow10(value.exponent().unsigned_abs())
    };
    Ok(if value.sign() { -magnitude } else { magnitude })
}
