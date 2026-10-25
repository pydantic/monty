//! Comparisons: `_cmp`, `compare`, `compare_signal`, `compare_total(_mag)` and the
//! `max`/`min` family.

use std::{borrow::Cow, cmp::Ordering};

use super::{Coeff, Ctx, Decimal};
use crate::exception_private::RunResult;

/// `_cmp`: numeric order of two non-NaN values, `-0 == 0`.
pub(super) fn cmp_numeric(a: &Decimal, b: &Decimal) -> Ordering {
    if a.is_special() || b.is_special() {
        return a.infinity_sign().cmp(&b.infinity_sign());
    }
    // A negative operand orders below; `sign_order` flips magnitude orders for negatives.
    let sign_order = |sign: bool, ord: Ordering| if sign { ord.reverse() } else { ord };
    match (a.is_zero(), b.is_zero()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return sign_order(b.sign(), Ordering::Less),
        (false, true) => return sign_order(a.sign(), Ordering::Greater),
        (false, false) => {}
    }
    if a.sign() != b.sign() {
        return if a.sign() { Ordering::Less } else { Ordering::Greater };
    }
    let magnitude = match a.adjusted().cmp(&b.adjusted()) {
        // Equal adjusted exponents: compare the coefficients aligned to a common
        // exponent. The shift is bounded by the coefficient lengths.
        Ordering::Equal => {
            let (a_digits, b_digits) = (a.coeff().digits(), b.coeff().digits());
            let width = a_digits.max(b_digits);
            aligned(a.coeff(), width - a_digits).cmp(&aligned(b.coeff(), width - b_digits))
        }
        other => other,
    };
    sign_order(a.sign(), magnitude)
}

impl Decimal {
    /// `compare(other)`: `Decimal(-1)`, `0` or `1`, or a NaN for a NaN operand.
    pub(crate) fn compare(&self, other: &Self, ctx: &mut Ctx<'_>) -> Self {
        if (self.is_special() || other.is_special())
            && let Some(answer) = self.check_nans(Some(other), ctx)
        {
            return answer;
        }
        ordering_value(self.cmp_numeric(other))
    }

    /// `compare_signal(other)`: `compare`, but any NaN raises `InvalidOperation`.
    pub(crate) fn compare_signal(&self, other: &Self, ctx: &mut Ctx<'_>) -> Self {
        match self.compare_check_nans(other, ctx) {
            Some(answer) => answer,
            None => self.compare(other, ctx),
        }
    }

    /// `compare_total(other)`: the total order on representations, which also orders
    /// NaNs and distinguishes `1.0` from `1.00`. Never signals.
    pub(crate) fn compare_total(&self, other: &Self) -> Self {
        ordering_value(self.total_order(other))
    }

    /// `compare_total_mag(other)`: `compare_total` with both signs ignored.
    pub(crate) fn compare_total_mag(&self, other: &Self) -> Self {
        self.copy_abs().compare_total(&other.copy_abs())
    }

    /// The ordering behind [`Self::compare_total`].
    pub(crate) fn total_order(&self, other: &Self) -> Ordering {
        if self.sign() && !other.sign() {
            return Ordering::Less;
        }
        if !self.sign() && other.sign() {
            return Ordering::Greater;
        }
        // Both operands share this sign, which reverses every magnitude order.
        let signed = |ord: Ordering| if self.sign() { ord.reverse() } else { ord };
        let (self_nan, other_nan) = (self.nan_kind(), other.nan_kind());
        if self_nan != 0 || other_nan != 0 {
            return if self_nan == other_nan {
                // Payloads compare as integers.
                signed(self.coeff().cmp(other.coeff()))
            } else {
                // A quiet NaN orders above a signaling one, both above every number.
                let rank = |kind: u8| match kind {
                    1 => 2,
                    2 => 1,
                    _ => 0,
                };
                signed(rank(self_nan).cmp(&rank(other_nan)))
            };
        }
        match self.cmp_numeric(other) {
            Ordering::Equal => signed(self.exponent().cmp(&other.exponent())),
            ord => ord,
        }
    }

    /// `max(other)`: the larger operand, a quiet NaN losing to a number. Rounds.
    pub(crate) fn max(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.pick(other, ctx, false, Ordering::Less)
    }

    /// `min(other)`.
    pub(crate) fn min(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.pick(other, ctx, false, Ordering::Greater)
    }

    /// `max_mag(other)`: `max` on the absolute values.
    pub(crate) fn max_mag(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.pick(other, ctx, true, Ordering::Less)
    }

    /// `min_mag(other)`.
    pub(crate) fn min_mag(&self, other: &Self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        self.pick(other, ctx, true, Ordering::Greater)
    }

    /// The shared body of the `max`/`min` family: `other` wins when the comparison
    /// of `self` against it is `other_wins`; numeric ties fall back on the total order.
    fn pick(&self, other: &Self, ctx: &mut Ctx<'_>, magnitude: bool, other_wins: Ordering) -> RunResult<Self> {
        if self.is_special() || other.is_special() {
            let (sn, on) = (self.nan_kind(), other.nan_kind());
            if sn != 0 || on != 0 {
                return if on == 1 && sn == 0 {
                    self.fix(ctx)
                } else if sn == 1 && on == 0 {
                    other.fix(ctx)
                } else {
                    Ok(self.check_nans(Some(other), ctx).expect("an operand is a NaN"))
                };
            }
        }
        let mut c = if magnitude {
            self.copy_abs().cmp_numeric(&other.copy_abs())
        } else {
            self.cmp_numeric(other)
        };
        if c == Ordering::Equal {
            c = self.total_order(other);
        }
        // `_pydecimal` keeps `self` unless the comparison is exactly `-1` (or `1` for min).
        let answer = if c == other_wins { other } else { self };
        answer.fix(ctx)
    }
}

/// `Decimal(-1)`, `Decimal(0)` or `Decimal(1)`.
fn ordering_value(ord: Ordering) -> Decimal {
    Decimal::from_i64(match ord {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// `coeff * 10**shift`, borrowed when there is nothing to shift.
fn aligned(coeff: &Coeff, shift: u64) -> Cow<'_, Coeff> {
    if shift == 0 {
        Cow::Borrowed(coeff)
    } else {
        Cow::Owned(coeff.shl(shift))
    }
}
