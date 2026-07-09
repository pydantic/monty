//! Transcendental `Decimal` operations — `sqrt`, `exp`, `ln`, `log10` — and
//! the fixed-point integer kernels behind them (and behind `__pow__`), ported
//! line-for-line from `_pydecimal.py` (CPython 3.14: `sqrt` 2681-2778, `exp`
//! 3001-3074, `ln` 3132-3205, `log10` 3207-3286, kernels 5651-5992).
//!
//! The kernels represent a real number `z` as an integer approximation to
//! `z * M` for a power-of-ten scale `M` and carry carefully analysed error
//! bounds (documented in the originals); they are deliberately NOT
//! "improved" — every `//`, `%`, and `>>` is Python floor semantics, mapped
//! to `num_integer`'s `div_floor`/`mod_floor` (the only negative-operand
//! shifts are re-expressed through them too, so no reliance on `BigInt`
//! bit-op sign semantics).
//!
//! Sandbox guards (see the module docs in `mod.rs`): every `10**k`
//! materialisation funnels through [`pow10`] (a hard exponent cap plus
//! `check_pow_size`), every Newton/Taylor/argument-reduction loop polls
//! `check_time`, and the correctly-rounded refinement loops (`extra += 3` /
//! `places += 3`) carry a hard round cap. The caps are unreachable for real
//! inputs (bounds are derived from `prec = 28` and the constructor's
//! `DECIMAL_MAX_DIGITS`/exponent limits and documented per call site) — they
//! are insurance against the table-maker's dilemma and contract violations.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Pow, Signed, ToPrimitive, Zero};

use super::{Decimal, EMAX, ETINY, PREC, RoundMode, allocate, check_nans, fix, magnitude_digits};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, RunResult},
    resource::ResourceTracker,
    value::Value,
};

/// Hard cap on the exponent of any `10**k` a kernel materialises. The largest
/// legitimate exponent is ≈ 4,700 (the `ln` of a maximal-digit near-1 literal:
/// `places ≤ prec + DECIMAL_MAX_DIGITS + 2 + 3·REFINEMENT_ROUNDS_CAP`; `sqrt`'s
/// base-100 rescale peaks at `10**(2·(DECIMAL_MAX_DIGITS/2 + 1))` ≈ `10**4306`),
/// so 16384 gives >3× headroom while bounding any single kernel `BigInt` at a
/// few kilobytes. Exceeding it means a caller lost its bound — internal error.
const POW10_EXP_CAP: i64 = 16_384;

/// Hard cap on the correctly-rounded refinement loops (`extra += 3` in `exp`,
/// `places += 3` in `ln`/`log10`, and `_log10_digits`' recompute loop). Each
/// extra round requires ~3 more digits of the (irrational) true result to be
/// `000`/`500`-degenerate — probability ~10⁻³ per round — so even 10 rounds is
/// astronomically unlikely. Insurance against the table-maker's dilemma.
const REFINEMENT_ROUNDS_CAP: u32 = 100;

/// Hard cap on Newton / argument-reduction iterations (`sqrt`'s Newton loop,
/// [`sqrt_nearest`], `_ilog`'s reduction loop). All converge in well under a
/// hundred iterations for capped-size inputs; unreachable insurance.
const KERNEL_ITERATIONS_CAP: u32 = 10_000;

/// `len(str((Emax+1)*3))` and `len(str((-Etiny()+1)*3))` under the fixed
/// context — both `len("3000000") == len("3000081") == 7`. An adjusted
/// exponent above this makes `exp` overflow (positive) or underflow to zero
/// (negative), so the shortcuts fire *before* any kernel work: a huge literal
/// exponent (adjusted up to ~±10¹⁸) never reaches `dexp`, keeping all its
/// internal `i64` exponent arithmetic bounded.
const EXP_ADJ_CUTOFF: i64 = 7;

/// `Decimal.sqrt()` — `_pydecimal` 2681-2778: the square root, correctly
/// rounded via an integer Newton iteration at `prec + 1` digits in base 100.
///
/// `sqrt(-0)` is `-0` (at half the operand exponent), `sqrt(+Inf)` is `+Inf`,
/// and any other negative operand raises `InvalidOperation`. An exact root
/// comes out at the ideal exponent `⌊exp/2⌋` naturally (the `exact` rescale).
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
pub(super) fn sqrt(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if d.is_special() {
        if let Some(nan) = check_nans(&d, None)? {
            return allocate(nan, vm);
        }
        if d.infinity_sign() == 1 {
            return allocate(d, vm);
        }
    }
    if d.is_zero() {
        // exponent = self._exp // 2 (floor); sqrt(-0) = -0.
        let ans = Decimal::from_triple(d.sign, BigInt::ZERO, d.exp.div_euclid(2));
        return allocate(fix::fix(ans, RoundMode::HalfEven)?, vm);
    }
    if d.sign == 1 {
        return Err(ExcType::decimal_invalid_operation());
    }

    let tracker = vm.heap.tracker();
    // Use an extra digit of precision: prec = context.prec + 1.
    let prec = PREC + 1;

    // Write the operand as c·100**e with e = self._exp // 2 the ideal
    // exponent; l is the number of base-100 "digits" of c.
    let len = i64::try_from(d.digits()).expect("digit count fits i64");
    let mut e = d.exp >> 1; // arithmetic shift = Python floor shift
    let (c, l) = if d.exp & 1 != 0 {
        (&d.coeff * 10, (len >> 1) + 1)
    } else {
        (d.coeff, (len + 1) >> 1)
    };

    // Rescale so that c has exactly prec base-100 digits. |shift| is bounded
    // by prec and the operand's digit count (l ≤ DECIMAL_MAX_DIGITS/2 + 1),
    // so the 100**|shift| = 10**(2·|shift|) below stays ≤ 10**~4306.
    let shift = prec - l;
    let (c, mut exact) = if shift >= 0 {
        (c * pow10(2 * shift, tracker)?, true)
    } else {
        let (q, remainder) = c.div_mod_floor(&pow10(2 * -shift, tracker)?);
        (q, remainder.is_zero())
    };
    e -= shift;

    // n = floor(sqrt(c)) by Newton's method from the over-estimate 10**prec
    // (c has exactly prec base-100 digits, so sqrt(c) ∈ [10**(prec-1), 10**prec)).
    let mut n = pow10(prec, tracker)?;
    let mut iterations = 0u32;
    loop {
        tracker.check_time()?;
        iterations += 1;
        if iterations > KERNEL_ITERATIONS_CAP {
            return Err(RunError::internal("decimal sqrt did not converge"));
        }
        let q = c.div_floor(&n);
        if n <= q {
            break;
        }
        n = (n + q) >> 1u32;
    }
    exact = exact && &n * &n == c;

    if exact {
        // Result is exact: rescale to use the ideal exponent e.
        n = if shift >= 0 {
            n / pow10(shift, tracker)? // n % 10**shift == 0
        } else {
            n * pow10(-shift, tracker)?
        };
        e += shift;
    } else if (&n % 5u32).is_zero() {
        // Inexact: bump a last digit of 0/5 to 1/6 so the final round to prec
        // places is correct under any mode (the "extra digit" trick).
        n += 1u32;
    }

    // CPython forces ROUND_HALF_EVEN into the final _fix (2773-2776); the
    // fixed context's default rounding is already HALF_EVEN.
    let ans = Decimal::from_triple(0, n, e);
    allocate(fix::fix(ans, RoundMode::HalfEven)?, vm)
}

/// `Decimal.exp()` — `_pydecimal` 3001-3074: `e**self`, correctly rounded.
///
/// Overflow/underflow shortcuts fire for any adjusted exponent outside
/// `[-prec-1, 7]` *before* any kernel work (see [`EXP_ADJ_CUTOFF`]), so
/// `Decimal('1E+999999999999').exp()` raises `Overflow` (via `fix`) and its
/// negative twin underflows to `0E-1000026` without a single big allocation.
pub(super) fn exp(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if let Some(nan) = check_nans(&d, None)? {
        return allocate(nan, vm);
    }
    if d.infinity_sign() == -1 {
        return allocate(Decimal::zero(), vm); // exp(-Infinity) = 0
    }
    if d.is_zero() {
        return allocate(Decimal::from_i64(1), vm); // exp(0) = 1
    }
    if d.infinity_sign() == 1 {
        return allocate(d, vm); // exp(Infinity) = Infinity
    }

    let adj = d.adjusted();
    let ans = if d.sign == 0 && adj > EXP_ADJ_CUTOFF {
        // Guaranteed overflow: the sentinel 1E(Emax+1) trips Overflow in fix.
        Decimal::from_triple(0, BigInt::one(), EMAX + 1)
    } else if d.sign == 1 && adj > EXP_ADJ_CUTOFF {
        // Guaranteed underflow: the sentinel 1E(Etiny-1) rounds to zero in fix.
        Decimal::from_triple(0, BigInt::one(), ETINY - 1)
    } else if d.sign == 0 && adj < -PREC {
        // Indistinguishable from 1 at this precision: p+1 digits '1'+'0'*(p-1)+'1'
        // so the final round carries the right inexact direction.
        Decimal::from_triple(0, pow10(PREC, vm.heap.tracker())? + 1u32, -PREC)
    } else if d.sign == 1 && adj < -PREC - 1 {
        // Just below 1: p+1 nines.
        Decimal::from_triple(0, pow10(PREC + 1, vm.heap.tracker())? - 1u32, -(PREC + 1))
    } else {
        // General case: -29 <= adj <= 7, so e below is bounded by the
        // operand's digit count (≥ adj - digits + 1 ≥ ~-4330).
        let tracker = vm.heap.tracker();
        let c = if d.sign == 1 { -&d.coeff } else { d.coeff.clone() };
        let e = d.exp;
        // Increase precision by 3 digits at a time until unambiguously roundable.
        let mut extra = 3i64;
        let mut rounds = 0u32;
        let (coeff, exp) = loop {
            tracker.check_time()?;
            rounds += 1;
            if rounds > REFINEMENT_ROUNDS_CAP {
                return Err(RunError::internal("decimal exp did not converge"));
            }
            let (coeff, exp) = dexp(&c, e, PREC + extra, tracker)?;
            if unambiguously_roundable(&coeff, tracker)? {
                break (coeff, exp);
            }
            extra += 3;
        };
        Decimal::from_triple(0, coeff, exp)
    };

    // At this stage ans rounds correctly with any mode; CPython forces
    // ROUND_HALF_EVEN into _fix (3068-3071) — already our fixed default.
    allocate(fix::fix(ans, RoundMode::HalfEven)?, vm)
}

/// `Decimal.ln()` — `_pydecimal` 3157-3205: the natural logarithm, correctly
/// rounded via the [`dlog`] fixed-point kernel.
///
/// `ln(±0)` is `-Infinity` (returned directly, no signal — matches the C
/// module), `ln(+Inf)` is `+Infinity`, `ln(1)` is exactly `Decimal('0')`, and
/// any negative operand (including `-Infinity`) raises `InvalidOperation`.
pub(super) fn ln(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if let Some(nan) = check_nans(d, None)? {
        return allocate(nan, vm);
    }
    if d.is_zero() {
        return allocate(Decimal::infinity(1), vm);
    }
    if d.infinity_sign() == 1 {
        return allocate(Decimal::infinity(0), vm);
    }
    if is_positive_one(d) {
        return allocate(Decimal::zero(), vm);
    }
    if d.sign == 1 {
        return Err(ExcType::decimal_invalid_operation());
    }

    // Result is irrational, hence inexact: repeatedly increase precision by 3
    // until the approximation is unambiguously roundable (at least p+3 places).
    let tracker = vm.heap.tracker();
    let mut places = PREC - ln_exp_bound(d, tracker)? + 2;
    let mut rounds = 0u32;
    let coeff = loop {
        tracker.check_time()?;
        rounds += 1;
        if rounds > REFINEMENT_ROUNDS_CAP {
            return Err(RunError::internal("decimal ln did not converge"));
        }
        let coeff = dlog(&d.coeff, d.exp, places, tracker)?;
        if unambiguously_roundable(&coeff, tracker)? {
            break coeff;
        }
        places += 3;
    };

    // CPython forces ROUND_HALF_EVEN into _fix (3199-3203) — our fixed default.
    let ans = Decimal::from_triple(u8::from(coeff.is_negative()), coeff.abs(), -places);
    allocate(fix::fix(ans, RoundMode::HalfEven)?, vm)
}

/// `Decimal.log10()` — `_pydecimal` 3237-3286: the base-10 logarithm.
///
/// Same special cases as [`ln`]; an exact power of ten short-circuits to the
/// integer answer `adjusted()` (which may itself still need rounding).
pub(super) fn log10(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if let Some(nan) = check_nans(d, None)? {
        return allocate(nan, vm);
    }
    if d.is_zero() {
        return allocate(Decimal::infinity(1), vm);
    }
    if d.infinity_sign() == 1 {
        return allocate(Decimal::infinity(0), vm);
    }
    if d.sign == 1 {
        return Err(ExcType::decimal_invalid_operation());
    }

    let ans = if coeff_is_power_of_ten(d) {
        // log10(10**n) = n; the answer may still need rounding in fix.
        Decimal::from_i64(d.adjusted())
    } else {
        // Irrational, hence inexact: refine until unambiguously roundable.
        let tracker = vm.heap.tracker();
        let mut places = PREC - log10_exp_bound(d, tracker)? + 2;
        let mut rounds = 0u32;
        let coeff = loop {
            tracker.check_time()?;
            rounds += 1;
            if rounds > REFINEMENT_ROUNDS_CAP {
                return Err(RunError::internal("decimal log10 did not converge"));
            }
            let coeff = dlog10(&d.coeff, d.exp, places, tracker)?;
            if unambiguously_roundable(&coeff, tracker)? {
                break coeff;
            }
            places += 3;
        };
        Decimal::from_triple(u8::from(coeff.is_negative()), coeff.abs(), -places)
    };

    // CPython forces ROUND_HALF_EVEN into _fix (3280-3284) — our fixed default.
    allocate(fix::fix(ans, RoundMode::HalfEven)?, vm)
}

/// Whether `d` equals exactly `Decimal(1)` (`self == _One` in `ln`, 3175):
/// positive, adjusted exponent 0, coefficient of the form `1000…0`.
fn is_positive_one(d: &Decimal) -> bool {
    d.sign == 0 && d.adjusted() == 0 && coeff_is_power_of_ten(d)
}

/// `log10`'s exact-power test (3261): the coefficient digit string is `'1'`
/// followed only by `'0'`s. False for a zero coefficient (`"0"`).
fn coeff_is_power_of_ten(d: &Decimal) -> bool {
    let s = d.coeff_str();
    let bytes = s.as_bytes();
    bytes[0] == b'1' && bytes[1..].iter().all(|&b| b == b'0')
}

/// `Decimal._ln_exp_bound` — `_pydecimal` 3132-3155: a lower bound `r` such
/// that `|self.ln()| >= 10**r`. Requires `self` finite, positive, and `!= 1`.
///
/// The `adj ∈ {0, -1}` branches materialise `10**-exp` with `-exp` bounded by
/// the operand's digit count (≤ `DECIMAL_MAX_DIGITS`, since `adjusted()` pins
/// `exp` to `-digits + {0,1}` there). The `adj` products use `i128`: `adjusted`
/// can reach ~±10¹⁸, so `adj * 23` would overflow `i64`.
fn ln_exp_bound(d: &Decimal, tracker: &impl ResourceTracker) -> RunResult<i64> {
    let adj = d.adjusted();
    if adj >= 1 {
        // argument >= 10; 23/10 = 2.3 is a lower bound for ln(10).
        Ok(decimal_digit_count(i128::from(adj) * 23 / 10) - 1)
    } else if adj <= -2 {
        // argument <= 0.1
        Ok(decimal_digit_count((-1 - i128::from(adj)) * 23 / 10) - 1)
    } else if adj == 0 {
        // 1 < self < 10 (self != 1 is a caller precondition, so num > 0).
        let num = (&d.coeff - pow10(-d.exp, tracker)?).to_string();
        let den = d.coeff_str();
        // Python compares the digit strings lexicographically; Rust `<` on
        // ASCII strings is the same comparison.
        Ok(str_len(&num) - str_len(&den) - i64::from(num < den))
    } else {
        // adj == -1: 0.1 <= self < 1.
        let num = (pow10(-d.exp, tracker)? - &d.coeff).to_string();
        Ok(d.exp + str_len(&num) - 1)
    }
}

/// `Decimal._log10_exp_bound` — `_pydecimal` 3207-3235: a lower bound `r` such
/// that `|self.log10()| >= 10**r`. Requires `self` finite, positive, `!= 1`.
///
/// `pub(super)` because `__pow__` (2375) uses it for its overflow/underflow
/// bound check (`bound = self._log10_exp_bound() + other.adjusted()`). Same
/// digit-count bounds as [`ln_exp_bound`].
pub(super) fn log10_exp_bound(d: &Decimal, tracker: &impl ResourceTracker) -> RunResult<i64> {
    let adj = d.adjusted();
    if adj >= 1 {
        // self >= 10
        Ok(decimal_digit_count(i128::from(adj)) - 1)
    } else if adj <= -2 {
        // self < 0.1
        Ok(decimal_digit_count(i128::from(-1 - adj)) - 1)
    } else if adj == 0 {
        // 1 < self < 10: |log10(x)| > (1 - 1/x)/2.31.
        let num = (&d.coeff - pow10(-d.exp, tracker)?).to_string();
        let den = (&d.coeff * 231i64).to_string();
        Ok(str_len(&num) - str_len(&den) - i64::from(num < den) + 2)
    } else {
        // adj == -1: 0.1 <= self < 1: |log10(x)| > (1 - x)/2.31.
        let num = (pow10(-d.exp, tracker)? - &d.coeff).to_string();
        Ok(str_len(&num) + d.exp - i64::from(num.as_str() < "231") - 1)
    }
}

/// The correctly-rounded refinement test shared by `exp`/`ln`/`log10`: true
/// when `coeff % (5 * 10**(len(str(abs(coeff))) - prec - 1))` is nonzero,
/// i.e. the approximation cannot straddle a rounding boundary at `prec`
/// digits. The cut is bounded by the refinement round (`extra ≤ 3·cap + 1`).
///
/// CPython asserts `len - p >= 1`; a shorter coefficient (impossible per the
/// kernels' contracts) is treated as roundable so the loop cannot spin.
fn unambiguously_roundable(coeff: &BigInt, tracker: &impl ResourceTracker) -> RunResult<bool> {
    let cut = magnitude_digits(coeff) - PREC - 1;
    if cut < 0 {
        Ok(true)
    } else {
        let modulus = BigInt::from(5) * pow10(cut, tracker)?;
        Ok(!coeff.mod_floor(&modulus).is_zero())
    }
}

/// `_pydecimal._dpower` (5943-5983): given `x = xc·10**xe` (positive, ≠ 1) and
/// `y = yc·10**ye` (nonzero, `yc` sign-carrying), computes `x**y` as `(c, e)`
/// with `10**(p-1) <= c <= 10**p` and an error in `c` of at most 1.
///
/// Consumed by `__pow__`'s inexact path. The caller's overflow/underflow bound
/// check (`_log10_exp_bound() + other.adjusted() < len(str(Emax))`) keeps
/// `|y·log(x)|` under ~10⁹ before this runs, which bounds every internal
/// exponent; the arithmetic below still uses saturating ops and guarded
/// [`pow10`] so a contract violation degrades to an internal error, never an
/// unbounded allocation.
pub(super) fn dpower(
    xc: &BigInt,
    xe: i64,
    yc: &BigInt,
    ye: i64,
    p: i64,
    tracker: &impl ResourceTracker,
) -> RunResult<(BigInt, i64)> {
    // Find b such that 10**(b-1) <= |y| <= 10**b.
    let b = magnitude_digits(yc).saturating_add(ye);

    // log(x) = lxc·10**(-p-b-1), to p+b+1 places after the decimal point.
    let lxc = dlog(xc, xe, p.saturating_add(b).saturating_add(1), tracker)?;

    // y·log(x) = yc·lxc·10**(-p-b-1+ye) = pc·10**(-p-1); shift = ye - b is
    // always -len(str(abs(yc))) (≤ -1, bounded by the operand's digits), but
    // the non-negative branch is ported for line fidelity.
    let shift = ye.saturating_sub(b);
    let pc = if shift >= 0 {
        &lxc * yc * pow10(shift, tracker)?
    } else {
        div_nearest(&(&lxc * yc), &pow10(-shift, tracker)?)
    };

    if pc.is_zero() {
        // We prefer a result that isn't exactly 1 (it makes the correctly
        // rounded result easier for __pow__): pick the side by whether x**y > 1.
        let x_gt_one = magnitude_digits(xc).saturating_add(xe) >= 1;
        if x_gt_one == yc.is_positive() {
            Ok((pow10(p - 1, tracker)? + 1u32, 1 - p))
        } else {
            Ok((pow10(p, tracker)? - 1u32, -p))
        }
    } else {
        let (coeff, exp) = dexp(&pc, -(p + 1), p + 1, tracker)?;
        Ok((div_nearest(&coeff, &BigInt::from(10)), exp + 1))
    }
}

/// `_pydecimal._log10_lb` (5985-5992): a lower bound for `100 * log10(c)` for
/// a positive integer `c`, from the digit count and a first-digit correction
/// table. `__pow__`'s `_power_exact` (2229) divides by it, so callers must
/// guarantee `c > 1` (for `c == 1` the bound is 0).
pub(super) fn log10_lb(c: &BigInt) -> i64 {
    debug_assert!(c.is_positive(), "log10_lb requires a positive argument");
    let s = c.to_string();
    let correction = match s.as_bytes()[0] {
        b'1' => 100,
        b'2' => 70,
        b'3' => 53,
        b'4' => 40,
        b'5' => 31,
        b'6' => 23,
        b'7' => 16,
        b'8' => 10,
        _ => 5, // '9' — positive integers never start with '0'
    };
    100 * str_len(&s) - correction
}

/// `_pydecimal._decimal_lshift_exact` (5655-5674): `n * 10**e` if that is an
/// integer, else `None`. Used by `_power_exact` to test whether candidate
/// exponents (`e·yc` shifted by `ye`) are integral.
///
/// Sandbox guard: a positive shift above ~10⁴ returns `None` instead of
/// materialising the power. This is unreachable for any representable result —
/// `__pow__`'s bound check rejects everything with `|log10(x**y)| ≥ ~10⁹`
/// before `_power_exact` runs, and these shifted values *are* result exponents
/// — so the divergence only reroutes impossible inputs to the inexact path.
pub(super) fn decimal_lshift_exact(n: &BigInt, e: i64) -> Option<BigInt> {
    /// The defensive positive-shift cap (see above): capped results would have
    /// ≥ 10⁴ digits and can never contribute to a representable exact power.
    const LSHIFT_EXACT_CAP: i64 = 10_000;
    if n.is_zero() {
        Some(BigInt::ZERO)
    } else if e >= 0 {
        (e <= LSHIFT_EXACT_CAP).then(|| n * BigInt::from(10).pow(u32::try_from(e).expect("capped shift fits u32")))
    } else {
        // val_n = largest power of 10 dividing n; exact iff it covers -e.
        // (-e ≤ val_n ≤ digits ≤ DECIMAL_MAX_DIGITS whenever we divide.)
        let digits = n.magnitude().to_string();
        let val_n = str_len(&digits) - str_len(digits.trim_end_matches('0'));
        let neg_e = e.checked_neg()?; // e == i64::MIN can never divide exactly
        (val_n >= neg_e).then(|| n / BigInt::from(10).pow(u32::try_from(neg_e).expect("bounded by val_n")))
    }
}

/// `_pydecimal._sqrt_nearest` (5676-5689): the closest integer to `sqrt(n)`
/// for positive `n`, by Newton's method from the initial over-estimate `a`.
///
/// Python's update `a--n//a>>1` parses as `(a - ((-n) // a)) >> 1`, i.e.
/// `(a + ceil(n/a)) >> 1` — the `(-n) // a` floor division is what makes the
/// iterate round *up*, which the convergence proof relies on. Non-positive
/// arguments are a caller bug (CPython raises `ValueError`): internal error.
pub(super) fn sqrt_nearest(n: &BigInt, a: &BigInt, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    if !n.is_positive() || !a.is_positive() {
        return Err(RunError::internal("decimal sqrt_nearest requires positive arguments"));
    }
    let mut a = a.clone();
    let mut b = BigInt::ZERO;
    let mut iterations = 0u32;
    while a != b {
        tracker.check_time()?;
        iterations += 1;
        if iterations > KERNEL_ITERATIONS_CAP {
            return Err(RunError::internal("decimal sqrt_nearest did not converge"));
        }
        // (a + ceil(n/a)) >> 1 — the shifted value is positive, so the shift
        // needs no floor-semantics care.
        let next = (&a - (-n).div_floor(&a)) >> 1u32;
        b = a;
        a = next;
    }
    Ok(a)
}

/// `_pydecimal._rshift_nearest` (5691-5697): the closest integer to
/// `x / 2**shift` (`shift >= 0`), ties rounded to even.
///
/// Expressed via `div_floor`/`mod_floor` on `2**shift` rather than `>>`/`&` so
/// Python's floor-shift and two's-complement-mask semantics for negative `x`
/// hold by construction (`x & (b-1) == x.mod_floor(b)`, `x >> s == x.div_floor(b)`).
pub(super) fn rshift_nearest(x: &BigInt, shift: u64) -> BigInt {
    let b = BigInt::one() << shift;
    let q = x.div_floor(&b);
    // q + (2*(x & (b-1)) + (q&1) > b); q&1 is q.mod_floor(2), i.e. is_odd.
    let tie = x.mod_floor(&b) * 2 + u32::from(q.is_odd());
    if tie > b { q + 1u32 } else { q }
}

/// `_pydecimal._div_nearest` (5699-5705): the closest integer to `a / b` for
/// positive `b`, ties rounded to even.
///
/// The docstring in `_pydecimal` claims both operands positive, but the
/// kernels routinely pass a negative `a` (Taylor terms, `yc·lxc` products);
/// Python's `divmod` is floor-based, so `div_floor`/`mod_floor` reproduce it
/// exactly (`r ∈ [0, b)` and `q & 1 == q.is_odd()` for negative `q` too).
pub(super) fn div_nearest(a: &BigInt, b: &BigInt) -> BigInt {
    debug_assert!(b.is_positive(), "div_nearest requires a positive divisor");
    let (q, r) = a.div_mod_floor(b);
    let tie = r * 2 + u32::from(q.is_odd());
    if tie > *b { q + 1u32 } else { q }
}

/// `_pydecimal._dexp` (5907-5941): an approximation to `exp(c·10**e)` with `p`
/// decimal digits of precision, returned as `(d, f)` with
/// `10**(p-1) <= d <= 10**p` and error in `d` at most 1.
///
/// Exponent arithmetic is bounded by the callers: `exp`'s shortcuts pin the
/// operand's adjusted exponent to `[-prec-2, 7]` (so `e ∈ [~-4630, 8]` and the
/// quotient below is < ~10⁸), and `dpower` passes `e = -(p+1)` with `|pc|`
/// bounded by `__pow__`'s overflow check. A quotient outside `i64` therefore
/// means a broken caller contract — internal error, never a bad allocation.
/// (Line fidelity note: CPython's `len(str(c))` in the `extra` computation
/// counts the `'-'` of a negative `c`, harmlessly adding one digit of margin;
/// reproduced here.)
fn dexp(c: &BigInt, e: i64, p: i64, tracker: &impl ResourceTracker) -> RunResult<(BigInt, i64)> {
    // We'll call iexp with M = 10**(p+2), giving p+3 digits of precision.
    let p = p.saturating_add(2);

    // Extra precision for log(10) = the adjusted exponent of c·10**e.
    let signed_len = str_len(&c.to_string());
    let extra = (e.saturating_add(signed_len) - 1).max(0);
    let q = p.saturating_add(extra);

    // Quotient c·10**(e+q) / (log(10)·10**q), rounding down (floor: c may be
    // negative). |shift| ≤ |e| + p + extra, all bounded per the doc above.
    let shift = e.saturating_add(q);
    let cshift = if shift >= 0 {
        c * pow10(shift, tracker)?
    } else {
        c.div_floor(&pow10(-shift, tracker)?)
    };
    let (quot, rem) = cshift.div_mod_floor(&log10_digits(q, tracker)?);

    // Reduce the remainder back to the original precision.
    let rem = div_nearest(&rem, &pow10(extra, tracker)?);

    // Error in the result of iexp < 120; error after the division < 0.62.
    let coeff = div_nearest(&iexp(&rem, &pow10(p, tracker)?, tracker)?, &BigInt::from(1000));
    let quot = quot
        .to_i64()
        .ok_or_else(|| RunError::internal("decimal dexp exponent out of bounds"))?;
    Ok((coeff, quot - p + 3))
}

/// `_pydecimal._iexp` (5870-5905): an integer approximation to `M·exp(x/M)`
/// for `0 <= x/M <= 2.4`, with absolute error at most 60 — argument halving
/// (`R` doublings) plus a Taylor series for `expm1`, all in fixed point.
///
/// The contract bounds `R` at ~10 (`R = nbits((x << 8) // M)`); a larger `R`
/// means the caller broke the `x/M` bound — internal error.
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
fn iexp(x: &BigInt, m: &BigInt, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    const L: u32 = 8;

    // Find R such that x / 2**R / M <= 2**-L.
    let r = ((x << L) / m).bits();
    let r = u32::try_from(r)
        .ok()
        .filter(|&r| r <= 64)
        .ok_or_else(|| RunError::internal("decimal iexp argument out of range"))?;

    // Taylor series with T terms: (2**L)**T > M. The explicit ceiling avoids
    // the `Integer::div_ceil` name collision: -int(-10*len(str(M))//(3*L)).
    let t = (10 * magnitude_digits(m) + 23) / 24;
    let mut y = div_nearest(x, &BigInt::from(t));
    let mshift = m << r;
    for i in (1..t).rev() {
        tracker.check_time()?;
        y = div_nearest(&(x * (&mshift + &y)), &(&mshift * i));
    }

    // Expansion: expm1(2z) = expm1(z)·(expm1(z) + 2), R times.
    for k in (0..r).rev() {
        tracker.check_time()?;
        let mshift = m << (k + 2);
        y = div_nearest(&(&y * (&y + &mshift)), &mshift);
    }

    Ok(m + y)
}

/// `_pydecimal._dlog` (5789-5831): an integer approximation to
/// `10**p · log(c·10**e)` for `c > 0`, absolute error at most 1.
///
/// `k = e + p - f` collapses algebraically to `p - l + (0|1)` (computed that
/// way to avoid an `e + p` overflow for saturated `p`), so the `10**|k|`
/// materialisation is bounded by `p` and the operand's digit count; the `f`
/// term's `10**extra` is bounded by `extra ≤ 18` (`f` fits `i64`).
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
fn dlog(c: &BigInt, e: i64, p: i64, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    // Increase precision by 2; compensated by the final division by 100.
    let p = p.saturating_add(2);

    // Rewrite c·10**e as d·10**f with f >= 0 and 1 <= d <= 10, or f <= 0 and
    // 0.1 <= d <= 1: 10**p·log(c·10**e) = 10**p·log(d) + 10**p·f·log(10).
    let l = magnitude_digits(c);
    let f = e + l - i64::from(e + l >= 1);

    // Approximation to 10**p·log(d), error < 27 (ilog magnifies the ≤ 0.5
    // rescale error in c by at most 10: 5 + 22).
    let log_d = if p > 0 {
        let k = p - l + i64::from(e + l >= 1); // == e + p - f
        let scaled = if k >= 0 {
            c * pow10(k, tracker)?
        } else {
            div_nearest(c, &pow10(-k, tracker)?)
        };
        ilog(&scaled, &pow10(p, tracker)?, tracker)?
    } else {
        // p <= 0: approximate log(d) by 0; error < 2.31.
        BigInt::ZERO
    };

    // Approximation to f·10**p·log(10), error < 11.
    let f_log_ten = if f != 0 {
        let extra = str_len(&f.unsigned_abs().to_string()) - 1;
        if p.saturating_add(extra) >= 0 {
            // Error in f·log10_digits(p+extra) < |f|; after division
            // < |f|/10**extra + 0.5 < 10.5 < 11.
            div_nearest(&(log10_digits(p + extra, tracker)? * f), &pow10(extra, tracker)?)
        } else {
            BigInt::ZERO
        }
    } else {
        BigInt::ZERO
    };

    // Error in the sum < 38; after the division by 100 < 0.38 + 0.5 < 1.
    Ok(div_nearest(&(f_log_ten + log_d), &BigInt::from(100)))
}

/// `_pydecimal._dlog10` (5755-5787): an integer approximation to
/// `10**p · log10(c·10**e)` for `c > 0`, absolute error at most 1.
///
/// Only called from [`log10`], whose `places` is always ≥ 12 (see
/// [`log10_exp_bound`]), so the `p <= 0` branch — whose `10**-p` would be
/// exponent-derived — is unreachable; [`pow10`]'s guard covers it regardless.
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
fn dlog10(c: &BigInt, e: i64, p: i64, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    // Increase precision by 2; compensated by the final division by 100.
    let p = p.saturating_add(2);

    // As in dlog: c·10**e = d·10**f with d near 1, so that
    // 10**p·log10(c·10**e) = 10**p·(f + log10(d)).
    let l = magnitude_digits(c);
    let f = e + l - i64::from(e + l >= 1);

    if p > 0 {
        let m = pow10(p, tracker)?;
        let k = p - l + i64::from(e + l >= 1); // == e + p - f, bounded by p and digits
        let scaled = if k >= 0 {
            c * pow10(k, tracker)?
        } else {
            div_nearest(c, &pow10(-k, tracker)?)
        };
        let log_d = ilog(&scaled, &m, tracker)?; // error < 5 + 22 = 27
        let log_10 = log10_digits(p, tracker)?; // error < 1
        let log_d = div_nearest(&(log_d * &m), &log_10);
        let log_tenpower = &m * f; // exact
        Ok(div_nearest(&(log_tenpower + log_d), &BigInt::from(100)))
    } else {
        // log_d = 0, error < 2.31; f/10**-p, error < 0.5.
        let log_tenpower = div_nearest(&BigInt::from(f), &pow10(p.saturating_neg(), tracker)?);
        Ok(div_nearest(&log_tenpower, &BigInt::from(100)))
    }
}

/// `_pydecimal._ilog` (5707-5753): an integer approximation to `M·log(x/M)`
/// with error at most 22 for `0.1 <= x/M <= 10` — repeated
/// `log1p(y) = 2·log1p(y/(1+sqrt(1+y)))` argument reduction, then a Taylor
/// series, in fixed point scaled by `M` (with `y` scaled by `2**R·M`).
///
/// The reduction count is ~`L + log2(log(x/M))` ≈ 12 for in-contract inputs;
/// the Taylor term count `T = ceil(10·len(str(M))/24)` is bounded by `M`'s
/// digits (≤ [`POW10_EXP_CAP`]). Both loops poll `check_time`.
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
fn ilog(x: &BigInt, m: &BigInt, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    const L: u64 = 8;

    let mut y = x - m;
    // Argument reduction; r = number of reductions performed so far.
    let mut r: u64 = 0;
    let mut iterations = 0u32;
    loop {
        tracker.check_time()?;
        let reduce = if r <= L {
            (y.abs() << (L - r)) >= *m
        } else {
            // y.abs() is non-negative, so the shift is floor regardless.
            (y.abs() >> (r - L)) >= *m
        };
        if !reduce {
            break;
        }
        iterations += 1;
        if iterations > KERNEL_ITERATIONS_CAP {
            return Err(RunError::internal("decimal ilog did not converge"));
        }
        let root = sqrt_nearest(&(m * (m + rshift_nearest(&y, r))), m, tracker)?;
        y = div_nearest(&((m * &y) << 1u32), &(m + root));
        r += 1;
    }

    // Taylor series with T terms: log1p(y) ~ y - y²/2 + y³/3 - … The explicit
    // ceiling avoids the `Integer::div_ceil` collision: -int(-10*len(str(M))//(3*L)).
    let t = (10 * magnitude_digits(m) + 23) / 24;
    let yshift = rshift_nearest(&y, r);
    let mut w = div_nearest(m, &BigInt::from(t));
    for k in (1..t).rev() {
        tracker.check_time()?;
        w = div_nearest(m, &BigInt::from(k)) - div_nearest(&(&yshift * &w), m);
    }

    Ok(div_nearest(&(w * y), m))
}

/// The first 47 digits of `log(10) = 2.302585…` — `_pydecimal._Log10Memoize`'s
/// seed (5838), copied verbatim. Covers every request with `p + 1 <= 47`,
/// which includes all first-round prec-28 computations (`p ≈ 30-40`).
const LOG10_DIGITS_SEED: &str = "23025850929940456840179914546843642076011014886";

/// `_pydecimal._log10_digits` (5833-5868): `floor(10**p · log(10))`.
///
/// Deliberately **no** mutable memo (CPython's `_Log10Memoize` grows a global
/// string): a process-global cache would grow without bound under attacker
/// control and make execution state depend on prior runs, breaking snapshot
/// determinism. Instead the seed constant serves `p <= 46` and larger `p`
/// (only reachable via many-digit operands or deep refinement rounds, bounded
/// by [`POW10_EXP_CAP`]) recompute from `_ilog(10·M, M)` per call.
fn log10_digits(p: i64, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    if p < 0 {
        return Err(RunError::internal("decimal log10_digits requires p >= 0"));
    }
    let want = usize::try_from(p + 1).expect("p+1 fits usize");
    if want <= LOG10_DIGITS_SEED.len() {
        return Ok(parse_digits(&LOG10_DIGITS_SEED[..want]));
    }

    // Compute p+extra digits, correct to within 1 ulp, until at least one of
    // the extra digits is nonzero (so truncating to p+1 digits is reliable).
    let mut extra = 3i64;
    let mut rounds = 0u32;
    loop {
        tracker.check_time()?;
        rounds += 1;
        if rounds > REFINEMENT_ROUNDS_CAP {
            return Err(RunError::internal("decimal log10_digits did not converge"));
        }
        let m = pow10(p + extra + 2, tracker)?;
        let digits = div_nearest(&ilog(&(&m * 10), &m, tracker)?, &BigInt::from(100)).to_string();
        let tail = usize::try_from(extra).expect("extra fits usize");
        if digits.len() > tail && !digits[digits.len() - tail..].bytes().all(|b| b == b'0') {
            // CPython memoises `digits.rstrip('0')[:-1]` (≥ p+1 reliable
            // chars) and returns its first p+1 digits — identical to slicing
            // the computed string directly.
            return digits
                .get(..want)
                .map(parse_digits)
                .ok_or_else(|| RunError::internal("decimal log10_digits produced too few digits"));
        }
        extra += 3;
    }
}

/// Parses an ASCII digit string into a `BigInt` (kernel-internal slices only).
fn parse_digits(s: &str) -> BigInt {
    BigInt::parse_bytes(s.as_bytes(), 10).expect("kernel digit slice is ASCII digits")
}

/// `10**exp` under the kernels' extra guard: the hard [`POW10_EXP_CAP`] (a
/// breached cap means a caller lost its documented bound — internal error) on
/// top of the shared tracker-checked [`super`] `pow10`. All kernel powers of
/// ten funnel through here.
fn pow10(exp: i64, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    if !(0..=POW10_EXP_CAP).contains(&exp) {
        return Err(RunError::internal("decimal power-of-ten exponent out of bounds"));
    }
    super::pow10(exp, tracker)
}

/// A string's length as the `i64` the exponent arithmetic works in.
fn str_len(s: &str) -> i64 {
    i64::try_from(s.len()).expect("string length fits i64")
}

/// Python's `len(str(v))` for a non-negative `i128` — the exponent-bound
/// estimates multiply `adjusted()` (up to ~10¹⁸) by 23, which needs `i128`.
fn decimal_digit_count(v: i128) -> i64 {
    debug_assert!(v >= 0, "digit count of a non-negative value");
    str_len(&v.to_string())
}
