//! Rounding and representation methods: `quantize`, `to_integral*`, `normalize`,
//! `round(d, n)`, the `next_*` family, `logb` / `scaleb`, `rotate` / `shift`, the
//! logical operations, `number_class` and the conversions to `float` and ratios.

use std::{cmp::Ordering, iter};

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Pow, Zero};

use super::{
    Coeff, Ctx, Decimal,
    context::{Condition, DecContext, Rounding},
    round_direction,
};
use crate::exception_private::{ExcType, ExcTypeExt, RunResult};

impl Decimal {
    /// `quantize(exp, rounding)`; `None` uses the context's rounding.
    pub(crate) fn quantize(&self, exp: &Self, rounding: Option<Rounding>, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let rounding = rounding.unwrap_or(ctx.settings.rounding);
        if self.is_special() || exp.is_special() {
            if let Some(answer) = self.check_nans(Some(exp), ctx) {
                return Ok(answer);
            }
            if exp.is_infinite() || self.is_infinite() {
                return Ok(if exp.is_infinite() && self.is_infinite() {
                    self.clone()
                } else {
                    ctx.invalid(Condition::InvalidOperation)
                });
            }
        }
        let settings = ctx.settings;
        let target = exp.exponent();
        if !(settings.etiny() <= target && target <= settings.emax) {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        if self.is_zero() {
            return Self::zero(self.sign(), target).fix(ctx);
        }
        let self_adjusted = self.adjusted();
        if self_adjusted > settings.emax || self_adjusted - target + 1 > settings.prec {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        let answer = self.rescale(target, rounding, ctx)?;
        if answer.adjusted() > settings.emax || answer.digits() > settings.prec {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        if !answer.is_zero() && answer.adjusted() < settings.emin {
            ctx.raise(Condition::Subnormal);
        }
        if answer.exponent() > self.exponent() {
            if answer.cmp_numeric(self).is_ne() {
                ctx.raise(Condition::Inexact);
            }
            ctx.raise(Condition::Rounded);
        }
        // `fix` folds the exponent down under `clamp` and signals `Clamped`.
        answer.fix(ctx)
    }

    /// `same_quantum(other)`: equal exponents, or both NaNs, or both infinities.
    pub(crate) fn same_quantum(&self, other: &Self) -> bool {
        if self.is_special() || other.is_special() {
            (self.is_nan() && other.is_nan()) || (self.is_infinite() && other.is_infinite())
        } else {
            self.exponent() == other.exponent()
        }
    }

    /// `_rescale(exp, rounding)`: this finite value with exponent `exp`, rounded.
    /// Never signals; specials are returned unchanged.
    pub(crate) fn rescale(&self, exp: i64, rounding: Rounding, ctx: &Ctx<'_>) -> RunResult<Self> {
        if self.is_special() {
            return Ok(self.clone());
        }
        if self.is_zero() {
            return Ok(Self::zero(self.sign(), exp));
        }
        if self.exponent() >= exp {
            // Pad the coefficient with zeros.
            let pad = self.exponent().abs_diff(exp);
            ctx.check_digits(pad.saturating_add(self.coeff().digits()))?;
            return Ok(Self::from_triple(self.sign(), self.coeff().shl(pad), exp));
        }
        // Too many digits: round them away.
        let (mut coeff, discarded) = self.coeff().shr(exp.abs_diff(self.exponent()));
        if round_direction(&coeff, discarded, self.sign(), rounding) == 1 {
            coeff.increment();
        }
        Ok(Self::from_triple(self.sign(), coeff, exp))
    }

    /// `to_integral_exact(rounding)`: rounds to an integer, raising `Inexact` and
    /// `Rounded` as appropriate.
    pub(crate) fn to_integral_exact(&self, rounding: Option<Rounding>, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special() {
            return Ok(self.check_nans(None, ctx).unwrap_or_else(|| self.clone()));
        }
        if self.exponent() >= 0 {
            return Ok(self.clone());
        }
        if self.is_zero() {
            return Ok(Self::zero(self.sign(), 0));
        }
        let rounding = rounding.unwrap_or(ctx.settings.rounding);
        let answer = self.rescale(0, rounding, ctx)?;
        if answer.cmp_numeric(self).is_ne() {
            ctx.raise(Condition::Inexact);
        }
        ctx.raise(Condition::Rounded);
        Ok(answer)
    }

    /// `to_integral_value(rounding)` (also `to_integral`): rounds to an integer
    /// without raising `Inexact` or `Rounded`.
    pub(crate) fn to_integral_value(&self, rounding: Option<Rounding>, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let rounding = rounding.unwrap_or(ctx.settings.rounding);
        if self.is_special() {
            Ok(self.check_nans(None, ctx).unwrap_or_else(|| self.clone()))
        } else if self.exponent() >= 0 {
            Ok(self.clone())
        } else {
            self.rescale(0, rounding, ctx)
        }
    }

    /// `round(d, n)`: quantize to `10**-n` under the context.
    pub(crate) fn round_digits(&self, n: i64, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let exp = Self::from_triple(false, Coeff::one(), n.checked_neg().unwrap_or(i64::MAX));
        self.quantize(&exp, None, ctx)
    }

    /// `normalize()`: rounds to the context and strips trailing zeros.
    pub(crate) fn normalize(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special()
            && let Some(answer) = self.check_nans(None, ctx)
        {
            return Ok(answer);
        }
        let dup = self.fix(ctx)?;
        if dup.is_infinite() {
            return Ok(dup);
        }
        if dup.is_zero() {
            return Ok(Self::zero(dup.sign(), 0));
        }
        let exp_max = if ctx.settings.clamp {
            ctx.settings.etop()
        } else {
            ctx.settings.emax
        };
        // Strip trailing zeros, but not past the largest exponent.
        let room = u64::try_from(exp_max - dup.exponent()).unwrap_or(0);
        let strip = dup.coeff().trailing_zeros().min(room);
        let exp = dup.exponent() + i64::try_from(strip).expect("bounded by the coefficient length");
        Ok(Self::from_triple(dup.sign(), dup.coeff().shr(strip).0, exp))
    }

    /// `next_minus()`: the largest representable number below this one.
    pub(crate) fn next_minus(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.next(ctx, true)
    }

    /// `next_plus()`: the smallest representable number above this one.
    pub(crate) fn next_plus(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.next(ctx, false)
    }

    /// The shared body of `next_minus` / `next_plus`: round towards the neighbour,
    /// or step by less than one unit of the smallest exponent. Raises no flags.
    fn next(&self, ctx: &mut Ctx<'_>, down: bool) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(None, ctx) {
            return Ok(answer);
        }
        let settings = ctx.settings;
        match (self.infinity_sign(), down) {
            (-1, true) | (1, false) => return Ok(self.clone()),
            (1, true) | (-1, false) => return Self::largest_finite(!down, ctx),
            _ => {}
        }
        // `_pydecimal` works on a flag-ignoring copy of the context.
        let mut quiet = ctx.with_rounding(if down { Rounding::Floor } else { Rounding::Ceiling });
        let new_self = self.fix(&mut quiet)?;
        // A numeric comparison: `1.0` rounding to `1` is no step at all.
        if new_self.cmp_numeric(self).is_ne() {
            return Ok(new_self);
        }
        let tiny = Self::from_triple(false, Coeff::one(), settings.etiny() - 1);
        if down {
            self.sub(&tiny, &mut quiet)
        } else {
            self.add(&tiny, &mut quiet)
        }
    }

    /// `next_toward(other)`: the neighbour of this number in the direction of `other`.
    pub(crate) fn next_toward(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        let answer = match self.cmp_numeric(other) {
            Ordering::Equal => return Ok(self.copy_sign(other)),
            Ordering::Less => self.next_plus(ctx)?,
            Ordering::Greater => self.next_minus(ctx)?,
        };
        if answer.is_infinite() {
            ctx.raise(Condition::Overflow);
            ctx.raise(Condition::Inexact);
            ctx.raise(Condition::Rounded);
        } else if answer.adjusted() < ctx.settings.emin {
            ctx.raise(Condition::Underflow);
            ctx.raise(Condition::Subnormal);
            ctx.raise(Condition::Inexact);
            ctx.raise(Condition::Rounded);
            if answer.is_zero() {
                ctx.raise(Condition::Clamped);
            }
        }
        Ok(answer)
    }

    /// `number_class()`: `'+Normal'`, `'-Zero'`, `'sNaN'`, ...
    pub(crate) fn number_class(&self, settings: &DecContext) -> &'static str {
        if self.is_snan() {
            "sNaN"
        } else if self.is_qnan() {
            "NaN"
        } else if self.is_infinite() {
            if self.sign() { "-Infinity" } else { "+Infinity" }
        } else if self.is_zero() {
            if self.sign() { "-Zero" } else { "+Zero" }
        } else if self.is_subnormal(settings) {
            if self.sign() { "-Subnormal" } else { "+Subnormal" }
        } else if self.sign() {
            "-Normal"
        } else {
            "+Normal"
        }
    }

    /// `logb()`: the adjusted exponent as a `Decimal`; `logb(0)` is `-Infinity`
    /// with `DivisionByZero`.
    pub(crate) fn logb(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(None, ctx) {
            return Ok(answer);
        }
        if self.is_infinite() {
            Ok(Self::infinity(false))
        } else if self.is_zero() {
            Ok(ctx.division_by_zero(true))
        } else {
            Self::from_i64(self.adjusted()).fix(ctx)
        }
    }

    /// `scaleb(other)`: adds the integer `other` to the exponent.
    pub(crate) fn scaleb(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        let limit = 2 * (ctx.settings.emax + ctx.settings.prec);
        let Some(shift) = integer_operand(other).filter(|n| (-limit..=limit).contains(n)) else {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        };
        if self.is_infinite() {
            return Ok(self.clone());
        }
        Self::from_triple(self.sign(), self.coeff().clone(), self.exponent() + shift).fix(ctx)
    }

    /// `rotate(other)`: rotates the coefficient, padded to `prec` digits, by `other` places.
    pub(crate) fn rotate(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.digit_shift(other, ctx, true)
    }

    /// `shift(other)`: shifts the coefficient, padded to `prec` digits, by `other` places.
    pub(crate) fn shift(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.digit_shift(other, ctx, false)
    }

    /// The shared body of `rotate` and `shift`.
    fn digit_shift(&self, other: &Self, ctx: &mut Ctx<'_>, rotate: bool) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        let prec = ctx.settings.prec;
        let Some(places) = integer_operand(other).filter(|n| (-prec..=prec).contains(n)) else {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        };
        if self.is_infinite() {
            return Ok(self.clone());
        }
        ctx.check_digits(u64::try_from(prec).unwrap_or(u64::MAX))?;
        let padded = pad_to_prec(&self.coeff().to_string(), prec);
        let len = padded.len();
        let places_abs = usize::try_from(places.unsigned_abs()).expect("bounded by the precision");
        let moved = if rotate {
            // Python's `s[n:] + s[:n]`, with negative `n` counted from the end.
            let split = if places >= 0 { places_abs } else { len - places_abs };
            format!("{}{}", &padded[split..], &padded[..split])
        } else if places < 0 {
            padded[..len - places_abs].to_owned()
        } else {
            let mut shifted = padded;
            shifted.extend(iter::repeat_n('0', places_abs));
            shifted[shifted.len() - len..].to_owned()
        };
        Ok(Self::from_triple(
            self.sign(),
            Coeff::from_digits(&moved),
            self.exponent(),
        ))
    }

    /// `logical_and(other)`.
    pub(crate) fn logical_and(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.logical(other, ctx, |a, b| a & b)
    }

    /// `logical_or(other)`.
    pub(crate) fn logical_or(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.logical(other, ctx, |a, b| a | b)
    }

    /// `logical_xor(other)`.
    pub(crate) fn logical_xor(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.logical(other, ctx, |a, b| a ^ b)
    }

    /// `logical_invert()`: `logical_xor` with `prec` ones.
    pub(crate) fn logical_invert(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if !self.is_logical() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        ctx.check_digits(u64::try_from(ctx.settings.prec).unwrap_or(u64::MAX))?;
        let ones = Self::from_triple(false, Coeff::repeat_digit(1, ctx.settings.prec.unsigned_abs()), 0);
        self.logical_xor(&ones, ctx)
    }

    /// The digit-wise logical operations; both operands must be logical (`_islogical`).
    fn logical(&self, other: &Self, ctx: &mut Ctx<'_>, op: fn(u8, u8) -> u8) -> RunResult<Self> {
        if !self.is_logical() || !other.is_logical() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        let prec = ctx.settings.prec;
        ctx.check_digits(u64::try_from(prec).unwrap_or(u64::MAX))?;
        let a = pad_to_prec(&self.coeff().to_string(), prec);
        let b = pad_to_prec(&other.coeff().to_string(), prec);
        let result: String = a
            .bytes()
            .zip(b.bytes())
            .map(|(x, y)| char::from(b'0' + op(x - b'0', y - b'0')))
            .collect();
        Ok(Self::from_triple(false, Coeff::from_digits(&result), 0))
    }

    /// `_islogical`: finite, positive, exponent zero and only the digits 0 and 1.
    fn is_logical(&self) -> bool {
        self.is_finite()
            && !self.sign()
            && self.exponent() == 0
            && self.coeff().to_string().bytes().all(|b| b == b'0' || b == b'1')
    }

    /// `as_integer_ratio()` in lowest terms, with C `_decimal`'s errors for NaNs and
    /// infinities.
    pub(crate) fn as_integer_ratio(&self, ctx: &Ctx<'_>) -> RunResult<(BigInt, BigInt)> {
        if self.is_nan() {
            return Err(ExcType::value_error("cannot convert NaN to integer ratio"));
        }
        if self.is_infinite() {
            return Err(ExcType::overflow_error("cannot convert Infinity to integer ratio"));
        }
        if self.is_zero() {
            return Ok((BigInt::zero(), BigInt::from(1u8)));
        }
        let mut n = self.int();
        let exp = self.exponent();
        let d = if exp >= 0 {
            n *= super::checked_pow10(exp.unsigned_abs(), ctx)?;
            BigInt::from(1u8)
        } else {
            let five = BigInt::from(5u8);
            let mut d5 = exp.unsigned_abs();
            while d5 > 0 && n.is_multiple_of(&five) {
                n /= &five;
                d5 -= 1;
            }
            let mut d2 = exp.unsigned_abs();
            let shift2 = n.trailing_zeros().unwrap_or(0).min(d2);
            n >>= shift2;
            d2 -= shift2;
            // `5**d5` has about `0.7 * d5` decimal digits.
            ctx.check_digits(d5.saturating_add(d2 / 3))?;
            Pow::pow(five, d5) << d2
        };
        Ok((if self.sign() { -n } else { n }, d))
    }

    /// `float(d)`: the correctly rounded double; a signaling NaN raises `ValueError`.
    pub(crate) fn to_f64(&self) -> RunResult<f64> {
        if self.is_snan() {
            Err(ExcType::value_error("cannot convert signaling NaN to float"))
        } else if self.is_nan() {
            Ok(if self.sign() { -f64::NAN } else { f64::NAN })
        } else if self.is_infinite() {
            Ok(if self.sign() { f64::NEG_INFINITY } else { f64::INFINITY })
        } else {
            Ok(self
                .to_sci_string(false, true)
                .parse::<f64>()
                .expect("scientific strings parse as floats"))
        }
    }
}

/// The value of an integral operand with exponent zero, as `rotate`, `shift` and
/// `scaleb` take; `None` for anything else or a value outside `i64`.
fn integer_operand(value: &Decimal) -> Option<i64> {
    if !value.is_finite() || value.exponent() != 0 {
        return None;
    }
    let magnitude = value.coeff().to_u64()?;
    if value.sign() {
        0i64.checked_sub_unsigned(magnitude)
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// Left-pads with zeros, or keeps the last `prec` digits (`_fill_logical`).
fn pad_to_prec(digits: &str, prec: i64) -> String {
    let prec = usize::try_from(prec).unwrap_or(0);
    if digits.len() >= prec {
        digits[digits.len() - prec..].to_owned()
    } else {
        let mut padded = "0".repeat(prec - digits.len());
        padded.push_str(digits);
        padded
    }
}
