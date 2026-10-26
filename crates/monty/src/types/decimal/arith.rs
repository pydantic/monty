//! Arithmetic: `__add__`, `__sub__`, `__mul__`, `__truediv__`, `__floordiv__`,
//! `__mod__`, `__divmod__`, `remainder_near` and `fma`.
//!
//! Each function follows the `_pydecimal` method of the same name, on [`Coeff`]
//! magnitudes ([`WorkRep`] is `_pydecimal`'s `_WorkRep`) once the specials and zeros
//! are out of the way; the result goes through [`Decimal::fix`].

use std::{borrow::Cow, cmp::Ordering};

use super::{
    Coeff, Ctx, Decimal,
    context::{Condition, Rounding},
};
use crate::exception_private::RunResult;

impl Decimal {
    /// `self + other` (`__add__`).
    pub(crate) fn add(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special() || other.is_special() {
            if let Some(answer) = self.check_nans(Some(other), ctx) {
                return Ok(answer);
            }
            if self.is_infinite() {
                // Both infinite: same sign is that infinity, opposite signs are invalid.
                return Ok(if self.sign() != other.sign() && other.is_infinite() {
                    ctx.invalid(Condition::InvalidOperation)
                } else {
                    self.clone()
                });
            }
            if other.is_infinite() {
                return Ok(other.clone());
            }
        }

        let mut exp = self.exponent().min(other.exponent());
        // Rounding towards -inf gives an exact zero sum of opposite signs a negative sign.
        let negative_zero = ctx.settings.rounding == Rounding::Floor && self.sign() != other.sign();

        if self.is_zero() && other.is_zero() {
            let sign = (self.sign() && other.sign()) || negative_zero;
            return Self::zero(sign, exp).fix(ctx);
        }
        if self.is_zero() {
            exp = exp.max(other.exponent() - ctx.settings.prec - 1);
            let answer = other.rescale(exp, ctx.settings.rounding, ctx)?;
            return answer.fix(ctx);
        }
        if other.is_zero() {
            exp = exp.max(self.exponent() - ctx.settings.prec - 1);
            let answer = self.rescale(exp, ctx.settings.rounding, ctx)?;
            return answer.fix(ctx);
        }

        let (op1, op2) = normalize(WorkRep::of(self), WorkRep::of(other), ctx)?;
        // Opposite signs subtract the smaller magnitude; the result takes the larger's sign.
        let (sign, coeff) = if op1.sign == op2.sign {
            (op1.sign, op1.coeff.add(&op2.coeff))
        } else {
            match op1.coeff.cmp(&op2.coeff) {
                // Equal and opposite.
                Ordering::Equal => return Self::zero(negative_zero, exp).fix(ctx),
                Ordering::Greater => (op1.sign, op1.coeff.sub(&op2.coeff)),
                Ordering::Less => (op2.sign, op2.coeff.sub(&op1.coeff)),
            }
        };
        Self::from_triple(sign, coeff, op1.exp).fix(ctx)
    }

    /// `self - other` (`__sub__`).
    pub(crate) fn sub(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if (self.is_special() || other.is_special())
            && let Some(answer) = self.check_nans(Some(other), ctx)
        {
            return Ok(answer);
        }
        self.add(&other.copy_negate(), ctx)
    }

    /// `self * other` (`__mul__`).
    pub(crate) fn mul(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let sign = self.sign() != other.sign();
        if self.is_special() || other.is_special() {
            if let Some(answer) = self.check_nans(Some(other), ctx) {
                return Ok(answer);
            }
            if self.is_infinite() {
                return Ok(if other.is_zero() {
                    ctx.invalid(Condition::InvalidOperation)
                } else {
                    Self::infinity(sign)
                });
            }
            if other.is_infinite() {
                return Ok(if self.is_zero() {
                    ctx.invalid(Condition::InvalidOperation)
                } else {
                    Self::infinity(sign)
                });
            }
        }

        let exp = self.exponent() + other.exponent();
        let answer = if self.is_zero() || other.is_zero() {
            Self::zero(sign, exp)
        } else {
            Self::from_triple(sign, self.coeff().mul(other.coeff()), exp)
        };
        answer.fix(ctx)
    }

    /// `self / other` (`__truediv__`, `Context.divide`).
    pub(crate) fn div(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let sign = self.sign() != other.sign();
        if self.is_special() || other.is_special() {
            if let Some(answer) = self.check_nans(Some(other), ctx) {
                return Ok(answer);
            }
            if self.is_infinite() && other.is_infinite() {
                return Ok(ctx.invalid(Condition::InvalidOperation));
            }
            if self.is_infinite() {
                return Ok(Self::infinity(sign));
            }
            if other.is_infinite() {
                ctx.raise(Condition::Clamped);
                return Ok(Self::zero(sign, ctx.settings.etiny()));
            }
        }

        if other.is_zero() {
            return Ok(if self.is_zero() {
                ctx.invalid(Condition::DivisionUndefined)
            } else {
                ctx.division_by_zero(sign)
            });
        }

        let (coeff, exp) = if self.is_zero() {
            (Coeff::zero(), self.exponent() - other.exponent())
        } else {
            // Enough quotient digits for the precision plus one guard digit.
            let shift = other.digits() - self.digits() + ctx.settings.prec + 1;
            let mut exp = self.exponent() - other.exponent() - shift;
            let (num, den) = if shift >= 0 {
                ctx.check_digits(shift.unsigned_abs() + self.coeff().digits())?;
                (
                    Cow::Owned(self.coeff().shl(shift.unsigned_abs())),
                    Cow::Borrowed(other.coeff()),
                )
            } else {
                ctx.check_digits(shift.unsigned_abs() + other.coeff().digits())?;
                (
                    Cow::Borrowed(self.coeff()),
                    Cow::Owned(other.coeff().shl(shift.unsigned_abs())),
                )
            };
            let (mut coeff, remainder) = num.div_rem(&den);
            if remainder.is_zero() {
                // Exact: strip trailing zeros back towards the ideal exponent.
                let ideal_exp = self.exponent() - other.exponent();
                let strip = u64::try_from(ideal_exp - exp).map_or(0, |gap| gap.min(coeff.trailing_zeros()));
                coeff = coeff.shr(strip).0;
                exp += i64::try_from(strip).expect("strip is at most the coefficient length");
            } else if matches!(coeff.last_digit(), 0 | 5) {
                // Inexact: nudge a trailing 0 or 5 so rounding sees the remainder.
                coeff.increment();
            }
            (coeff, exp)
        };
        Self::from_triple(sign, coeff, exp).fix(ctx)
    }

    /// `self // other` (`__floordiv__`, `Context.divide_int`).
    pub(crate) fn div_int(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        let sign = self.sign() != other.sign();
        if self.is_infinite() {
            return Ok(if other.is_infinite() {
                ctx.invalid(Condition::InvalidOperation)
            } else {
                Self::infinity(sign)
            });
        }
        if other.is_zero() {
            return Ok(if self.is_zero() {
                ctx.invalid(Condition::DivisionUndefined)
            } else {
                ctx.division_by_zero(sign)
            });
        }
        Ok(self.divide(other, ctx)?.0)
    }

    /// `self % other` (`__mod__`, `Context.remainder`): the remainder takes the
    /// dividend's sign, unlike `int`'s.
    pub(crate) fn rem(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        if self.is_infinite() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        if other.is_zero() {
            return Ok(if self.is_zero() {
                ctx.invalid(Condition::DivisionUndefined)
            } else {
                ctx.invalid(Condition::InvalidOperation)
            });
        }
        let remainder = self.divide(other, ctx)?.1;
        remainder.fix(ctx)
    }

    /// `divmod(self, other)` (`__divmod__`, `Context.divmod`).
    pub(crate) fn divmod(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<(Self, Self)> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok((answer.clone(), answer));
        }
        let sign = self.sign() != other.sign();
        if self.is_infinite() {
            return Ok(if other.is_infinite() {
                let answer = ctx.invalid(Condition::InvalidOperation);
                (answer.clone(), answer)
            } else {
                (Self::infinity(sign), ctx.invalid(Condition::InvalidOperation))
            });
        }
        if other.is_zero() {
            return Ok(if self.is_zero() {
                let answer = ctx.invalid(Condition::DivisionUndefined);
                (answer.clone(), answer)
            } else {
                (ctx.division_by_zero(sign), ctx.invalid(Condition::InvalidOperation))
            });
        }
        let (quotient, remainder) = self.divide(other, ctx)?;
        Ok((quotient, remainder.fix(ctx)?))
    }

    /// `remainder_near(other)`: the remainder of rounding the quotient to the
    /// nearest integer (ties to even), so `abs(result) <= abs(other) / 2`.
    pub(crate) fn remainder_near(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(Some(other), ctx) {
            return Ok(answer);
        }
        if self.is_infinite() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }
        if other.is_zero() {
            return Ok(if self.is_zero() {
                ctx.invalid(Condition::DivisionUndefined)
            } else {
                ctx.invalid(Condition::InvalidOperation)
            });
        }
        if other.is_infinite() {
            return self.fix(ctx);
        }

        let ideal_exponent = self.exponent().min(other.exponent());
        if self.is_zero() {
            return Self::zero(self.sign(), ideal_exponent).fix(ctx);
        }

        let expdiff = self.adjusted() - other.adjusted();
        if expdiff > ctx.settings.prec {
            // `abs(self / other) > 10**prec`.
            return Ok(ctx.invalid(Condition::DivisionImpossible));
        }
        if expdiff <= -2 {
            // `abs(self / other) < 0.1`.
            let answer = self.rescale(ideal_exponent, ctx.settings.rounding, ctx)?;
            return answer.fix(ctx);
        }

        let (op1, op2) = aligned_coeffs(self, other, ctx)?;
        let (mut q, r) = op1.div_rem(&op2);
        // Round the quotient to nearest, ties to even: past half, the remainder becomes
        // `r - op2`, negative, so the result takes the opposite of the dividend's sign.
        let mut twice = r.add(&r);
        if q.last_digit() % 2 == 1 {
            twice.increment();
        }
        let (sign, r) = if twice > *op2 {
            q.increment();
            (!self.sign(), op2.sub(&r))
        } else {
            (self.sign(), r)
        };
        if digit_count(&q) > ctx.settings.prec {
            return Ok(ctx.invalid(Condition::DivisionImpossible));
        }
        Self::from_triple(sign, r, ideal_exponent).fix(ctx)
    }

    /// `fma(other, third)`: `self * other + third` with a single rounding.
    pub(crate) fn fma(&self, other: &Self, third: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        let sign = self.sign() != other.sign();
        let product = if self.is_special() || other.is_special() {
            if self.is_snan() {
                return Ok(ctx.invalid_nan(self));
            }
            if other.is_snan() {
                return Ok(ctx.invalid_nan(other));
            }
            if self.is_qnan() {
                self.clone()
            } else if other.is_qnan() {
                other.clone()
            } else if self.is_infinite() {
                if other.is_zero() {
                    return Ok(ctx.invalid(Condition::InvalidOperation));
                }
                Self::infinity(sign)
            } else {
                if self.is_zero() {
                    return Ok(ctx.invalid(Condition::InvalidOperation));
                }
                Self::infinity(sign)
            }
        } else {
            // The exact, unrounded product.
            let exp = self.exponent() + other.exponent();
            Self::from_triple(sign, self.coeff().mul(other.coeff()), exp)
        };
        product.add(third, ctx)
    }

    /// `_divide`: the integer quotient and remainder for `//`, `%` and `divmod`.
    ///
    /// Assumes neither operand is a NaN, `self` is finite and `other` is non-zero.
    /// A quotient longer than the precision raises `DivisionImpossible`.
    fn divide(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<(Self, Self)> {
        let sign = self.sign() != other.sign();
        let ideal_exp = if other.is_infinite() {
            self.exponent()
        } else {
            self.exponent().min(other.exponent())
        };
        let expdiff = self.adjusted() - other.adjusted();
        if self.is_zero() || other.is_infinite() || expdiff <= -2 {
            let remainder = self.rescale(ideal_exp, ctx.settings.rounding, ctx)?;
            return Ok((Self::zero(sign, 0), remainder));
        }
        if expdiff <= ctx.settings.prec {
            let (op1, op2) = aligned_coeffs(self, other, ctx)?;
            let (q, r) = op1.div_rem(&op2);
            if digit_count(&q) <= ctx.settings.prec {
                return Ok((
                    Self::from_triple(sign, q, 0),
                    Self::from_triple(self.sign(), r, ideal_exp),
                ));
            }
        }
        let answer = ctx.invalid(Condition::DivisionImpossible);
        Ok((answer.clone(), answer))
    }
}

/// The coefficients of two finite values scaled to their common (smaller) exponent.
///
/// Callers have bounded the adjusted-exponent difference by the precision, so the
/// scaling factor is bounded too; it is still checked against the memory limit.
fn aligned_coeffs<'a>(a: &'a Decimal, b: &'a Decimal, ctx: &Ctx<'_>) -> RunResult<(Cow<'a, Coeff>, Cow<'a, Coeff>)> {
    let shift = a.exponent().abs_diff(b.exponent());
    Ok(if a.exponent() >= b.exponent() {
        ctx.check_digits(shift + a.coeff().digits())?;
        (Cow::Owned(a.coeff().shl(shift)), Cow::Borrowed(b.coeff()))
    } else {
        ctx.check_digits(shift + b.coeff().digits())?;
        (Cow::Borrowed(a.coeff()), Cow::Owned(b.coeff().shl(shift)))
    })
}

/// A finite operand as a sign, coefficient and exponent (`_WorkRep`).
struct WorkRep {
    sign: bool,
    coeff: Coeff,
    exp: i64,
}

impl WorkRep {
    /// The working form of a finite `Decimal`.
    fn of(decimal: &Decimal) -> Self {
        Self {
            sign: decimal.sign(),
            coeff: decimal.coeff().clone(),
            exp: decimal.exponent(),
        }
    }

    /// The number of decimal digits in the coefficient.
    fn digits(&self) -> i64 {
        digit_count(&self.coeff)
    }
}

/// The number of decimal digits of a coefficient (`len(str(n))`).
fn digit_count(n: &Coeff) -> i64 {
    i64::try_from(n.digits()).expect("digit count fits in i64")
}

/// `_normalize`: brings two non-zero operands of an addition to a common exponent.
///
/// An operand far below the other's last significant digit is replaced by a single
/// unit just below it, which rounds identically and bounds the alignment shift by
/// the precision.
fn normalize(op1: WorkRep, op2: WorkRep, ctx: &Ctx<'_>) -> RunResult<(WorkRep, WorkRep)> {
    let swapped = op1.exp < op2.exp;
    let (mut tmp, mut other) = if swapped { (op2, op1) } else { (op1, op2) };
    let exp = tmp.exp + (-1i64).min(tmp.digits() - ctx.settings.prec - 2);
    if other.digits() + other.exp - 1 < exp {
        other.coeff = Coeff::one();
        other.exp = exp;
    }
    let shift = tmp.exp.abs_diff(other.exp);
    ctx.check_digits(shift + tmp.coeff.digits())?;
    tmp.coeff = tmp.coeff.shl(shift);
    tmp.exp = other.exp;
    Ok(if swapped { (other, tmp) } else { (tmp, other) })
}
