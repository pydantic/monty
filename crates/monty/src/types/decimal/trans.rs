//! Correctly rounded `sqrt`, `exp`, `ln` and `log10`, plus the fixed-point integer
//! kernels they and `__pow__` share (`_pydecimal`'s `_ilog`, `_dlog`, `_dexp`, ...).
//!
//! The kernels work on integers standing for fixed-point reals (`x` represents
//! `x / M`), so every intermediate is about the working precision in size; callers
//! check that size against the memory limit first, and every loop whose trip count
//! grows with the precision polls the time limit.
//!
//! The four functions always round half-even, whatever the context's rounding,
//! as C `_decimal` does: the result is correctly rounded either way.

#![expect(
    clippy::many_single_char_names,
    reason = "the kernels keep _pydecimal's variable names so they can be read against it"
)]

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::{
    Coeff, Ctx, Decimal, checked_pow10,
    context::{Condition, Rounding},
};
use crate::exception_private::RunResult;

impl Decimal {
    /// `sqrt()`: Newton's method on an integer scaled to one digit beyond the precision.
    pub(crate) fn sqrt(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if self.is_special() {
            if let Some(answer) = self.check_nans(None, ctx) {
                return Ok(answer);
            }
            if self.is_infinite() && !self.sign() {
                return Ok(self.clone());
            }
        }
        if self.is_zero() {
            // The ideal exponent is half the operand's; `sqrt(-0)` is `-0`.
            return Self::zero(self.sign(), self.exponent().div_euclid(2)).fix(ctx);
        }
        if self.sign() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }

        // Write the operand as `c * 100**e` with `c` holding exactly `prec` base-100
        // digits; `e` starts as the ideal exponent of an exact root.
        let prec = ctx.settings.prec + 1;
        ctx.check_digits(u64::try_from(prec).unwrap_or(u64::MAX).saturating_mul(2))?;
        let op_exp = self.exponent();
        let mut e = op_exp.div_euclid(2);
        let (mut c, l) = if op_exp.rem_euclid(2) == 1 {
            (self.int() * 10u8, (self.digits() >> 1) + 1)
        } else {
            (self.int(), (self.digits() + 1) >> 1)
        };
        let shift = prec - l;
        let mut exact = if shift >= 0 {
            c *= ten(2 * shift, ctx)?;
            true
        } else {
            let (q, r) = c.div_mod_floor(&ten(-2 * shift, ctx)?);
            c = q;
            r.is_zero()
        };
        e -= shift;

        // n = floor(sqrt(c)).
        let mut n = ten(prec, ctx)?;
        loop {
            ctx.check_time()?;
            let q = &c / &n;
            if n <= q {
                break;
            }
            n = (n + q) >> 1u8;
        }
        exact = exact && &n * &n == c;
        if exact {
            // Rescale to the ideal exponent.
            if shift >= 0 {
                n /= ten(shift, ctx)?;
            } else {
                n *= ten(-shift, ctx)?;
            }
            e += shift;
        } else if (&n % 5u8).is_zero() {
            // An inexact root ending in 0 or 5 would round as if exact: nudge it.
            n += 1u8;
        }
        round_half_even(&Self::from_bigint_triple(false, &n, e), ctx)
    }

    /// `exp()`.
    pub(crate) fn exp(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(None, ctx) {
            return Ok(answer);
        }
        if self.infinity_sign() == -1 {
            return Ok(Self::from_i64(0));
        }
        if self.is_zero() {
            return Ok(Self::from_i64(1));
        }
        if self.infinity_sign() == 1 {
            return Ok(self.clone());
        }

        // The result is transcendental, so `fix` always raises Inexact and Rounded.
        let p = ctx.settings.prec;
        ctx.check_digits(u64::try_from(p).unwrap_or(u64::MAX).saturating_add(8))?;
        let adj = self.adjusted();
        let settings = ctx.settings;
        let answer = if !self.sign() && adj > dec_len_i64((settings.emax + 1) * 3) {
            // Overflow.
            Self::from_triple(false, Coeff::one(), settings.emax + 1)
        } else if self.sign() && adj > dec_len_i64((-settings.etiny() + 1) * 3) {
            // Underflow to zero.
            Self::from_triple(false, Coeff::one(), settings.etiny() - 1)
        } else if !self.sign() && adj < -p {
            // p+1 digits just above one; the final round raises the right flags.
            let mut coeff = Coeff::pow10(p.unsigned_abs());
            coeff.increment();
            Self::from_triple(false, coeff, -p)
        } else if self.sign() && adj < -p - 1 {
            // p+1 nines, just below one.
            Self::from_triple(false, Coeff::repeat_digit(9, (p + 1).unsigned_abs()), -p - 1)
        } else {
            let c = if self.sign() { -self.int() } else { self.int() };
            let e = self.exponent();
            // Raise the working precision three digits at a time until the result
            // is unambiguously roundable.
            let mut extra = 3;
            loop {
                ctx.check_time()?;
                let (coeff, exp) = dexp(&c, e, p + extra, ctx)?;
                if roundable(&coeff, p, ctx)? {
                    break Self::from_bigint_triple(false, &coeff, exp);
                }
                extra += 3;
            }
        };
        round_half_even(&answer, ctx)
    }

    /// `ln()`.
    pub(crate) fn ln(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(None, ctx) {
            return Ok(answer);
        }
        if self.is_zero() {
            return Ok(Self::infinity(true));
        }
        if self.infinity_sign() == 1 {
            return Ok(Self::infinity(false));
        }
        if self.is_numerically_one() {
            return Ok(Self::from_i64(0));
        }
        if self.sign() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }

        let (c, e) = (self.int(), self.exponent());
        let p = ctx.settings.prec;
        ctx.check_digits(u64::try_from(p).unwrap_or(u64::MAX).saturating_add(8))?;
        let mut places = p - self.ln_exp_bound(ctx)? + 2;
        let coeff = loop {
            ctx.check_time()?;
            let coeff = dlog(&c, e, places, ctx)?;
            if roundable(&coeff, p, ctx)? {
                break coeff;
            }
            places += 3;
        };
        let answer = Self::from_bigint_triple(coeff.is_negative(), &coeff.abs(), -places);
        round_half_even(&answer, ctx)
    }

    /// `log10()`.
    pub(crate) fn log10(&self, ctx: &mut Ctx<'_>) -> RunResult<Self> {
        if let Some(answer) = self.check_nans(None, ctx) {
            return Ok(answer);
        }
        if self.is_zero() {
            return Ok(Self::infinity(true));
        }
        if self.infinity_sign() == 1 {
            return Ok(Self::infinity(false));
        }
        if self.sign() {
            return Ok(ctx.invalid(Condition::InvalidOperation));
        }

        let answer = if self.coeff().is_power_of_ten() {
            // log10(10**n) = n exactly (though it may still need rounding).
            Self::from_i64(self.adjusted())
        } else {
            let (c, e) = (self.int(), self.exponent());
            let p = ctx.settings.prec;
            ctx.check_digits(u64::try_from(p).unwrap_or(u64::MAX).saturating_add(8))?;
            let mut places = p - self.log10_exp_bound(ctx)? + 2;
            let coeff = loop {
                ctx.check_time()?;
                let coeff = dlog10(&c, e, places, ctx)?;
                if roundable(&coeff, p, ctx)? {
                    break coeff;
                }
                places += 3;
            };
            Self::from_bigint_triple(coeff.is_negative(), &coeff.abs(), -places)
        };
        round_half_even(&answer, ctx)
    }

    /// Whether this is a finite value numerically equal to one (`self == _One`).
    pub(super) fn is_numerically_one(&self) -> bool {
        self.is_finite() && !self.sign() && self.cmp_numeric(&Self::from_i64(1)).is_eq()
    }

    /// `_ln_exp_bound`: `r` such that `ln(self) >= 10**r`, for a finite positive
    /// operand other than one.
    fn ln_exp_bound(&self, ctx: &Ctx<'_>) -> RunResult<i64> {
        let adj = self.adjusted();
        if adj >= 1 {
            // 23/10 bounds ln(10) from below.
            return Ok(dec_len_i128(i128::from(adj) * 23 / 10) - 1);
        }
        if adj <= -2 {
            return Ok(dec_len_i128((-1 - i128::from(adj)) * 23 / 10) - 1);
        }
        let (c, e) = (self.int(), self.exponent());
        Ok(if adj == 0 {
            // 1 < self < 10.
            let num = (&c - ten(-e, ctx)?).to_string();
            let den = c.to_string();
            super::len_i64(&num) - super::len_i64(&den) - i64::from(num < den)
        } else {
            // adj == -1: 0.1 <= self < 1.
            e + dec_len(&(ten(-e, ctx)? - &c)) - 1
        })
    }

    /// `_log10_exp_bound`: `r` such that `log10(self) >= 10**r`, for a finite positive
    /// operand other than one.
    pub(super) fn log10_exp_bound(&self, ctx: &Ctx<'_>) -> RunResult<i64> {
        let adj = self.adjusted();
        if adj >= 1 {
            return Ok(dec_len_i64(adj) - 1);
        }
        if adj <= -2 {
            return Ok(dec_len_i64(-1 - adj) - 1);
        }
        let (c, e) = (self.int(), self.exponent());
        Ok(if adj == 0 {
            // 1 < self < 10; |log10(x)| > (1-1/x)/2.31.
            let num = (&c - ten(-e, ctx)?).to_string();
            let den = (c * 231u8).to_string();
            super::len_i64(&num) - super::len_i64(&den) - i64::from(num < den) + 2
        } else {
            // adj == -1: 0.1 <= self < 1.
            let num = (ten(-e, ctx)? - c).to_string();
            super::len_i64(&num) + e - i64::from(num.as_str() < "231") - 1
        })
    }
}

/// `_fix` under `ROUND_HALF_EVEN`, keeping the conditions on `ctx`.
fn round_half_even(value: &Decimal, ctx: &mut Ctx<'_>) -> RunResult<Decimal> {
    let rounding = ctx.settings.rounding;
    ctx.settings.rounding = Rounding::HalfEven;
    let answer = value.fix(ctx);
    ctx.settings.rounding = rounding;
    answer
}

/// Whether an approximation with an error below one unit in its last place rounds
/// the same however small the error: its digits below the precision are not
/// `5000...` or `0000...` (`coeff % (5*10**(len(coeff)-p-1)) != 0`).
pub(super) fn roundable(coeff: &BigInt, p: i64, ctx: &Ctx<'_>) -> RunResult<bool> {
    let below = dec_len(coeff) - p - 1;
    if below < 0 {
        return Ok(true);
    }
    Ok(!(coeff % (ten(below, ctx)? * 5u8)).is_zero())
}

/// `10**n` for `n >= 0`, checked against the memory limit.
pub(super) fn ten(n: i64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    checked_pow10(u64::try_from(n).expect("power of ten exponent is non-negative"), ctx)
}

/// `len(str(abs(n)))`.
pub(super) fn dec_len(n: &BigInt) -> i64 {
    match n.to_u64().or_else(|| (-n).to_u64()) {
        Some(small) => dec_len_u64(small),
        None => super::len_i64(&n.magnitude().to_string()),
    }
}

/// `len(str(abs(n)))` for a machine integer.
pub(super) fn dec_len_i64(n: i64) -> i64 {
    dec_len_u64(n.unsigned_abs())
}

/// `len(str(abs(n)))` for an intermediate wider than `i64`.
fn dec_len_i128(n: i128) -> i64 {
    i64::from(n.unsigned_abs().checked_ilog10().unwrap_or(0)) + 1
}

/// The number of decimal digits of `n` (one for zero).
fn dec_len_u64(n: u64) -> i64 {
    i64::from(n.checked_ilog10().unwrap_or(0)) + 1
}

/// `len(str(n))`, counting a minus sign as `_pydecimal` does in `_dexp`.
fn str_len(n: &BigInt) -> i64 {
    dec_len(n) + i64::from(n.is_negative())
}

/// `_div_nearest(a, b)`: the integer nearest `a / b` for `b > 0`, ties to even.
pub(super) fn div_nearest(a: &BigInt, b: &BigInt) -> BigInt {
    let (q, r) = a.div_mod_floor(b);
    let twice: BigInt = r * 2u8 + u8::from(q.is_odd());
    if &twice > b { q + 1u8 } else { q }
}

/// `_rshift_nearest(x, shift)`: the integer nearest `x / 2**shift`, ties to even.
fn rshift_nearest(x: &BigInt, shift: u64) -> BigInt {
    let b = BigInt::one() << shift;
    div_nearest(x, &b)
}

/// `_sqrt_nearest(n, a)`: the integer nearest `sqrt(n)` for positive `n`, by Newton's
/// method from the positive estimate `a`.
fn sqrt_nearest(n: &BigInt, mut a: BigInt, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    let mut b = BigInt::zero();
    while a != b {
        ctx.check_time()?;
        // `a - (-n // a) >> 1`, i.e. `(a + ceil(n / a)) >> 1`.
        let next = (&a - (-n).div_floor(&a)) >> 1u8;
        b = a;
        a = next;
    }
    Ok(a)
}

/// `_ilog(x, M)`: an integer approximation to `M * log(x / M)`, with error at most
/// 22 for `0.1 <= x/M <= 10`. Reduces with `log1p(y) = 2*log1p(y/(1+sqrt(1+y)))`
/// until the argument is below `2**-8`, then sums the Taylor series.
fn ilog(x: &BigInt, m: &BigInt, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    const L: u64 = 8;
    let mut y = x - m;
    let mut r: u64 = 0;
    loop {
        ctx.check_time()?;
        let big = if r <= L {
            (y.abs() << (L - r)) >= *m
        } else {
            (y.abs() >> (r - L)) >= *m
        };
        if !big {
            break;
        }
        let root = sqrt_nearest(&(m * (m + rshift_nearest(&y, r))), m.clone(), ctx)?;
        y = div_nearest(&((m * &y) << 1u8), &(m + root));
        r += 1;
    }
    // Taylor series with T terms.
    let t = series_terms(m);
    let yshift = rshift_nearest(&y, r);
    let mut w = div_nearest(m, &BigInt::from(t));
    for k in (1..t).rev() {
        ctx.check_time()?;
        w = div_nearest(m, &BigInt::from(k)) - div_nearest(&(&yshift * &w), m);
    }
    Ok(div_nearest(&(w * y), m))
}

/// The number of Taylor terms `_ilog` and `_iexp` sum: `ceil(10 * len(str(M)) / 24)`.
fn series_terms(m: &BigInt) -> i64 {
    (10 * dec_len(m) + 23) / 24
}

/// `_dlog10(c, e, p)`: an integer approximation to `10**p * log10(c * 10**e)` with
/// error at most one, for `c > 0` and `c * 10**e != 1`.
fn dlog10(c: &BigInt, e: i64, p: i64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    // Two guard digits, divided away at the end.
    let p = p + 2;
    // Write c*10**e as d*10**f with 1 <= d <= 10 (f >= 0) or 0.1 <= d <= 1 (f <= 0).
    let l = dec_len(c);
    let f = e + l - i64::from(e + l >= 1);
    let (log_d, log_tenpower) = if p > 0 {
        let m = ten(p, ctx)?;
        let k = e + p - f;
        let c = if k >= 0 {
            c * ten(k, ctx)?
        } else {
            div_nearest(c, &ten(-k, ctx)?)
        };
        let log_d = ilog(&c, &m, ctx)?;
        let log_10 = log10_digits(p, ctx)?;
        (div_nearest(&(log_d * &m), &log_10), BigInt::from(f) * m)
    } else {
        (BigInt::zero(), div_nearest(&BigInt::from(f), &ten(-p, ctx)?))
    };
    Ok(div_nearest(&(log_tenpower + log_d), &BigInt::from(100u8)))
}

/// `_dlog(c, e, p)`: an integer approximation to `10**p * ln(c * 10**e)` with error
/// at most one, for `c > 0` and `c * 10**e != 1`.
pub(super) fn dlog(c: &BigInt, e: i64, p: i64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    let p = p + 2;
    let l = dec_len(c);
    let f = e + l - i64::from(e + l >= 1);
    let log_d = if p > 0 {
        let k = e + p - f;
        let c = if k >= 0 {
            c * ten(k, ctx)?
        } else {
            div_nearest(c, &ten(-k, ctx)?)
        };
        ilog(&c, &ten(p, ctx)?, ctx)?
    } else {
        BigInt::zero()
    };
    let f_log_ten = if f == 0 {
        BigInt::zero()
    } else {
        let extra = dec_len_i64(f) - 1;
        if p + extra >= 0 {
            div_nearest(&(BigInt::from(f) * log10_digits(p + extra, ctx)?), &ten(extra, ctx)?)
        } else {
            BigInt::zero()
        }
    };
    Ok(div_nearest(&(f_log_ten + log_d), &BigInt::from(100u8)))
}

/// The first 1000 digits of `ln(10)`, truncated (not rounded).
///
/// Far more than `_pydecimal`'s 47 built-in digits, because fractional powers work at
/// the precision plus 23 digits: [`log10_digits`] only recomputes beyond 1000 digits.
const LOG10_DIGITS: &str = concat!(
    "2302585092994045684017991454684364207601101488628772976033327900967572609677352480235997205089598298",
    "3419677840422862486334095254650828067566662873690987816894829072083255546808437998948262331985283935",
    "0530896537773262884616336622228769821988674654366747440424327436515504893431493939147961940440022210",
    "5101714174800368808401264708068556774321622835522011480466371565912137345074785694768346361679210180",
    "6445070648000277502684916746550586856935673420670581136429224554405758925724208241314695689016758940",
    "2567763113569192920333765871416602301057030896345720754403708474699401682692828084811842893148485249",
    "4864487192780967627127577539702766860595249671667418348570442250719796500471495105049221477656763693",
    "8662976979522110718264549734772662425709429322582798502585509785265383207606726317164309505995087807",
    "5237103331011978575473315414218084275438635917781170543098274823850456480190956102992918243182375253",
    "5770975053956518769751037497088869218020518933950723853920514463419726528728696511086257149219884997",
);

/// `_log10_digits(p)`: `floor(10**p * ln(10))` for `p >= 0`.
///
/// `_pydecimal` memoizes the digits module-wide; here they are recomputed when more
/// than the built-in digits are needed, which only happens near 1000 digits of precision.
fn log10_digits(p: i64, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    let take = usize::try_from(p + 1).expect("p is non-negative");
    if take <= LOG10_DIGITS.len() {
        return Ok(digits_to_int(&LOG10_DIGITS[..take]));
    }
    // Compute p+3, p+6, ... digits until one of the extra digits is non-zero.
    let mut extra = 3;
    let digits = loop {
        ctx.check_time()?;
        let m = ten(p + extra + 2, ctx)?;
        let digits = div_nearest(&ilog(&(&m * 10u8), &m, ctx)?, &BigInt::from(100u8)).to_string();
        let tail = usize::try_from(extra).expect("extra is small");
        if !digits[digits.len() - tail..].bytes().all(|b| b == b'0') {
            break digits;
        }
        extra += 3;
    };
    // Keep the reliable digits: drop trailing zeros and the next (possibly off) digit.
    let trimmed = digits.trim_end_matches('0');
    let reliable = &trimmed[..trimmed.len() - 1];
    Ok(digits_to_int(&reliable[..take]))
}

/// Parses a string of decimal digits into an integer.
fn digits_to_int(digits: &str) -> BigInt {
    Coeff::from_digits(digits).to_bigint()
}

/// `_iexp(x, M)`: an integer approximation to `M * exp(x / M)` for small
/// non-negative `x / M`. Halves the argument `R` times, sums the `expm1` series and
/// doubles back with `expm1(2x) = expm1(x) * (expm1(x) + 2)`.
fn iexp(x: &BigInt, m: &BigInt, ctx: &Ctx<'_>) -> RunResult<BigInt> {
    const L: u8 = 8;
    let r = ((x << L).div_floor(m)).bits();
    let t = series_terms(m);
    let mut y = div_nearest(x, &BigInt::from(t));
    let mshift: BigInt = m << r;
    for i in (1..t).rev() {
        ctx.check_time()?;
        y = div_nearest(&(x * (&mshift + &y)), &(&mshift * i));
    }
    for k in (0..r).rev() {
        ctx.check_time()?;
        let mshift: BigInt = m << (k + 2);
        y = div_nearest(&(&y * (&y + &mshift)), &mshift);
    }
    Ok(m + y)
}

/// `_dexp(c, e, p)`: integers `(d, f)` with `10**(p-1) <= d <= 10**p` and `d * 10**f`
/// within one unit of `exp(c * 10**e)`.
pub(super) fn dexp(c: &BigInt, e: i64, p: i64, ctx: &Ctx<'_>) -> RunResult<(BigInt, i64)> {
    // M = 10**(p+2) gives p+3 digits of precision.
    let p = p + 2;
    // ln(10) with extra precision matching the adjusted exponent of c*10**e.
    let extra = (e + str_len(c) - 1).max(0);
    let q = p + extra;
    // c*10**e / ln(10), rounding down.
    let shift = e + q;
    let cshift = if shift >= 0 {
        c * ten(shift, ctx)?
    } else {
        c.div_floor(&ten(-shift, ctx)?)
    };
    let (quot, rem) = cshift.div_mod_floor(&log10_digits(q, ctx)?);
    let rem = div_nearest(&rem, &ten(extra, ctx)?);
    let coeff = div_nearest(&iexp(&rem, &ten(p, ctx)?, ctx)?, &BigInt::from(1000u16));
    let quot = quot
        .to_i64()
        .expect("the quotient is bounded by the caller's exponent checks");
    Ok((coeff, quot - p + 3))
}

/// `_dpower(xc, xe, yc, ye, p)`: integers `(c, e)` with `10**(p-1) <= c <= 10**p` and
/// `c * 10**e` within one unit of `x**y`, for positive `x != 1` and non-zero `y`.
pub(super) fn dpower(xc: &BigInt, xe: i64, yc: &BigInt, ye: i64, p: i64, ctx: &Ctx<'_>) -> RunResult<(BigInt, i64)> {
    // 10**(b-1) <= |y| <= 10**b.
    let b = dec_len(yc) + ye;
    // ln(x) to p+b+1 places.
    let lxc = dlog(xc, xe, p + b + 1, ctx)?;
    // y*ln(x) = pc * 10**(-p-1).
    let shift = ye - b;
    let pc = if shift >= 0 {
        lxc * yc * ten(shift, ctx)?
    } else {
        div_nearest(&(lxc * yc), &ten(-shift, ctx)?)
    };
    if pc.is_zero() {
        // Prefer a result that is not exactly one, so the caller rounds it correctly.
        Ok(if (dec_len(xc) + xe >= 1) == yc.is_positive() {
            (ten(p - 1, ctx)? + 1u8, 1 - p)
        } else {
            (ten(p, ctx)? - 1u8, -p)
        })
    } else {
        let (coeff, exp) = dexp(&pc, -(p + 1), p + 1, ctx)?;
        Ok((div_nearest(&coeff, &BigInt::from(10u8)), exp + 1))
    }
}

/// `_log10_lb(c)`: a lower bound for `100 * log10(c)`, for positive `c`.
pub(super) fn log10_lb(c: &BigInt) -> i64 {
    let digits = c.to_string();
    let correction = match digits.as_bytes()[0] {
        b'1' => 100,
        b'2' => 70,
        b'3' => 53,
        b'4' => 40,
        b'5' => 31,
        b'6' => 23,
        b'7' => 16,
        b'8' => 10,
        _ => 5,
    };
    100 * super::len_i64(&digits) - correction
}

/// `_decimal_lshift_exact(n, e)`: `n * 10**e` if that is an integer, else `None`.
pub(super) fn decimal_lshift_exact(n: &BigInt, e: i64, ctx: &Ctx<'_>) -> RunResult<Option<BigInt>> {
    if n.is_zero() {
        Ok(Some(BigInt::zero()))
    } else if e >= 0 {
        Ok(Some(n * ten(e, ctx)?))
    } else {
        // The number of trailing zeros of n, the largest power of ten dividing it.
        let digits = n.magnitude().to_string();
        let zeros = super::len_i64(&digits) - super::len_i64(digits.trim_end_matches('0'));
        if zeros < -e {
            Ok(None)
        } else {
            Ok(Some(n / ten(-e, ctx)?))
        }
    }
}
