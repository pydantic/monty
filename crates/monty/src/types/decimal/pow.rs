//! `__pow__`: the special cases, exact powers, the general `exp(y * ln(x))` path and
//! the three-argument modular form.
//!
//! Unlike `exp` and `ln`, the power respects the context's rounding mode.

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Pow, ToPrimitive, Zero};

use super::{
    Coeff, Ctx, Decimal,
    context::{Condition, DecContext, MAX_EMAX, MIN_EMIN, Rounding},
    trans::{dec_len, dec_len_i64, decimal_lshift_exact, dpower, log10_lb, roundable, ten},
};
use crate::exception_private::RunResult;

/// Exponents of exact results are clamped to this magnitude when they do not fit an
/// `i64`; `fix` then overflows or underflows them as the true value would.
const EXPONENT_CLAMP: i64 = 4_000_000_000_000_000_000;

/// libmpdec's guard digits for a non-integer power: `4 + MPD_EXPDIGITS` on 64-bit.
const LIBMPDEC_POW_GUARD_DIGITS: i64 = 23;

/// libmpdec's `MPD_EXP_INF`, the exponent it gives an overflowed power before rounding.
const LIBMPDEC_EXP_INF: i64 = 2_000_000_000_000_000_001;

/// The largest power of ten the three-argument form multiplies into its exponent.
const MAX_DIRECT_EXPONENT_DIGITS: i64 = 10_000;

impl Decimal {
    /// `self ** other`, or `pow(self, other, modulo)` when `modulo` is given.
    pub(crate) fn pow(&self, other: &Self, modulo: Option<&Self>, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(modulo) = modulo {
            return self.power_modulo(other, modulo, ctx);
        }
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }

        // 0**0 is NaN; x**0 is 1 for any other x, infinities included.
        if other.is_zero() {
            return Ok(if self.is_zero() {
                ctx.invalid(Condition::InvalidOperation)
            } else {
                Self::from_i64(1)
            });
        }

        // The result is negative iff self is negative and other an odd integer.
        let mut result_sign = false;
        let base = if self.sign() {
            if other.is_integer() {
                result_sign = !other.is_even();
            } else if !self.is_zero() {
                // A negative number to a non-integer power; (-0)**y is 0**y.
                return Ok(ctx.invalid(Condition::InvalidOperation));
            }
            self.copy_negate()
        } else {
            self.clone()
        };

        // 0**(+y or Inf) = 0; 0**(-y or -Inf) = Infinity.
        if base.is_zero() {
            return Ok(if other.sign() {
                Self::infinity(result_sign)
            } else {
                Self::zero(result_sign, 0)
            });
        }
        // Inf**(+y or Inf) = Inf; Inf**(-y or -Inf) = 0.
        if base.is_infinite() {
            return Ok(if other.sign() {
                Self::zero(result_sign, 0)
            } else {
                Self::infinity(result_sign)
            });
        }

        // 1**y = 1, with the exponent and flags depending on y.
        if base.is_numerically_one() {
            return base.power_of_one(other, result_sign, ctx);
        }

        // x**Inf is Inf for x > 1 and 0 for x < 1; the reverse for -Inf.
        let self_adj = base.adjusted();
        if other.is_infinite() {
            return Ok(if other.sign() == (self_adj >= 0) {
                Self::zero(result_sign, 0)
            } else {
                Self::infinity(result_sign)
            });
        }

        let p = ctx.settings.prec;
        ctx.check_digits(u64::try_from(p).unwrap_or(u64::MAX).saturating_add(8))?;

        // A crude test for extreme overflow or underflow: if log10(x)*y >= 10**bound,
        // the result is far beyond Emax (or below Etiny).
        let bound = i128::from(base.log10_exp_bound(ctx)?) + i128::from(other.adjusted());
        let settings = ctx.settings;
        // Underflow is possible for x > 1 with y < 0 or x < 1 with y > 0, overflow otherwise.
        let answer = if (self_adj >= 0) == other.sign() {
            let etiny = settings.etiny();
            (bound >= i128::from(dec_len_i64(-etiny))).then(|| Self::from_triple(result_sign, Coeff::one(), etiny - 1))
        } else {
            (bound >= i128::from(dec_len_i64(settings.emax)))
                .then(|| Self::from_triple(result_sign, Coeff::one(), settings.emax + 1))
        };

        if let Some(answer) = answer {
            return answer.fix(ctx);
        }
        if !other.is_integer() {
            return base.power_real(other, ctx);
        }

        // An integer power: exact when it fits in one more digit than the precision,
        // otherwise exp(y*ln(x)) with the working precision raised until the result is
        // unambiguously roundable.
        let answer = if let Some(exact) = base.power_exact(other, p + 1, ctx)? {
            exact.copy_with_sign(result_sign)
        } else {
            let (xc, xe) = (base.int(), base.exponent());
            let yc = if other.sign() { -other.int() } else { other.int() };
            let ye = other.exponent();
            let mut extra = 3;
            loop {
                ctx.check_time()?;
                let (coeff, exp) = dpower(&xc, xe, &yc, ye, p + extra, ctx)?;
                if roundable(&coeff, p, ctx)? {
                    break Self::from_bigint_triple(result_sign, &coeff, exp);
                }
                extra += 3;
            }
        };
        answer.fix(ctx)
    }

    /// A non-integer power of a positive finite value other than one, computed as
    /// C `_decimal` (libmpdec's `_mpd_qpow_real`) does, so results and flags match it.
    ///
    /// `exp(y * ln(x))` is evaluated at `max(digits(x), prec) + 23` digits, each step
    /// correctly rounded half-even, and the result rounded once more to the context.
    /// That double rounding is visible: in a directed rounding mode an exact power
    /// such as `4 ** -0.5` can come out one unit low (`0.499999999`), and a result
    /// just above a subnormal boundary can raise Underflow. `_pydecimal`, which
    /// detects exact powers, differs from CPython here; Monty follows CPython.
    fn power_real(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let prec = ctx.settings.prec;
        let wp = self.digits().max(prec).saturating_add(LIBMPDEC_POW_GUARD_DIGITS);
        ctx.check_digits(u64::try_from(wp).unwrap_or(u64::MAX).saturating_mul(2))?;
        let work = DecContext {
            prec: wp,
            rounding: Rounding::HalfEven,
            emin: MIN_EMIN,
            emax: MAX_EMAX,
            clamp: false,
            ..ctx.settings
        };
        // ln and the product signal into a scratch status libmpdec discards.
        let mut scratch = Ctx::new(work, ctx.tracker);
        let log = self.ln(&mut scratch)?;
        let product_coeff = log.int() * other.int();
        let product_exp = log.exponent().saturating_add(other.exponent());
        let product =
            Self::from_bigint_triple(log.sign() != other.sign(), &product_coeff, product_exp).fix(&mut scratch)?;
        // exp's conditions are kept, then the final rounding adds the context's.
        let mut exp_ctx = Ctx::new(work, ctx.tracker);
        let mut result = product.exp(&mut exp_ctx)?;
        ctx.status.merge(exp_ctx.status);
        if result.is_finite() && result.is_numerically_one() {
            // libmpdec widens an exact one to the full precision.
            result = Self::from_triple(false, Coeff::pow10((prec - 1).unsigned_abs()), 1 - prec);
        } else if result.is_infinite() {
            // So that rounding towards zero yields the largest finite value.
            result = Self::from_triple(false, Coeff::one(), LIBMPDEC_EXP_INF);
        }
        result.fix(ctx)
    }

    /// `1**other`, where the result's exponent and flags depend on `other` and on the
    /// exponent of the one.
    fn power_of_one(&self, other: &Self, result_sign: bool, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let prec = ctx.settings.prec;
        let exp = if other.is_integer() {
            // exp = max(self.exp * max(int(other), 0), 1 - prec), without evaluating a
            // possibly enormous int(other).
            let multiplier = if other.sign() {
                0
            } else if other.cmp_numeric(&Self::from_i64(prec)) == Ordering::Greater {
                prec
            } else {
                integral_value(other, ctx)?.to_i64().expect("bounded by the precision")
            };
            let exp = i128::from(self.exponent()) * i128::from(multiplier);
            if exp < i128::from(1 - prec) {
                ctx.raise(Condition::Rounded);
                1 - prec
            } else {
                i64::try_from(exp).expect("between 1-prec and zero")
            }
        } else {
            ctx.raise(Condition::Inexact);
            ctx.raise(Condition::Rounded);
            1 - prec
        };
        ctx.check_digits(exp.unsigned_abs())?;
        Ok(Self::from_triple(result_sign, Coeff::pow10(exp.unsigned_abs()), exp))
    }

    /// `_power_exact(other, p)`: `self**other` if it is exactly representable in `p`
    /// digits, else `None`, for positive finite `self != 1` and a non-zero integer
    /// `other` (`_pydecimal`'s n-th root branch serves non-integer powers, which take
    /// [`Self::power_real`] here).
    ///
    /// Mostly cheap tests that rule exactness out; only a survivor is computed.
    fn power_exact(&self, other: &Self, p: i64, ctx: &Ctx<'_>) -> RunResult<Option<Self>> {
        // x = xc*10**xe and |y| = yc*10**ye with xc and yc not divisible by 10.
        let (xc, xe) = strip_tens(self);
        let (yc, ye) = strip_tens(other);
        let mut xe = BigInt::from(xe);
        let mut ye = ye;

        if xc.is_one() {
            // x = 10**xe, so the result is 10**(xe*y) when xe*y is an integer.
            let mut product = &xe * &yc;
            while !product.is_zero() && (&product % 10u8).is_zero() {
                product /= 10u8;
                ye += 1;
            }
            if ye < 0 {
                return Ok(None);
            }
            let mut exponent = product * ten(ye, ctx)?;
            if other.sign() {
                exponent = -exponent;
            }
            // A non-negative integer power keeps the ideal exponent where it can.
            let zeros = if other.is_integer() && !other.sign() {
                let ideal = BigInt::from(self.exponent()) * integral_value(other, ctx)?;
                (&exponent - ideal).min(BigInt::from(p - 1))
            } else {
                BigInt::zero()
            };
            let zeros_i64 = zeros.to_i64().expect("at most the precision");
            ctx.check_digits(zeros_i64.unsigned_abs())?;
            // `'1' + '0' * zeros`, where a negative count (as in Python) adds none.
            let coeff = Coeff::pow10(u64::try_from(zeros_i64).unwrap_or(0));
            return Ok(Some(Self::from_triple(
                false,
                coeff,
                clamp_exponent(&(exponent - zeros)),
            )));
        }

        if other.sign() {
            // A negative power: xc must be a power of 2 (the result a power of 5) or
            // of 5 (the result a power of 2).
            // The last non-zero digit.
            let last_digit = self.coeff().digit_at(self.coeff().trailing_zeros());
            let (e, result_base) = match last_digit {
                2 | 4 | 6 | 8 => {
                    if xc.magnitude().count_ones() != 1 {
                        return Ok(None);
                    }
                    let e = BigInt::from(xc.bits() - 1);
                    // emax bounds e such that 5**e < 10**p.
                    let emax = i128::from(p) * 93 / 65;
                    let Some(e) = negative_power_exponent(&e, &mut xe, &yc, ye, emax, ctx)? else {
                        return Ok(None);
                    };
                    (e, 5u8)
                }
                5 => {
                    // e >= log_5(xc) when xc is a power of 5.
                    let mut e = i64::try_from(xc.bits() * 28 / 65).expect("bit count fits");
                    ctx.check_digits(e.unsigned_abs())?;
                    let (mut quotient, remainder) = Pow::pow(BigInt::from(5u8), e.unsigned_abs()).div_rem(&xc);
                    if !remainder.is_zero() {
                        return Ok(None);
                    }
                    while (&quotient % 5u8).is_zero() {
                        ctx.check_time()?;
                        quotient /= 5u8;
                        e -= 1;
                    }
                    let emax = i128::from(p) * 10 / 3;
                    let Some(e) = negative_power_exponent(&BigInt::from(e), &mut xe, &yc, ye, emax, ctx)? else {
                        return Ok(None);
                    };
                    (e, 2u8)
                }
                _ => return Ok(None),
            };
            let e_u64 = e.to_u64().expect("bounded by emax");
            ctx.check_digits(e_u64)?;
            let result = Coeff::from_bigint(&Pow::pow(BigInt::from(result_base), e_u64));
            if i128::from(result.digits()) > i128::from(p) {
                return Ok(None);
            }
            let exp = -e - xe;
            return Ok(Some(Self::from_triple(false, result, clamp_exponent(&exp))));
        }

        // A positive integer power m: stripping an integer's zeros leaves `ye >= 0`.
        debug_assert!(ye >= 0, "non-integer powers take `power_real`");
        let m = &yc * ten(ye, ctx)?;

        // If m > p*100/log10_lb(xc) then xc**m > 10**p, too many digits.
        if xc > BigInt::one() && m > BigInt::from(i128::from(p) * 100 / i128::from(log10_lb(&xc))) {
            return Ok(None);
        }
        let m_u64 = m.to_u64().expect("bounded by the precision");
        ctx.check_digits(m_u64.saturating_mul(xc.bits()) / 3)?;
        let xc = Coeff::from_bigint(&Pow::pow(xc, m_u64));
        let xe = xe * &m;
        let len = i64::try_from(xc.digits()).expect("bounded by the memory check");
        if len > p {
            return Ok(None);
        }

        // Move towards the ideal exponent for a non-negative integer power.
        let zeros = if other.is_integer() && !other.sign() {
            let ideal = BigInt::from(self.exponent()) * integral_value(other, ctx)?;
            (&xe - ideal).min(BigInt::from(p - len))
        } else {
            BigInt::zero()
        };
        let zeros_i64 = zeros.to_i64().expect("at most the precision");
        // As in Python, a negative count adds no zeros.
        let coeff = xc.shl(u64::try_from(zeros_i64).unwrap_or(0));
        Ok(Some(Self::from_triple(false, coeff, clamp_exponent(&(xe - zeros)))))
    }

    /// `_power_modulo`: `pow(self, other, modulo)`, exact, for integers only.
    fn power_modulo(&self, other: &Self, modulo: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        // NaNs behave as in `fma`: the first signaling NaN wins, then the first quiet one.
        let operands = [self, other, modulo];
        if let Some(snan) = operands.iter().find(|d| d.is_snan()) {
            return Ok(ctx.invalid_nan(snan));
        }
        if let Some(nan) = operands.iter().find(|d| d.is_qnan()) {
            return Ok(nan.fix_nan(ctx));
        }

        // Python's pow() restrictions, plus the modulus fitting in the precision.
        if !(self.is_integer() && other.is_integer() && modulo.is_integer())
            || (other.sign() && !other.is_zero())
            || modulo.is_zero()
            || modulo.adjusted() >= ctx.settings.prec
            || (other.is_zero() && self.is_zero())
        {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }

        let sign = if other.is_even() { false } else { self.sign() };
        let modulus = integral_value(modulo, ctx)?;
        let (base_int, base_exp) = integral_parts(self);
        let (exp_int, exp_exp) = integral_parts(other);
        let base = (base_int % &modulus * BigInt::from(10u8).modpow(&BigInt::from(base_exp), &modulus)) % &modulus;
        // base**(c * 10**k) = (base**c)**(10**k). A moderate `k` builds the exponent
        // (as cheap as C `_decimal`); a huge one raises to the tenth power `k` times
        // under the time limit rather than materializing `10**k`.
        let result = if exp_exp <= MAX_DIRECT_EXPONENT_DIGITS {
            base.modpow(&(exp_int * ten(exp_exp, ctx)?), &modulus)
        } else {
            let ten = BigInt::from(10u8);
            let mut acc = base.modpow(&exp_int, &modulus);
            for _ in 0..exp_exp {
                ctx.check_time()?;
                acc = acc.modpow(&ten, &modulus);
            }
            acc
        };
        Ok(Self::from_bigint_triple(sign, &result, 0))
    }

    /// The same digits and exponent with the given sign.
    fn copy_with_sign(&self, sign: bool) -> Self {
        if sign == self.sign() {
            self.clone()
        } else {
            self.copy_negate()
        }
    }
}

/// The coefficient and exponent of a finite value with trailing zeros moved into the
/// exponent (`_WorkRep` plus the `while xc % 10 == 0` loops of `_power_exact`).
fn strip_tens(value: &Decimal) -> (BigInt, i64) {
    let zeros = value.coeff().trailing_zeros();
    let exp = value.exponent() + i64::try_from(zeros).expect("bounded by the coefficient length");
    (value.coeff().shr(zeros).0.to_bigint(), exp)
}

/// An integral value as `(c, e)` with `e >= 0`: `to_integral_value()`'s `_WorkRep`.
fn integral_parts(value: &Decimal) -> (BigInt, i64) {
    if value.exponent() >= 0 {
        (value.int(), value.exponent())
    } else {
        let (c, e) = strip_tens(value);
        // `value` is integral, so stripping its zeros brings the exponent to >= 0.
        (c, e.max(0))
    }
}

/// `abs(int(value))` for an integral value whose size the caller has bounded.
fn integral_value(value: &Decimal, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    let (c, e) = integral_parts(value);
    Ok(c * ten(e, ctx)?)
}

/// The shared tail of `_power_exact`'s negative-power branches: `-e*y` and `-xe*y`
/// must both be integers no larger than `emax`. Updates `xe` to `-xe*y`.
fn negative_power_exponent(
    e: &BigInt,
    xe: &mut BigInt,
    yc: &BigInt,
    ye: i64,
    emax: i128,
    ctx: &Ctx<'_>,
) -> RunResult<Option<BigInt>> {
    // 10**ye > emax makes 5**(-e*y) (or 2**...) longer than the precision.
    if i128::from(ye) >= i128::from(dec_len(&BigInt::from(emax))) {
        return Ok(None);
    }
    let Some(e) = decimal_lshift_exact(&(e * yc), ye, ctx)? else {
        return Ok(None);
    };
    let Some(new_xe) = decimal_lshift_exact(&(&*xe * yc), ye, ctx)? else {
        return Ok(None);
    };
    if e > BigInt::from(emax) {
        return Ok(None);
    }
    *xe = new_xe;
    Ok(Some(e))
}

/// Converts an exact result's exponent to `i64`, clamping one too large to matter.
fn clamp_exponent(exp: &BigInt) -> i64 {
    exp.to_i64()
        .unwrap_or(if exp.sign() == num_bigint::Sign::Minus {
            -EXPONENT_CLAMP
        } else {
            EXPONENT_CLAMP
        })
        .clamp(-EXPONENT_CLAMP, EXPONENT_CLAMP)
}
