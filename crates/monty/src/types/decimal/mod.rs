//! Python's `decimal.Decimal`: a port of CPython's `_pydecimal.py`.
//!
//! The representation mirrors `_pydecimal` so its algorithms port line for line: a
//! finite value is `(-1)**sign * coeff * 10**exp`. The coefficient is a [`Coeff`],
//! stored in base `10**19` as libmpdec stores it, so the digit-level work that
//! dominates decimal arithmetic (rounding in [`Decimal::fix`], aligning exponents,
//! counting digits, printing) is word moves rather than binary multiplications.
//! The transcendental kernels work on `BigInt`s, as `_pydecimal` does with `int()`.
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
pub(crate) mod coeff;
pub(crate) mod context;
mod format;
mod methods;
pub(crate) mod parse;
mod pow;
pub(crate) mod py;
pub(crate) mod py_context;
mod trans;

use std::{cmp::Ordering, iter};

pub(crate) use coeff::{Coeff, Discarded};
pub(crate) use context::{Ctx, DecContext};
use num_bigint::BigInt;

use crate::{
    exception_private::RunResult,
    heap::{HeapId, HeapItem},
    types::decimal::context::{Condition, Rounding},
};

/// Largest coefficient (or NaN payload) a constructor accepts, in digits.
///
/// The same bound as `int`'s string conversion limit: `int` operands and the
/// transcendental kernels convert coefficients to binary, which is quadratic in the
/// digit count. CPython has no such cap.
pub(crate) const MAX_DIGITS: usize = 4300;

/// The kinds of value a [`Decimal`] can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Special {
    /// A finite number, zero included.
    Finite,
    /// `Infinity`: the coefficient is zero and the exponent `0`.
    Infinite,
    /// A quiet NaN; the coefficient is the diagnostic payload (zero for none).
    QuietNan,
    /// A signaling NaN; any arithmetic use raises `InvalidOperation`.
    SignalingNan,
}

/// A `decimal.Decimal` value.
///
/// Immutable, holds no heap references, and the derived equality is structural
/// (`1.0` and `1.00` differ); Python comparison goes through `cmp.rs`.
/// Invariants: an infinity's coefficient is zero and its exponent `0`; a NaN's
/// exponent is `0` and a zero payload means none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Decimal {
    /// `true` for negative values (`_pydecimal`'s `_sign == 1`).
    sign: bool,
    /// The coefficient (the payload of a NaN).
    coeff: Coeff,
    /// The exponent of a finite value; `0` for the specials.
    exp: i64,
    special: Special,
}

impl Decimal {
    /// The largest finite magnitude the context allows, `prec` nines at `Etop`:
    /// the overflow result when rounding towards zero, and `next_*` from an infinity.
    pub(crate) fn largest_finite(sign: bool, ctx: &Ctx<'_>) -> RunResult<Self> {
        let prec = ctx.settings.prec.unsigned_abs();
        ctx.check_digits(prec)?;
        Ok(Self::from_triple(
            sign,
            Coeff::repeat_digit(9, prec),
            ctx.settings.etop(),
        ))
    }

    /// A finite value from its parts, `_pydecimal`'s `_dec_from_triple`.
    pub(crate) fn from_triple(sign: bool, coeff: Coeff, exp: i64) -> Self {
        Self {
            sign,
            coeff,
            exp,
            special: Special::Finite,
        }
    }

    /// A finite value from an integer coefficient; its sign is ignored.
    pub(crate) fn from_bigint_triple(sign: bool, coeff: &BigInt, exp: i64) -> Self {
        Self::from_triple(sign, Coeff::from_bigint(coeff), exp)
    }

    /// An exact integer.
    pub(crate) fn from_bigint(value: &BigInt) -> Self {
        Self::from_bigint_triple(value.sign() == num_bigint::Sign::Minus, value, 0)
    }

    /// An exact small integer.
    pub(crate) fn from_i64(value: i64) -> Self {
        Self::from_triple(value < 0, Coeff::from_u64(value.unsigned_abs()), 0)
    }

    /// A signed zero with the given exponent.
    pub(crate) fn zero(sign: bool, exp: i64) -> Self {
        Self::from_triple(sign, Coeff::zero(), exp)
    }

    /// `±Infinity`.
    pub(crate) fn infinity(sign: bool) -> Self {
        Self {
            sign,
            coeff: Coeff::zero(),
            exp: 0,
            special: Special::Infinite,
        }
    }

    /// A quiet NaN with no payload (`_pydecimal`'s `_NaN`).
    pub(crate) fn nan() -> Self {
        Self::special_nan(false, Coeff::zero(), Special::QuietNan)
    }

    /// A NaN of the given kind; a zero payload means none.
    pub(crate) fn special_nan(sign: bool, payload: Coeff, kind: Special) -> Self {
        debug_assert!(matches!(kind, Special::QuietNan | Special::SignalingNan));
        Self {
            sign,
            coeff: payload,
            exp: 0,
            special: kind,
        }
    }

    /// Whether the value is negative (including `-0`, `-Infinity` and negative NaNs).
    pub(crate) fn sign(&self) -> bool {
        self.sign
    }

    /// The coefficient (the payload of a NaN).
    pub(crate) fn coeff(&self) -> &Coeff {
        &self.coeff
    }

    /// The coefficient's digits as `_pydecimal`'s `_int` holds them: a NaN without a
    /// payload has none.
    pub(crate) fn coeff_digits(&self) -> String {
        if self.is_nan() && self.coeff.is_zero() {
            String::new()
        } else {
            self.coeff.to_string()
        }
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
        self.coeff.to_bigint()
    }

    /// Whether this is a NaN or an infinity (`_is_special`).
    #[inline]
    pub(crate) fn is_special(&self) -> bool {
        self.special != Special::Finite
    }

    /// Whether this is a NaN of either kind.
    #[inline]
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
    #[inline]
    pub(crate) fn is_zero(&self) -> bool {
        self.special == Special::Finite && self.coeff.is_zero()
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

    /// The number of coefficient digits; none for a NaN without a payload.
    #[inline]
    pub(crate) fn digits(&self) -> i64 {
        if self.is_nan() && self.coeff.is_zero() {
            0
        } else {
            i64::try_from(self.coeff.digits()).expect("digit count fits in i64")
        }
    }

    /// `adjusted()`: the exponent of the most significant digit; `0` for specials.
    #[inline]
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
        } else {
            self.exp >= 0 || self.coeff.is_zero() || self.coeff.trailing_zeros() >= self.exp.unsigned_abs()
        }
    }

    /// `_iseven`: for an integral value, whether it is even.
    pub(crate) fn is_even(&self) -> bool {
        self.is_zero() || self.exp > 0 || self.coeff.digit_at(self.exp.unsigned_abs()).is_multiple_of(2)
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
            let tail = self.coeff.low_digits(max_payload.unsigned_abs());
            Self::special_nan(self.sign, tail, self.special)
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
    pub(crate) fn fix(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special() {
            return Ok(if self.is_nan() { self.fix_nan(ctx) } else { self.clone() });
        }

        let settings = ctx.settings;
        let etiny = settings.etiny();
        let etop = settings.etop();
        if self.is_zero() {
            let exp_max = if settings.clamp { etop } else { settings.emax };
            let new_exp = self.exp.max(etiny).min(exp_max);
            return Ok(if new_exp == self.exp {
                self.clone()
            } else {
                ctx.raise(Condition::Clamped);
                Self::zero(self.sign, new_exp)
            });
        }

        // The smallest allowable exponent of the result.
        let mut exp_min = self.digits() + self.exp - settings.prec;
        if exp_min > etop {
            let answer = ctx.overflow(self.sign)?;
            ctx.raise(Condition::Inexact);
            ctx.raise(Condition::Rounded);
            return Ok(answer);
        }

        let is_subnormal = exp_min < etiny;
        if is_subnormal {
            exp_min = etiny;
        }

        if self.exp < exp_min {
            // Round away the digits below `exp_min`.
            let (mut coeff, discarded) = self.coeff.shr(exp_min.abs_diff(self.exp));
            let changed = round_direction(&coeff, discarded, self.sign, settings.rounding);
            if changed > 0 {
                coeff.increment();
                if i64::try_from(coeff.digits()).is_ok_and(|digits| digits > settings.prec) {
                    // `99..9` rounded up to `10..0`: drop the extra zero.
                    coeff = coeff.shr(1).0;
                    exp_min += 1;
                }
            }

            let answer = if exp_min > etop {
                ctx.overflow(self.sign)?
            } else {
                Self::from_triple(self.sign, coeff, exp_min)
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
            if answer.is_zero() {
                // Underflow to zero.
                ctx.raise(Condition::Clamped);
            }
            return Ok(answer);
        }

        if is_subnormal {
            ctx.raise(Condition::Subnormal);
        }

        if settings.clamp && self.exp > etop {
            // Fold the exponent down into the coefficient.
            ctx.raise(Condition::Clamped);
            let pad = self.exp.abs_diff(etop);
            ctx.check_digits(self.coeff.digits().saturating_add(pad))?;
            return Ok(Self::from_triple(self.sign, self.coeff.shl(pad), etop));
        }

        Ok(self.clone())
    }

    /// `-x`, rounded to the context (`__neg__`).
    pub(crate) fn neg(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special()
            && let Some(answer) = self.check_nans(None, ctx)
        {
            return Ok(answer);
        }
        // `-Decimal('0')` is `0`, not `-0`, except when rounding towards -inf.
        let answer = if self.is_zero() && ctx.settings.rounding != Rounding::Floor {
            self.copy_abs()
        } else {
            self.copy_negate()
        };
        answer.fix(ctx)
    }

    /// `+x`, rounded to the context (`__pos__`).
    pub(crate) fn plus(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special()
            && let Some(answer) = self.check_nans(None, ctx)
        {
            return Ok(answer);
        }
        let answer = if self.is_zero() && ctx.settings.rounding != Rounding::Floor {
            self.copy_abs()
        } else {
            self.clone()
        };
        answer.fix(ctx)
    }

    /// `abs(x)`, rounded to the context (`__abs__`).
    pub(crate) fn abs(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special()
            && let Some(answer) = self.check_nans(None, ctx)
        {
            return Ok(answer);
        }
        if self.sign { self.neg(ctx) } else { self.plus(ctx) }
    }

    /// The scientific (`str`) or engineering (`to_eng_string`) string form.
    pub(crate) fn to_sci_string(&self, eng: bool, capitals: bool) -> String {
        let sign = if self.sign { "-" } else { "" };
        match self.special {
            Special::Infinite => return format!("{sign}Infinity"),
            Special::QuietNan => return format!("{sign}NaN{}", self.coeff_digits()),
            Special::SignalingNan => return format!("{sign}sNaN{}", self.coeff_digits()),
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
        let mut out = String::with_capacity(usize::try_from(len).unwrap_or(0) + 8);
        out.push_str(sign);
        if dotplace <= 0 {
            out.push_str("0.");
            push_zeros(&mut out, -dotplace);
            self.coeff.write_digits(&mut out);
        } else if dotplace >= len {
            self.coeff.write_digits(&mut out);
            push_zeros(&mut out, dotplace - len);
        } else {
            let split = usize::try_from(dotplace).expect("dotplace is positive");
            let start = out.len();
            self.coeff.write_digits(&mut out);
            out.insert(start + split, '.');
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
    pub(crate) fn overflow(&mut self, sign: bool) -> RunResult<Decimal> {
        self.raise(Condition::Overflow);
        let to_infinity = match self.settings.rounding {
            Rounding::HalfUp | Rounding::HalfEven | Rounding::HalfDown | Rounding::Up => true,
            Rounding::Ceiling => !sign,
            Rounding::Floor => sign,
            Rounding::Down | Rounding::Up05 => false,
        };
        if to_infinity {
            Ok(Decimal::infinity(sign))
        } else {
            Decimal::largest_finite(sign, self)
        }
    }
}

/// The `_round_*` functions: whether rounding the kept digits `kept` of a value with
/// sign `sign`, given what was discarded, rounds away from zero (`1`), was exact (`0`)
/// or truncates towards zero (`-1`).
pub(crate) fn round_direction(kept: &Coeff, discarded: Discarded, sign: bool, rounding: Rounding) -> i8 {
    let round_down = if discarded.is_zero() { 0 } else { -1 };
    let half_up = if discarded.first >= 5 { 1 } else { round_down };
    match rounding {
        Rounding::Down => round_down,
        Rounding::Up => -round_down,
        Rounding::HalfUp => half_up,
        Rounding::HalfDown => {
            if discarded.is_half() {
                -1
            } else {
                half_up
            }
        }
        Rounding::HalfEven => {
            if discarded.is_half() && kept.last_digit().is_multiple_of(2) {
                -1
            } else {
                half_up
            }
        }
        Rounding::Ceiling => {
            if sign {
                round_down
            } else {
                -round_down
            }
        }
        Rounding::Floor => {
            if sign {
                -round_down
            } else {
                round_down
            }
        }
        Rounding::Up05 => {
            if matches!(kept.last_digit(), 0 | 5) {
                -round_down
            } else {
                round_down
            }
        }
    }
}

/// `str` length as `i64`.
pub(crate) fn len_i64(s: &str) -> i64 {
    i64::try_from(s.len()).expect("string length fits in i64")
}

/// Appends `count` zeros (none if `count <= 0`).
pub(crate) fn push_zeros(out: &mut String, count: i64) {
    if count > 0 {
        out.extend(iter::repeat_n('0', usize::try_from(count).unwrap_or(0)));
    }
}

/// `10**n` for a non-negative `n` already bounded by the caller.
pub(crate) fn pow10(n: u64) -> BigInt {
    // `10**38` is the largest power of ten a `u128` holds.
    match u32::try_from(n) {
        Ok(small) if small <= 38 => BigInt::from(10u128.pow(small)),
        _ => num_traits::Pow::pow(BigInt::from(10u8), n),
    }
}

/// Checks the size of `10**n` against the memory limit, then computes it.
pub(crate) fn checked_pow10(n: u64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    ctx.check_digits(n)?;
    Ok(pow10(n))
}
