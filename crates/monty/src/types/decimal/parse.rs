//! `Decimal(...)` construction: the string parser (a hand-rolled port of
//! `_pydecimal._parser`'s regex), exact `float` conversion, `int`/`LongInt`
//! conversion, the `(sign, digits, exponent)` sequence form, and the
//! constructor entry point.
//!
//! This file owns **guard 1**: every way a coefficient (or NaN payload) can
//! enter the system caps its digit count at [`DECIMAL_MAX_DIGITS`] and its
//! exponent at the C module's literal bounds, so no other module ever sees an
//! unbounded operand.

use std::{borrow::Cow, str::from_utf8};

use num_bigint::BigInt;
use num_traits::Pow;

use super::{DECIMAL_MAX_DIGITS, Decimal, MAX_LITERAL_EXP, MIN_LITERAL_EXP, allocate};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::HeapData,
    resource::ResourceTracker,
    types::long_int::estimate_decimal_digits,
    value::Value,
};

/// Arguments of `Decimal(value='0')`. CPython also accepts a `context`
/// argument; Monty has no `Context` objects, so passing one (positionally or
/// by name) is a `TypeError` (see `limitations/decimal.md`).
#[derive(FromArgs)]
#[from_args(name = "Decimal", style = c)]
struct DecimalInitArgs {
    #[from_args(default)]
    value: Option<Value>,
}

/// Constructor for `decimal.Decimal(value='0')`.
///
/// Accepts the input types CPython's `Decimal(...)` does: `str`, `int` (incl.
/// `bool` and heap `LongInt`), `float` (exact binary expansion), another
/// `Decimal` (copied), and the `(sign, digits, exponent)` sequence form. Zero
/// args yields `Decimal('0')`. A type Monty cannot convert (`None`,
/// containers, …) raises CPython's `TypeError: conversion from {type} to
/// Decimal is not supported`; an unparsable string raises `InvalidOperation`
/// with the `[<class 'decimal.ConversionSyntax'>]` message.
pub(crate) fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let DecimalInitArgs { value } = DecimalInitArgs::from_args(args, vm)?;
    let Some(value) = value else {
        return allocate(Decimal::zero(), vm);
    };
    defer_drop!(value, vm);
    let d = decimal_from_value(value, vm)?;
    allocate(d, vm)
}

/// Converts a Python value into a [`Decimal`] for the constructor.
pub(super) fn decimal_from_value(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Decimal> {
    match value {
        // `bool` is an `int` subtype: `Decimal(True) == Decimal('1')`.
        Value::Bool(b) => Ok(Decimal::from_i64(i64::from(*b))),
        Value::Int(i) => Ok(Decimal::from_i64(*i)),
        Value::Float(f) => Ok(from_float(*f)),
        Value::InternString(id) => parse_str(vm.interns.get_str(*id)),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(s) => parse_str(s.as_str()),
            HeapData::LongInt(li) => decimal_from_bigint(li.inner()),
            HeapData::Decimal(d) => Ok(d.clone()),
            // The `(sign, digits, exponent)` tuple/list form (a `DecimalTuple`
            // from `as_tuple` is a NamedTuple, so it round-trips too).
            HeapData::Tuple(t) => decimal_from_sequence(t.as_slice(), vm),
            HeapData::List(l) => decimal_from_sequence(l.as_slice(), vm),
            HeapData::NamedTuple(nt) => decimal_from_sequence(nt.as_vec(), vm),
            _ => Err(ExcType::decimal_unsupported_conversion(value.py_type_name(vm))),
        },
        other => Err(ExcType::decimal_unsupported_conversion(other.py_type_name(vm))),
    }
}

/// Exact conversion of an `int` (as `BigInt`) — guarded by the digit cap:
/// CPython accepts `Decimal(10**100000)`, Monty rejects past
/// [`DECIMAL_MAX_DIGITS`] (documented divergence).
pub(super) fn decimal_from_bigint(n: &BigInt) -> RunResult<Decimal> {
    check_digit_cap_bits(n)?;
    let (sign, magnitude) = split_sign(n);
    Ok(Decimal::from_triple(sign, magnitude, 0))
}

/// Exact conversion of a `float`, matching `Decimal.from_float`: NaN/∞ map to
/// the decimal specials, a finite value to its *exact* binary expansion
/// (`Decimal(0.1)` is the 55-digit value). Bounded by construction: the
/// coefficient of any finite `f64` has ≤ 767 digits.
pub(super) fn from_float(f: f64) -> Decimal {
    if f.is_nan() {
        return Decimal::qnan(0, BigInt::ZERO);
    }
    let sign = u8::from(f.is_sign_negative());
    if f.is_infinite() {
        return Decimal::infinity(sign);
    }
    if f == 0.0 {
        return Decimal::from_triple(sign, BigInt::ZERO, 0);
    }
    // Decompose |f| = m · 2^e with m odd, then m · 2^e = (m · 5^-e) · 10^e for
    // negative e — CPython's `n * 5**k` with `10**-k` exponent.
    let bits = f.abs().to_bits();
    let raw_exp = i64::try_from((bits >> 52) & 0x7ff).expect("11-bit value fits i64");
    let mantissa = bits & ((1u64 << 52) - 1);
    let (mut m, mut e): (u64, i64) = if raw_exp == 0 {
        (mantissa, -1074) // subnormal: no implicit leading bit
    } else {
        (mantissa | (1 << 52), raw_exp - 1075)
    };
    let trailing = m.trailing_zeros();
    m >>= trailing;
    e += i64::from(trailing);
    if e >= 0 {
        Decimal::from_triple(sign, BigInt::from(m) << u64::try_from(e).expect("non-negative"), 0)
    } else {
        let k = u32::try_from(-e).expect("|e| <= 1074");
        Decimal::from_triple(sign, BigInt::from(m) * BigInt::from(5u8).pow(k), e)
    }
}

/// Parses a decimal string — the port of `_pydecimal._parser` plus its
/// pre-processing (`value.strip().replace('_', '')`): optional sign, then a
/// finite number (`digits[.digits][E±digits]`, needing at least one digit),
/// `Inf`/`Infinity`, or `[s]NaN[payload]`, all case-insensitive and
/// ASCII-only. An unparsable string raises CPython's
/// `InvalidOperation([ConversionSyntax])`; a coefficient or payload past the
/// digit cap raises the Monty-specific `ValueError`; an exponent outside the
/// C module's literal bounds raises `InvalidOperation`, as CPython does.
pub(super) fn parse_str(s: &str) -> RunResult<Decimal> {
    let trimmed = s.trim();
    // CPython removes every underscore before parsing, so `_1_2.3_` is 12.3.
    let cleaned: Cow<'_, str> = if trimmed.contains('_') {
        Cow::Owned(trimmed.replace('_', ""))
    } else {
        Cow::Borrowed(trimmed)
    };
    let bytes = cleaned.as_bytes();
    let (sign, rest) = match bytes.first() {
        Some(b'+') => (0u8, &bytes[1..]),
        Some(b'-') => (1u8, &bytes[1..]),
        _ => (0u8, bytes),
    };

    if rest.eq_ignore_ascii_case(b"inf") || rest.eq_ignore_ascii_case(b"infinity") {
        return Ok(Decimal::infinity(sign));
    }
    // `[s]NaN` with an optional all-digit payload.
    let nan_payload = if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(b"snan") {
        Some((true, &rest[4..]))
    } else if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"nan") {
        Some((false, &rest[3..]))
    } else {
        None
    };
    if let Some((signaling, payload)) = nan_payload {
        if !payload.iter().all(u8::is_ascii_digit) {
            return Err(ExcType::decimal_conversion_syntax());
        }
        let digits = strip_leading_zeros(payload);
        check_digit_cap(digits.len())?;
        let payload = parse_coefficient(digits);
        return Ok(if signaling {
            Decimal::snan(sign, payload)
        } else {
            Decimal::qnan(sign, payload)
        });
    }

    parse_finite(sign, rest)
}

/// Parses the finite-number production `digits[.digits][E±digits]` (at least
/// one coefficient digit required, matching the regex's `(?=\d|\.\d)`
/// lookahead).
fn parse_finite(sign: u8, rest: &[u8]) -> RunResult<Decimal> {
    let syntax_error = ExcType::decimal_conversion_syntax;
    // Split off the exponent part at the first `e`/`E`.
    let (mantissa, exp_lit) = match rest.iter().position(|&b| b == b'e' || b == b'E') {
        Some(pos) => {
            let exp_part = &rest[pos + 1..];
            let (exp_sign, exp_digits) = match exp_part.first() {
                Some(b'+') => (1i64, &exp_part[1..]),
                Some(b'-') => (-1i64, &exp_part[1..]),
                _ => (1i64, exp_part),
            };
            if exp_digits.is_empty() || !exp_digits.iter().all(u8::is_ascii_digit) {
                return Err(syntax_error());
            }
            // An exponent literal too large for i64 is far outside the C
            // module's bounds: same `InvalidOperation` as any out-of-bounds
            // exponent (the digits are already validated, so this is not a
            // syntax error).
            let magnitude = from_utf8(exp_digits)
                .expect("ASCII digits")
                .parse::<i64>()
                .map_err(|_| ExcType::decimal_invalid_operation())?;
            (&rest[..pos], exp_sign * magnitude)
        }
        None => (rest, 0i64),
    };

    // Split the coefficient at the decimal point; both sides all-digits, at
    // least one digit overall.
    let (int_part, frac_part) = match mantissa.iter().position(|&b| b == b'.') {
        Some(pos) => (&mantissa[..pos], &mantissa[pos + 1..]),
        None => (mantissa, [].as_slice()),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(syntax_error());
    }
    if !int_part.iter().all(u8::is_ascii_digit) || !frac_part.iter().all(u8::is_ascii_digit) {
        return Err(syntax_error());
    }

    // Coefficient = int digits ++ frac digits, leading zeros stripped (cheaply,
    // before the BigInt parse, so a megabyte of zeros costs a scan, not a
    // quadratic parse). The exponent shifts down by the fraction length.
    let mut digits = Vec::with_capacity(int_part.len() + frac_part.len());
    digits.extend_from_slice(int_part);
    digits.extend_from_slice(frac_part);
    let digits = strip_leading_zeros(&digits);
    check_digit_cap(digits.len())?;
    let frac_len = i64::try_from(frac_part.len()).map_err(|_| ExcType::decimal_invalid_operation())?;
    let exp = exp_lit
        .checked_sub(frac_len)
        .ok_or_else(ExcType::decimal_invalid_operation)?;

    check_literal_exponent_bounds(exp, digits.len())?;

    Ok(Decimal::from_triple(sign, parse_coefficient(digits), exp))
}

/// The C module's literal bounds, shared by the string parser and the
/// `(sign, digits, exponent)` tuple form: the adjusted exponent may not exceed
/// `MAX_LITERAL_EXP` and the exponent may not undershoot `MIN_LITERAL_EXP` —
/// CPython raises `InvalidOperation` for either. Also the sandbox guard that
/// keeps every stored exponent safe for downstream i64 arithmetic.
fn check_literal_exponent_bounds(exp: i64, digits: usize) -> RunResult<()> {
    let ndigits = i64::try_from(digits.max(1)).expect("digit count fits i64");
    if exp < MIN_LITERAL_EXP || exp.saturating_add(ndigits - 1) > MAX_LITERAL_EXP {
        Err(ExcType::decimal_invalid_operation())
    } else {
        Ok(())
    }
}

/// Constructs a `Decimal` from the `(sign, digits, exponent)` sequence form,
/// e.g. `Decimal((0, (1, 2, 0), -2)) == Decimal('1.20')`.
///
/// `sign` is `0`/`1` (a `bool` is accepted, being an `int` subtype); `digits`
/// is a tuple/list of single `0..=9` digits; `exponent` is an `int`, or
/// `'F'`/`'n'`/`'N'` for ∞ / NaN / sNaN (for `'F'` the digits are ignored; for
/// the NaN forms they become the payload). Each malformed element raises
/// CPython's exact `ValueError`; an exponent beyond i64 raises CPython's
/// `OverflowError`.
fn decimal_from_sequence(items: &[Value], vm: &VM<'_, impl ResourceTracker>) -> RunResult<Decimal> {
    let heap = &vm.heap;
    let [sign_value, digits_value, exponent_value] = items else {
        return Err(decimal_sequence_error("argument must be a sequence of length 3"));
    };
    let sign = match sign_value {
        Value::Int(0) | Value::Bool(false) => 0u8,
        Value::Int(1) | Value::Bool(true) => 1u8,
        _ => return Err(decimal_sequence_error("sign must be an integer with the value 0 or 1")),
    };

    // Concatenate the coefficient digits (each a single `0..=9` int, `bool`
    // accepted as `0`/`1`). A non-sequence `digits` or a non-digit element is
    // a `ValueError`, matching CPython (not the parser's `ConversionSyntax`).
    let digit_items = match digits_value {
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Tuple(t) => t.as_slice(),
            HeapData::List(l) => l.as_slice(),
            _ => return Err(decimal_sequence_error(COEFFICIENT_NOT_DIGITS)),
        },
        _ => return Err(decimal_sequence_error(COEFFICIENT_NOT_DIGITS)),
    };
    let mut coefficient = Vec::with_capacity(digit_items.len());
    for digit in digit_items {
        let value = match digit {
            Value::Int(n @ 0..=9) => *n,
            Value::Bool(b) => i64::from(*b),
            _ => return Err(decimal_sequence_error(COEFFICIENT_NOT_DIGITS)),
        };
        coefficient.push(b'0' + u8::try_from(value).expect("0..=9"));
    }
    let coefficient = strip_leading_zeros(&coefficient);
    check_digit_cap(coefficient.len())?;

    // The exponent is an `int` (`bool` included), or a special-value marker
    // string `'F'`/`'n'`/`'N'` (which may be interned or heap-allocated). An
    // `int` exponent must satisfy the same C-module literal bounds as the
    // string parser — an unbounded exponent would otherwise overflow the i64
    // exponent arithmetic downstream (CPython raises `InvalidOperation` too).
    match exponent_value {
        Value::Int(exp) => {
            check_literal_exponent_bounds(*exp, coefficient.len())?;
            Ok(Decimal::from_triple(sign, parse_coefficient(coefficient), *exp))
        }
        Value::Bool(b) => Ok(Decimal::from_triple(
            sign,
            parse_coefficient(coefficient),
            i64::from(*b),
        )),
        // A heap `LongInt` exponent is beyond i64: CPython's `OverflowError`.
        Value::Ref(id) if let HeapData::LongInt(_) = heap.get(*id) => Err(ExcType::int_too_large_for_ssize_t()),
        other => {
            let marker = match other {
                Value::InternString(id) => Some(vm.interns.get_str(*id)),
                Value::Ref(id) if let HeapData::Str(s) = heap.get(*id) => Some(s.as_str()),
                _ => None,
            };
            match marker {
                Some("F") => Ok(Decimal::infinity(sign)),
                Some("n") => Ok(Decimal::qnan(sign, parse_coefficient(coefficient))),
                Some("N") => Ok(Decimal::snan(sign, parse_coefficient(coefficient))),
                // A string in the third position must be a valid marker;
                // anything else (e.g. a `float` exponent) "must be an integer"
                // — distinct CPython messages.
                Some(_) => Err(decimal_sequence_error(
                    "string argument in the third position must be 'F', 'n' or 'N'",
                )),
                None => Err(decimal_sequence_error("exponent must be an integer")),
            }
        }
    }
}

/// CPython's `ValueError` message for a non-digit-tuple coefficient in the
/// sequence form (used for both a non-sequence `digits` and a non-digit
/// element).
const COEFFICIENT_NOT_DIGITS: &str = "coefficient must be a tuple of digits";

/// A `ValueError` for a malformed `Decimal((sign, digits, exponent))`
/// sequence, carrying CPython's exact wording for the offending element.
fn decimal_sequence_error(message: &str) -> RunError {
    SimpleException::new_msg(ExcType::ValueError, message.to_owned()).into()
}

/// The digit slice without leading `'0'`s (empty for an all-zero slice).
fn strip_leading_zeros(digits: &[u8]) -> &[u8] {
    let start = digits.iter().position(|&b| b != b'0').unwrap_or(digits.len());
    &digits[start..]
}

/// Parses a validated ASCII-digit slice into the coefficient `BigInt`
/// (`BigInt::ZERO` for an empty slice).
fn parse_coefficient(digits: &[u8]) -> BigInt {
    if digits.is_empty() {
        BigInt::ZERO
    } else {
        BigInt::parse_bytes(digits, 10).expect("validated ASCII digits")
    }
}

/// Guard 1 for digit counts already known exactly.
fn check_digit_cap(digits: usize) -> RunResult<()> {
    if digits > DECIMAL_MAX_DIGITS {
        Err(ExcType::decimal_digits_limit())
    } else {
        Ok(())
    }
}

/// Guard 1 for a `BigInt` operand, using the bit length so a huge `int` is
/// rejected without ever stringifying it (`digits ≈ bits · log10(2)`; the
/// bound errs high by < 1 digit, and the exact check settles the boundary).
fn check_digit_cap_bits(n: &BigInt) -> RunResult<()> {
    // The estimate is a strict upper bound on the digit count, so an in-cap
    // estimate proves the operand fits with no stringify at all; only the
    // one-digit boundary window needs the exact count.
    let approx_digits = estimate_decimal_digits(n.bits());
    if approx_digits <= DECIMAL_MAX_DIGITS as u64 {
        Ok(())
    } else if approx_digits > DECIMAL_MAX_DIGITS as u64 + 1 {
        Err(ExcType::decimal_digits_limit())
    } else {
        check_digit_cap(n.magnitude().to_string().len())
    }
}

/// Splits a `BigInt` into `(decimal sign, magnitude)`.
fn split_sign(n: &BigInt) -> (u8, BigInt) {
    if n.sign() == num_bigint::Sign::Minus {
        (1, -n)
    } else {
        (0, n.clone())
    }
}
