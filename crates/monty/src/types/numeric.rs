//! Numeric helpers shared by immediate and heap-allocated numbers.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Pow, ToPrimitive, float::FloatCore};
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::Heap,
    intern::StaticStrings,
    types::{LongInt, allocate_tuple, long_int::bigint_true_divide},
    value::{EitherStr, Value},
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

/// Returns `true` if `value` is finite and has an integral value (`float.is_integer()`).
#[inline]
#[expect(
    clippy::float_cmp,
    reason = "integral float definition requires exact equality with truncation"
)]
pub(crate) fn float_is_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value
}

/// `abs(f).as_integer_ratio()` as `(n, k)` with denominator `2**k`, in lowest terms.
///
/// Caller must ensure `value` is finite and non-negative (`value.abs()`).
pub(crate) fn float_as_ratio(value: f64) -> (BigInt, u32) {
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

/// Computes `float.as_integer_ratio()`, returning `(numerator, denominator)` in lowest terms.
pub(crate) fn float_as_integer_ratio(value: f64, heap: &Heap) -> RunResult<Value> {
    if value.is_infinite() {
        return Err(ExcType::overflow_error("cannot convert Infinity to integer ratio"));
    }
    if value.is_nan() {
        return Err(ExcType::value_error("cannot convert NaN to integer ratio"));
    }
    if value == 0.0 {
        return Ok(allocate_tuple(smallvec![Value::Int(0), Value::Int(1)], heap));
    }
    let (mut numerator, k) = float_as_ratio(value.abs());
    if value.is_sign_negative() {
        numerator = -numerator;
    }
    let num_val = LongInt::new(numerator).into_value(heap);
    let den_val = if k == 0 {
        Value::Int(1)
    } else if k < 63 {
        Value::Int(1_i64 << k)
    } else {
        LongInt::new(BigInt::from(1u8) << k).into_value(heap)
    };
    Ok(allocate_tuple(smallvec![num_val, den_val], heap))
}

/// Class methods on `float` (`fromhex`).
pub(crate) fn float_class_method(
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Result<RunResult<CallResult>, ArgValues> {
    match attr.static_string(vm.interns) {
        Some(StaticStrings::Fromhex) => Ok(float_fromhex(args, vm).map(CallResult::Value)),
        _ => Err(args),
    }
}

/// Formats an `f64` as CPython's `float.hex()` representation.
pub(crate) fn float_hex(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }
    let bits = value.to_bits();
    let sign = if (bits >> 63) != 0 { "-" } else { "" };
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);

    if exponent == 0 && fraction == 0 {
        format!("{sign}0x0.0p+0")
    } else if exponent == 0 {
        // Subnormal: biased exponent is 0, unbiased exponent is -1022, leading digit is 0.
        format!("{sign}0x0.{fraction:013x}p-1022")
    } else {
        let unbiased = exponent - 1023;
        format!("{sign}0x1.{fraction:013x}p{unbiased:+}")
    }
}

/// Implements `float.fromhex(string)` (both classmethod and instance method).
pub(crate) fn float_fromhex(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let args = args.reject_kwargs("float.fromhex", vm.heap)?;
    let arg = args.get_one_arg("float.fromhex", vm.heap)?;
    defer_drop!(arg, vm);
    let Some(s) = arg.as_str(vm) else {
        return Err(ExcType::type_error("bad argument type for built-in operation"));
    };
    parse_float_fromhex(s).map(Value::Float)
}

/// Returns `true` if `b` is an ASCII whitespace character recognized by CPython's `Py_ISSPACE`.
#[inline]
fn is_py_ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parses a hexadecimal floating-point string matching CPython's `float_fromhex_impl`.
fn parse_float_fromhex(input: &str) -> RunResult<f64> {
    let invalid_err = || ExcType::value_error("invalid hexadecimal floating-point string");
    let overflow_err = || {
        SimpleException::new_msg(
            ExcType::OverflowError,
            "hexadecimal value too large to represent as a float",
        )
        .into()
    };

    let bytes = input.as_bytes();
    let start = bytes
        .iter()
        .position(|&b| !is_py_ascii_space(b))
        .ok_or_else(invalid_err)?;
    let end = bytes
        .iter()
        .rposition(|&b| !is_py_ascii_space(b))
        .map_or(start, |p| p + 1);
    let mut s = &bytes[start..end];

    // 1. Optional sign
    let negative = match s.first() {
        Some(b'-') => {
            s = &s[1..];
            true
        }
        Some(b'+') => {
            s = &s[1..];
            false
        }
        _ => false,
    };
    let apply_sign = |val: f64| if negative { -val } else { val };

    // 2. Check for inf / infinity / nan (case-insensitive)
    if s.eq_ignore_ascii_case(b"inf") || s.eq_ignore_ascii_case(b"infinity") {
        return Ok(apply_sign(f64::INFINITY));
    }
    if s.eq_ignore_ascii_case(b"nan") {
        return Ok(apply_sign(f64::NAN));
    }

    // 3. Optional 0x / 0X prefix
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1], b'x' | b'X') {
        s = &s[2..];
    }

    // 4. Parse hex significand: accumulate up to 15 significant hex nibbles (<= 60 bits)
    // into `mantissa`, and collapse any further non-zero digits into a sticky bit.
    let mut mantissa: u64 = 0;
    let mut sig_nibbles: usize = 0;
    let mut sticky = false;
    let mut saw_digit = false;
    let mut saw_dot = false;
    // Base-2 exponent adjustment from hex digit positions (4 bits per nibble).
    let mut hex_exp: i64 = 0;

    let mut idx = 0;
    while idx < s.len() {
        let b = s[idx];
        if b == b'.' {
            if saw_dot {
                return Err(invalid_err());
            }
            saw_dot = true;
            idx += 1;
            continue;
        }
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => break,
        };
        saw_digit = true;
        if mantissa == 0 && nibble == 0 {
            if saw_dot {
                hex_exp = hex_exp.saturating_sub(4);
            }
        } else {
            if !saw_dot {
                hex_exp = hex_exp.saturating_add(4);
            }
            if sig_nibbles < 15 {
                mantissa = (mantissa << 4) | u64::from(nibble);
                sig_nibbles += 1;
            } else if nibble != 0 {
                sticky = true;
            }
        }
        idx += 1;
    }

    if !saw_digit {
        return Err(invalid_err());
    }

    // 5. Optional binary exponent: ('p' | 'P') ['+' | '-'] DIGIT+
    let mut p_exp: i64 = 0;
    if idx < s.len() {
        if !matches!(s[idx], b'p' | b'P') {
            return Err(invalid_err());
        }
        idx += 1;
        let exp_neg = match s.get(idx) {
            Some(b'-') => {
                idx += 1;
                true
            }
            Some(b'+') => {
                idx += 1;
                false
            }
            _ => false,
        };
        if idx >= s.len() || !s[idx].is_ascii_digit() {
            return Err(invalid_err());
        }
        while idx < s.len() {
            let b = s[idx];
            if !b.is_ascii_digit() {
                return Err(invalid_err());
            }
            p_exp = p_exp.saturating_mul(10).saturating_add(i64::from(b - b'0'));
            idx += 1;
        }
        if exp_neg {
            p_exp = -p_exp;
        }
    }

    if mantissa == 0 {
        return Ok(apply_sign(0.0));
    }

    let bit_len = i64::from(64 - mantissa.leading_zeros());
    let total_exp2 = p_exp
        .saturating_add(hex_exp)
        .saturating_sub(i64::try_from(sig_nibbles * 4).expect("<= 60"));
    let top_bit_exp = total_exp2.saturating_add(bit_len - 1);

    if top_bit_exp > 1023 {
        return Err(overflow_err());
    }
    if top_bit_exp < -1075 {
        return Ok(apply_sign(0.0));
    }

    let target_bits = if top_bit_exp >= -1022 {
        53_i64
    } else {
        top_bit_exp + 1075
    };

    let drop_bits = bit_len - target_bits;
    let (rounded, final_top_exp) = if drop_bits <= 0 {
        let shift = u32::try_from(-drop_bits).expect("<= 53");
        (mantissa << shift, top_bit_exp)
    } else {
        let shift = u32::try_from(drop_bits).expect("1..=61");
        let half = 1_u64 << (shift - 1);
        let mask = (1_u64 << shift) - 1;
        let rem = (mantissa & mask) | u64::from(sticky);
        let mut q = mantissa >> shift;
        if rem > half || (rem == half && (q & 1) != 0) {
            q += 1;
        }
        let mut exp = top_bit_exp;
        if target_bits > 0 && q == (1_u64 << u32::try_from(target_bits).expect("<= 53")) {
            if target_bits == 53 {
                q >>= 1;
                exp += 1;
            } else {
                exp += 1;
            }
        }
        (q, exp)
    };

    if final_top_exp > 1023 {
        return Err(overflow_err());
    }

    let result = if rounded == 0 {
        0.0
    } else if final_top_exp < -1022 {
        f64::from_bits(rounded)
    } else {
        let biased_exp = u64::try_from(final_top_exp + 1023).expect("1..=2046");
        let frac = rounded & ((1_u64 << 52) - 1);
        f64::from_bits((biased_exp << 52) | frac)
    };

    Ok(apply_sign(result))
}
