//! Python's `decimal.Decimal`: a port of CPython's `_pydecimal.py`.
//!
//! The representation mirrors `_pydecimal` field for field so its algorithms port
//! line for line: a finite value is `(-1)**sign * coeff * 10**exp`, with the
//! coefficient kept as its decimal digit string (`_pydecimal`'s `_int`), which makes
//! the digit-level rounding in [`Decimal::fix`] and the string forms direct.
//! Arithmetic converts the digits to a `BigInt` as `_pydecimal` does with `int()`.
//!
//! Every operation takes a [`Ctx`]: the settings of the `decimal.Context` it runs
//! under plus the conditions it raises. CPython's default `decimal` is the C
//! `_decimal` module, which records every condition of an operation and only then
//! checks the traps, so ported `context._raise_error(...)` calls record the condition
//! and continue with the untrapped result; the Python-facing layer (`py.rs`) applies
//! the conditions to the context and raises. Error messages follow `_decimal`.
//!
//! Sandbox guards: constructors cap coefficients at [`MAX_DIGITS`] digits, results
//! are bounded by the precision, and [`Ctx::check_digits`] / [`Ctx::check_time`]
//! bound the precision-scaled work. See `limitations/decimal.md`.

mod arith;
mod cmp;
pub(crate) mod context;
mod format;
mod methods;
pub(crate) mod parse;
mod pow;
pub(crate) mod py;
pub(crate) mod py_context;
mod trans;

use std::{borrow::Cow, cmp::Ordering, iter};

pub(crate) use context::{Ctx, DecContext};
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use crate::{
    exception_private::RunResult,
    heap::{HeapId, HeapItem},
    types::decimal::context::{Condition, Rounding},
};

/// Largest coefficient (or NaN payload) a constructor accepts, in digits.
///
/// The same bound as `int`'s string conversion limit: converting a digit string to a
/// binary integer is quadratic, and every operation does so. CPython has no such cap.
pub(crate) const MAX_DIGITS: usize = 4300;

/// The kinds of value a [`Decimal`] can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Special {
    /// A finite number, zero included.
    Finite,
    /// `Infinity`: the coefficient is `"0"` and the exponent `0`.
    Infinite,
    /// A quiet NaN; the coefficient is the diagnostic payload (`""` for none).
    QuietNan,
    /// A signaling NaN; any arithmetic use raises `InvalidOperation`.
    SignalingNan,
}

/// A `decimal.Decimal` value.
///
/// Immutable, holds no heap references, and the derived equality is structural
/// (`1.0` and `1.00` differ); Python comparison goes through `cmp.rs`.
/// Invariants: a finite coefficient has no leading zeros unless it is `"0"`; a NaN
/// payload has none at all; an infinity's coefficient is `"0"` and its exponent `0`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Decimal {
    /// `true` for negative values (`_pydecimal`'s `_sign == 1`).
    sign: bool,
    /// The coefficient's decimal digits, most significant first.
    coeff: Box<str>,
    /// The exponent of a finite value; `0` for the specials.
    exp: i64,
    special: Special,
}

impl Decimal {
    /// A finite value from its parts, `_pydecimal`'s `_dec_from_triple`.
    ///
    /// The coefficient must already be normalized (no leading zeros).
    pub(crate) fn from_triple(sign: bool, coeff: impl Into<Box<str>>, exp: i64) -> Self {
        let coeff = coeff.into();
        debug_assert!(
            coeff.as_ref() == "0" || (!coeff.is_empty() && !coeff.starts_with('0')),
            "unnormalized coefficient {coeff:?}"
        );
        Self {
            sign,
            coeff,
            exp,
            special: Special::Finite,
        }
    }

    /// A finite value from an integer coefficient.
    pub(crate) fn from_bigint_triple(sign: bool, coeff: &BigInt, exp: i64) -> Self {
        Self::from_triple(sign, int_to_coeff(coeff), exp)
    }

    /// An exact integer.
    pub(crate) fn from_bigint(value: &BigInt) -> Self {
        Self::from_triple(value.sign() == num_bigint::Sign::Minus, int_to_coeff(value), 0)
    }

    /// An exact small integer.
    pub(crate) fn from_i64(value: i64) -> Self {
        Self::from_triple(value < 0, value.unsigned_abs().to_string(), 0)
    }

    /// A signed zero with the given exponent.
    pub(crate) fn zero(sign: bool, exp: i64) -> Self {
        Self::from_triple(sign, "0", exp)
    }

    /// `±Infinity`.
    pub(crate) fn infinity(sign: bool) -> Self {
        Self {
            sign,
            coeff: "0".into(),
            exp: 0,
            special: Special::Infinite,
        }
    }

    /// A quiet NaN with no payload (`_pydecimal`'s `_NaN`).
    pub(crate) fn nan() -> Self {
        Self::special_nan(false, "", Special::QuietNan)
    }

    /// A NaN of the given kind with a normalized payload.
    pub(crate) fn special_nan(sign: bool, payload: impl Into<Box<str>>, kind: Special) -> Self {
        debug_assert!(matches!(kind, Special::QuietNan | Special::SignalingNan));
        Self {
            sign,
            coeff: payload.into(),
            exp: 0,
            special: kind,
        }
    }

    /// Whether the value is negative (including `-0`, `-Infinity` and negative NaNs).
    pub(crate) fn sign(&self) -> bool {
        self.sign
    }

    /// The coefficient digits (the payload of a NaN).
    pub(crate) fn coeff(&self) -> &str {
        &self.coeff
    }

    /// The exponent; `0` for specials.
    pub(crate) fn exponent(&self) -> i64 {
        self.exp
    }

    /// Which kind of value this is.
    pub(crate) fn special(&self) -> Special {
        self.special
    }

    /// The coefficient as an integer (the payload of a NaN, zero for none).
    pub(crate) fn int(&self) -> BigInt {
        coeff_to_int(&self.coeff)
    }

    /// Whether this is a NaN or an infinity (`_is_special`).
    pub(crate) fn is_special(&self) -> bool {
        self.special != Special::Finite
    }

    /// Whether this is a NaN of either kind.
    pub(crate) fn is_nan(&self) -> bool {
        matches!(self.special, Special::QuietNan | Special::SignalingNan)
    }

    /// Whether this is a quiet NaN.
    pub(crate) fn is_qnan(&self) -> bool {
        self.special == Special::QuietNan
    }

    /// Whether this is a signaling NaN.
    pub(crate) fn is_snan(&self) -> bool {
        self.special == Special::SignalingNan
    }

    /// Whether this is `±Infinity`.
    pub(crate) fn is_infinite(&self) -> bool {
        self.special == Special::Infinite
    }

    /// Whether this is finite.
    pub(crate) fn is_finite(&self) -> bool {
        self.special == Special::Finite
    }

    /// Whether this is a (signed) zero.
    pub(crate) fn is_zero(&self) -> bool {
        self.special == Special::Finite && &*self.coeff == "0"
    }

    /// Python truthiness: everything but zero is true, NaNs and infinities included.
    pub(crate) fn is_truthy(&self) -> bool {
        !self.is_zero()
    }

    /// `_isnan`: `0` for a number, `1` for a quiet NaN, `2` for a signaling one.
    pub(crate) fn nan_kind(&self) -> u8 {
        match self.special {
            Special::QuietNan => 1,
            Special::SignalingNan => 2,
            Special::Finite | Special::Infinite => 0,
        }
    }

    /// `_isinfinity`: `1` for `+Infinity`, `-1` for `-Infinity`, else `0`.
    pub(crate) fn infinity_sign(&self) -> i8 {
        match (self.special, self.sign) {
            (Special::Infinite, false) => 1,
            (Special::Infinite, true) => -1,
            _ => 0,
        }
    }

    /// The number of coefficient digits.
    pub(crate) fn digits(&self) -> i64 {
        len_i64(&self.coeff)
    }

    /// `adjusted()`: the exponent of the most significant digit; `0` for specials.
    pub(crate) fn adjusted(&self) -> i64 {
        if self.is_special() {
            0
        } else {
            self.exp + self.digits() - 1
        }
    }

    /// `_isinteger`: whether this is finite and integral.
    pub(crate) fn is_integer(&self) -> bool {
        if self.is_special() {
            false
        } else if self.exp >= 0 {
            true
        } else {
            all_zeros(&self.coeff, offset_from_end(&self.coeff, self.exp))
        }
    }

    /// `_iseven`: for an integral value, whether it is even.
    pub(crate) fn is_even(&self) -> bool {
        if self.is_zero() || self.exp > 0 {
            true
        } else {
            let index = offset_from_end(&self.coeff, self.exp - 1);
            matches!(self.coeff.as_bytes()[index], b'0' | b'2' | b'4' | b'6' | b'8')
        }
    }

    /// `copy_abs()`.
    pub(crate) fn copy_abs(&self) -> Self {
        Self {
            sign: false,
            ..self.clone()
        }
    }

    /// `copy_negate()`.
    pub(crate) fn copy_negate(&self) -> Self {
        Self {
            sign: !self.sign,
            ..self.clone()
        }
    }

    /// `copy_sign(other)`.
    pub(crate) fn copy_sign(&self, other: &Self) -> Self {
        Self {
            sign: other.sign,
            ..self.clone()
        }
    }

    /// `is_normal()`.
    pub(crate) fn is_normal(&self, ctx: &DecContext) -> bool {
        !self.is_special() && !self.is_zero() && ctx.emin <= self.adjusted()
    }

    /// `is_subnormal()`.
    pub(crate) fn is_subnormal(&self, ctx: &DecContext) -> bool {
        !self.is_special() && !self.is_zero() && self.adjusted() < ctx.emin
    }

    /// `_fix_nan`: truncates an oversized NaN payload to the precision.
    pub(crate) fn fix_nan(&self, ctx: &Ctx<'_>) -> Self {
        let max_payload = ctx.settings.prec - i64::from(ctx.settings.clamp);
        if self.digits() > max_payload {
            let keep = usize::try_from(max_payload).unwrap_or(0);
            let tail = &self.coeff[self.coeff.len() - keep..];
            Self::special_nan(self.sign, tail.trim_start_matches('0'), self.special)
        } else {
            self.clone()
        }
    }

    /// `_check_nans`: the result of an operation with a NaN operand, or `None`.
    ///
    /// A signaling NaN raises `InvalidOperation` and yields its quiet form; otherwise
    /// the first quiet NaN propagates.
    pub(crate) fn check_nans(&self, other: Option<&Self>, ctx: &mut Ctx<'_>) -> Option<Self> {
        let self_nan = self.nan_kind();
        let other_nan = other.map_or(0, Self::nan_kind);
        if self_nan == 0 && other_nan == 0 {
            None
        } else if self_nan == 2 {
            Some(ctx.invalid_nan(self))
        } else if other_nan == 2 {
            Some(ctx.invalid_nan(other.expect("other is a NaN")))
        } else if self_nan != 0 {
            Some(self.fix_nan(ctx))
        } else {
            Some(other.expect("other is a NaN").fix_nan(ctx))
        }
    }

    /// `_compare_check_nans`: for the signaling comparisons, any NaN operand raises
    /// `InvalidOperation` (signaling ones first) and yields a quiet NaN.
    pub(crate) fn compare_check_nans(&self, other: &Self, ctx: &mut Ctx<'_>) -> Option<Self> {
        if !self.is_special() && !other.is_special() {
            None
        } else if self.is_snan() {
            Some(ctx.invalid_nan(self))
        } else if other.is_snan() {
            Some(ctx.invalid_nan(other))
        } else if self.is_qnan() {
            Some(ctx.invalid_nan(self))
        } else if other.is_qnan() {
            Some(ctx.invalid_nan(other))
        } else {
            None
        }
    }

    /// `_fix`: rounds to the context precision and brings the exponent into range,
    /// raising `Clamped`, `Overflow`, `Underflow`, `Subnormal`, `Inexact` and `Rounded`.
    pub(crate) fn fix(&self, ctx: &mut Ctx<'_>) -> Self {
        if self.is_special() {
            return if self.is_nan() { self.fix_nan(ctx) } else { self.clone() };
        }

        let settings = ctx.settings;
        let etiny = settings.etiny();
        let etop = settings.etop();
        if self.is_zero() {
            let exp_max = if settings.clamp { etop } else { settings.emax };
            let new_exp = self.exp.max(etiny).min(exp_max);
            return if new_exp == self.exp {
                self.clone()
            } else {
                ctx.raise(Condition::Clamped);
                Self::zero(self.sign, new_exp)
            };
        }

        // The smallest allowable exponent of the result.
        let mut exp_min = self.digits() + self.exp - settings.prec;
        if exp_min > etop {
            let ans = ctx.overflow(self.sign);
            ctx.raise(Condition::Inexact);
            ctx.raise(Condition::Rounded);
            return ans;
        }

        let is_subnormal = exp_min < etiny;
        if is_subnormal {
            exp_min = etiny;
        }

        if self.exp < exp_min {
            // Round away the digits below `exp_min`.
            let digits = self.digits() + self.exp - exp_min;
            let (rounded, digits) = if digits < 0 {
                (Cow::Owned(Self::from_triple(self.sign, "1", exp_min - 1)), 0)
            } else {
                (Cow::Borrowed(self), digits)
            };
            let digits = usize::try_from(digits).expect("digits is non-negative");
            let changed = rounded.round_direction(digits, settings.rounding);
            let mut coeff: Cow<'_, str> = if digits == 0 {
                Cow::Borrowed("0")
            } else {
                Cow::Borrowed(&rounded.coeff[..digits])
            };
            if changed > 0 {
                let mut incremented = increment_digits(&coeff);
                if len_i64(&incremented) > settings.prec {
                    incremented.pop();
                    exp_min += 1;
                }
                coeff = Cow::Owned(incremented);
            }

            let ans = if exp_min > etop {
                ctx.overflow(self.sign)
            } else {
                Self::from_triple(self.sign, coeff.into_owned(), exp_min)
            };

            // Raise the signals in the specification's order of precedence.
            if changed != 0 && is_subnormal {
                ctx.raise(Condition::Underflow);
            }
            if is_subnormal {
                ctx.raise(Condition::Subnormal);
            }
            if changed != 0 {
                ctx.raise(Condition::Inexact);
            }
            ctx.raise(Condition::Rounded);
            if ans.is_zero() {
                // Underflow to zero.
                ctx.raise(Condition::Clamped);
            }
            return ans;
        }

        if is_subnormal {
            ctx.raise(Condition::Subnormal);
        }

        if settings.clamp && self.exp > etop {
            // Fold the exponent down into the coefficient.
            ctx.raise(Condition::Clamped);
            let pad = usize::try_from(self.exp - etop).expect("exp is above etop");
            let mut padded = String::with_capacity(self.coeff.len() + pad);
            padded.push_str(&self.coeff);
            padded.extend(iter::repeat_n('0', pad));
            return Self::from_triple(self.sign, padded, etop);
        }

        self.clone()
    }

    /// The `_round_*` functions: whether truncating the coefficient to `prec` digits
    /// (`0 <= prec < digits`) should round away from zero (`1`), drops only zeros
    /// (`0`), or drops non-zero digits towards zero (`-1`).
    pub(crate) fn round_direction(&self, prec: usize, rounding: Rounding) -> i8 {
        let digits = self.coeff.as_bytes();
        let round_down = || if all_zeros(&self.coeff, prec) { 0 } else { -1 };
        let half_up = || {
            if matches!(digits[prec], b'5'..=b'9') {
                1
            } else {
                round_down()
            }
        };
        match rounding {
            Rounding::Down => round_down(),
            Rounding::Up => -round_down(),
            Rounding::HalfUp => half_up(),
            Rounding::HalfDown => {
                if exact_half(&self.coeff, prec) {
                    -1
                } else {
                    half_up()
                }
            }
            Rounding::HalfEven => {
                let even_before = prec == 0 || matches!(digits[prec - 1], b'0' | b'2' | b'4' | b'6' | b'8');
                if exact_half(&self.coeff, prec) && even_before {
                    -1
                } else {
                    half_up()
                }
            }
            Rounding::Ceiling => {
                if self.sign {
                    round_down()
                } else {
                    -round_down()
                }
            }
            Rounding::Floor => {
                if self.sign {
                    -round_down()
                } else {
                    round_down()
                }
            }
            Rounding::Up05 => {
                if prec > 0 && !matches!(digits[prec - 1], b'0' | b'5') {
                    round_down()
                } else {
                    -round_down()
                }
            }
        }
    }

    /// `-x`, rounded to the context (`__neg__`).
    pub(crate) fn neg(&self, ctx: &mut Ctx<'_>) -> Self {
        if self.is_special()
            && let Some(ans) = self.check_nans(None, ctx)
        {
            return ans;
        }
        // `-Decimal('0')` is `0`, not `-0`, except when rounding towards -inf.
        let ans = if self.is_zero() && ctx.settings.rounding != Rounding::Floor {
            self.copy_abs()
        } else {
            self.copy_negate()
        };
        ans.fix(ctx)
    }

    /// `+x`, rounded to the context (`__pos__`).
    pub(crate) fn plus(&self, ctx: &mut Ctx<'_>) -> Self {
        if self.is_special()
            && let Some(ans) = self.check_nans(None, ctx)
        {
            return ans;
        }
        let ans = if self.is_zero() && ctx.settings.rounding != Rounding::Floor {
            self.copy_abs()
        } else {
            self.clone()
        };
        ans.fix(ctx)
    }

    /// `abs(x)`, rounded to the context (`__abs__`).
    pub(crate) fn abs(&self, ctx: &mut Ctx<'_>) -> Self {
        if self.is_special()
            && let Some(ans) = self.check_nans(None, ctx)
        {
            return ans;
        }
        if self.sign { self.neg(ctx) } else { self.plus(ctx) }
    }

    /// The scientific (`str`) or engineering (`to_eng_string`) string form.
    pub(crate) fn to_sci_string(&self, eng: bool, capitals: bool) -> String {
        let sign = if self.sign { "-" } else { "" };
        match self.special {
            Special::Infinite => return format!("{sign}Infinity"),
            Special::QuietNan => return format!("{sign}NaN{}", self.coeff),
            Special::SignalingNan => return format!("{sign}sNaN{}", self.coeff),
            Special::Finite => {}
        }

        // Digits of the coefficient left of the point, before any exponent.
        let leftdigits = self.exp + self.digits();
        // Digits of the coefficient left of the point in the printed mantissa.
        let dotplace = if self.exp <= 0 && leftdigits > -6 {
            leftdigits
        } else if !eng {
            1
        } else if self.is_zero() {
            (leftdigits + 1).rem_euclid(3) - 1
        } else {
            (leftdigits - 1).rem_euclid(3) + 1
        };

        let len = self.digits();
        let mut out = String::with_capacity(self.coeff.len() + 8);
        out.push_str(sign);
        if dotplace <= 0 {
            out.push_str("0.");
            push_zeros(&mut out, -dotplace);
            out.push_str(&self.coeff);
        } else if dotplace >= len {
            out.push_str(&self.coeff);
            push_zeros(&mut out, dotplace - len);
        } else {
            let split = usize::try_from(dotplace).expect("dotplace is positive");
            out.push_str(&self.coeff[..split]);
            out.push('.');
            out.push_str(&self.coeff[split..]);
        }
        if leftdigits != dotplace {
            out.push(if capitals { 'E' } else { 'e' });
            let exponent = leftdigits - dotplace;
            if exponent >= 0 {
                out.push('+');
            }
            out.push_str(&exponent.to_string());
        }
        out
    }

    /// `_cmp`: compares two non-NaN values numerically.
    pub(crate) fn cmp_numeric(&self, other: &Self) -> Ordering {
        cmp::cmp_numeric(self, other)
    }
}

impl HeapItem for Decimal {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

impl Ctx<'_> {
    /// Raises an `InvalidOperation` condition and returns its default result, `NaN`.
    pub(crate) fn invalid(&mut self, condition: Condition) -> Decimal {
        self.raise(condition);
        Decimal::nan()
    }

    /// Raises `InvalidOperation` for a signaling NaN operand and returns its quiet form.
    pub(crate) fn invalid_nan(&mut self, operand: &Decimal) -> Decimal {
        self.raise(Condition::InvalidOperation);
        Decimal::special_nan(operand.sign, operand.coeff.clone(), Special::QuietNan).fix_nan(self)
    }

    /// Raises `DivisionByZero` and returns its default result, a signed infinity.
    pub(crate) fn division_by_zero(&mut self, sign: bool) -> Decimal {
        self.raise(Condition::DivisionByZero);
        Decimal::infinity(sign)
    }

    /// Raises `Overflow` and returns its default result, which depends on the rounding:
    /// an infinity, or the largest finite value when rounding towards zero.
    pub(crate) fn overflow(&mut self, sign: bool) -> Decimal {
        self.raise(Condition::Overflow);
        let to_infinity = match self.settings.rounding {
            Rounding::HalfUp | Rounding::HalfEven | Rounding::HalfDown | Rounding::Up => true,
            Rounding::Ceiling => !sign,
            Rounding::Floor => sign,
            Rounding::Down | Rounding::Up05 => false,
        };
        if to_infinity {
            Decimal::infinity(sign)
        } else {
            let nines = "9".repeat(usize::try_from(self.settings.prec).unwrap_or(0));
            Decimal::from_triple(sign, nines, self.settings.etop())
        }
    }
}

/// Parses a coefficient digit string into an integer.
pub(crate) fn coeff_to_int(coeff: &str) -> BigInt {
    if coeff.is_empty() {
        BigInt::zero()
    } else if coeff.len() <= 19 {
        BigInt::from(coeff.parse::<u64>().expect("coefficient is decimal digits"))
    } else {
        BigInt::parse_bytes(coeff.as_bytes(), 10).expect("coefficient is decimal digits")
    }
}

/// Formats a non-negative integer as a coefficient digit string.
pub(crate) fn int_to_coeff(value: &BigInt) -> String {
    match value.to_u64() {
        Some(small) => small.to_string(),
        None => value.magnitude().to_string(),
    }
}

/// `str` length as `i64`; coefficients are far shorter than `i64::MAX`.
pub(crate) fn len_i64(s: &str) -> i64 {
    i64::try_from(s.len()).expect("coefficient length fits in i64")
}

/// `_all_zeros(s, start)`: whether every digit from `start` on is `0`.
pub(crate) fn all_zeros(s: &str, start: usize) -> bool {
    s.as_bytes()
        .get(start..)
        .is_none_or(|rest| rest.iter().all(|&b| b == b'0'))
}

/// `_exact_half(s, start)`: whether the digits from `start` on are `5` then zeros.
pub(crate) fn exact_half(s: &str, start: usize) -> bool {
    s.as_bytes().get(start) == Some(&b'5') && all_zeros(s, start + 1)
}

/// Resolves a Python-style negative index (`s[exp:]` with `exp < 0`) to an offset,
/// clamping to the start as slicing does.
fn offset_from_end(s: &str, negative: i64) -> usize {
    let back = usize::try_from(negative.unsigned_abs()).unwrap_or(usize::MAX);
    s.len().saturating_sub(back)
}

/// Adds one to a digit string, `str(int(coeff) + 1)`.
fn increment_digits(coeff: &str) -> String {
    let mut digits = coeff.as_bytes().to_vec();
    let mut i = digits.len();
    loop {
        if i == 0 {
            digits.insert(0, b'1');
            break;
        }
        i -= 1;
        if digits[i] == b'9' {
            digits[i] = b'0';
        } else {
            digits[i] += 1;
            break;
        }
    }
    String::from_utf8(digits).expect("digits are ASCII")
}

/// Appends `count` zeros (none if `count <= 0`).
pub(crate) fn push_zeros(out: &mut String, count: i64) {
    if count > 0 {
        out.extend(iter::repeat_n('0', usize::try_from(count).unwrap_or(0)));
    }
}

/// `10**n` for a non-negative `n` already bounded by the caller.
pub(crate) fn pow10(n: u64) -> BigInt {
    num_traits::Pow::pow(BigInt::from(10u8), n)
}

/// Checks the size of `10**n` against the memory limit, then computes it.
pub(crate) fn checked_pow10(n: u64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    ctx.check_digits(n)?;
    Ok(pow10(n))
}
