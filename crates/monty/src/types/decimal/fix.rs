//! The rounding kernel: `_pydecimal._fix` (result finalisation under the
//! fixed context), the eight `ROUND_*` decision functions, `_rescale`,
//! `_round`, and per-call `rounding=` resolution.
//!
//! Everything here follows `_pydecimal` line-for-line, operating on the
//! coefficient's digit string exactly as CPython does (post-`fix` coefficients
//! are ≤ 28 digits and unfixed constructor operands are ≤
//! [`DECIMAL_MAX_DIGITS`], so the per-call stringify is cheap). Under the
//! fixed context only the `Overflow` signal can trap; the ignored-signal calls
//! (`Inexact`/`Rounded`/`Subnormal`/`Underflow`/`Clamped`) are omitted.

use std::iter::repeat_n;

use num_bigint::BigInt;
use num_traits::Zero;

use super::{DECIMAL_MAX_DIGITS, Decimal, EMAX, ETINY, ETOP, PREC, ROUNDING_MODES, RoundMode};
use crate::exception_private::{ExcType, RunError, RunResult, SimpleException};

/// `_pydecimal._fix`: rounds `d` to the working precision and clamps its
/// exponent into range — the finalisation step of every arithmetic result.
/// The only trapped outcome is `Overflow`; underflow quietly rounds toward
/// [`ETINY`] (down to a signed zero when nothing survives).
///
/// `rounding` is a parameter (not always `HalfEven`) because exp/pow force
/// `ROUND_HALF_EVEN` while quantize/`__round__` thread the per-call mode into
/// their own rescale before `fix` — mirror CPython if a mutable context is
/// ever reintroduced.
pub(super) fn fix(d: Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_special() {
        return Ok(if d.is_nan() { fix_nan(d) } else { d });
    }

    // A zero only has its exponent clamped into `[Etiny, Emax]` (clamp=0).
    if d.coeff_is_zero() {
        let new_exp = d.exp.clamp(ETINY, EMAX);
        return Ok(if new_exp == d.exp {
            d
        } else {
            Decimal::from_triple(d.sign, BigInt::ZERO, new_exp)
        });
    }

    let len = i64::try_from(d.digits()).expect("digit count fits i64");
    // The smallest allowable exponent of the result: rounding to `prec`
    // digits puts the last kept digit at this exponent.
    let exp_min = len + d.exp - PREC;
    if exp_min > ETOP {
        // Overflow: `exp_min > Etop` iff `d.adjusted() > Emax`.
        return Err(ExcType::decimal_overflow());
    }
    // A subnormal result rounds at Etiny instead (Underflow/Subnormal are
    // untrapped, so subnormality itself needs no bookkeeping here).
    let mut exp_min = exp_min.max(ETINY);

    if d.exp < exp_min {
        // Too many digits: round at the cut. A value smaller than the least
        // representable magnitude is replaced by the sentinel `1E(exp_min-1)`
        // so the decision functions see "everything below the cut".
        let int_str = if len + d.exp - exp_min < 0 {
            "1".to_owned()
        } else {
            d.coeff_str()
        };
        let digits = usize::try_from((len + d.exp - exp_min).max(0)).expect("non-negative cut fits usize");
        let changed = round_decision(&int_str, digits, d.sign, rounding);
        let mut coeff = if digits == 0 {
            "0".to_owned()
        } else {
            int_str[..digits].to_owned()
        };
        if changed > 0 {
            // Increment the kept digits; a carry past `prec` digits sheds the
            // surplus trailing zero into the exponent.
            coeff = (parse_digits(&coeff) + 1u8).to_string();
            if coeff.len() > super::DEFAULT_PREC {
                coeff.pop();
                exp_min += 1;
            }
        }
        if exp_min > ETOP {
            // The rounding carry pushed the exponent out of range.
            return Err(ExcType::decimal_overflow());
        }
        return Ok(Decimal::from_triple(d.sign, parse_digits(&coeff), exp_min));
    }

    // Representable to begin with (the clamp=1 fold-down branch of CPython's
    // `_fix` is dead code under the fixed clamp=0 and is deliberately not
    // ported — it would pad the coefficient by up to ~Emax digits).
    Ok(d)
}

/// `_pydecimal._fix_nan`: decapitates a NaN payload to the working precision
/// (keeps the *last* `prec` digits, leading zeros stripped by the re-parse).
pub(super) fn fix_nan(d: Decimal) -> Decimal {
    let payload = d.coeff_str();
    if !d.coeff_is_zero() && payload.len() > super::DEFAULT_PREC {
        let kept = parse_digits(&payload[payload.len() - super::DEFAULT_PREC..]);
        Decimal { coeff: kept, ..d }
    } else {
        d
    }
}

/// `_pydecimal._rescale`: returns `d` with exponent `exp`, padding with zeros
/// or rounding under `rounding`. Specials pass through unchanged; the
/// operation is quiet (no signals) and context-free.
///
/// The zero-padding length is defensively bounded: every caller's own checks
/// (quantize's digit bounds, `to_integral`'s `exp >= 0` short-circuit) keep it
/// tiny, but a future caller must not be able to turn this into an unbounded
/// allocation.
pub(super) fn rescale(d: &Decimal, exp: i64, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_special() {
        return Ok(d.clone());
    }
    if d.coeff_is_zero() {
        return Ok(Decimal::from_triple(d.sign, BigInt::ZERO, exp));
    }

    if d.exp >= exp {
        // Pad with zeros: coeff · 10^(d.exp − exp).
        let pad = usize::try_from(d.exp - exp).map_err(|_| rescale_pad_error())?;
        if d.digits() + pad > DECIMAL_MAX_DIGITS {
            return Err(rescale_pad_error());
        }
        let mut coeff = d.coeff_str();
        coeff.extend(repeat_n('0', pad));
        return Ok(Decimal::from_triple(d.sign, parse_digits(&coeff), exp));
    }

    // Too many digits; round and lose data. If `d.adjusted() < exp - 1`,
    // replace `d` by the sentinel `1E(exp-1)` before rounding.
    let len = i64::try_from(d.digits()).expect("digit count fits i64");
    let (int_str, digits) = if len + d.exp - exp < 0 {
        ("1".to_owned(), 0)
    } else {
        (
            d.coeff_str(),
            usize::try_from(len + d.exp - exp).expect("non-negative cut fits usize"),
        )
    };
    let changed = round_decision(&int_str, digits, d.sign, rounding);
    let coeff = if digits == 0 { "0" } else { &int_str[..digits] };
    let coeff = if changed == 1 {
        parse_digits(coeff) + 1u8
    } else {
        parse_digits(coeff)
    };
    Ok(Decimal::from_triple(d.sign, coeff, exp))
}

/// The guard error for an out-of-bounds `rescale` pad. Unreachable through the
/// current callers (each pre-checks); an internal error rather than a Python
/// exception because reaching it means a caller lost its bound.
fn rescale_pad_error() -> RunError {
    RunError::internal("decimal rescale pad out of bounds")
}

/// `_pydecimal._round`: rounds a nonzero, non-special `d` to `places`
/// significant figures (quiet, context-free). `places >= 1`.
pub(super) fn round_sig(d: &Decimal, places: usize, rounding: RoundMode) -> RunResult<Decimal> {
    debug_assert!(places >= 1, "_round requires places >= 1");
    if d.is_special() || d.coeff_is_zero() {
        return Ok(d.clone());
    }
    let places_i = i64::try_from(places).expect("places fits i64");
    let ans = rescale(d, d.adjusted() + 1 - places_i, rounding)?;
    // The rescale's carry can grow the adjusted exponent (99.97 → 100.0 at 3
    // sig figs), leaving an extra trailing zero; a second rescale sheds it.
    if ans.adjusted() == d.adjusted() {
        Ok(ans)
    } else {
        rescale(&ans, ans.adjusted() + 1 - places_i, rounding)
    }
}

/// The eight `ROUND_*` decision functions, keyed by `mode`, exactly as
/// `_pydecimal` defines them. `int_str` is the coefficient digit string of a
/// finite nonzero value, `cut` the number of digits kept (`0 <= cut <
/// int_str.len()`). Returns `1` (round away from zero), `0` (dropped digits
/// are all zero — exact), or `-1` (nonzero digits dropped, truncate).
pub(super) fn round_decision(int_str: &str, cut: usize, sign: u8, mode: RoundMode) -> i8 {
    let digits = int_str.as_bytes();
    let round_down = || if all_zeros(digits, cut) { 0i8 } else { -1 };
    let round_half_up = || {
        if matches!(digits[cut], b'5'..=b'9') {
            1i8
        } else if all_zeros(digits, cut) {
            0
        } else {
            -1
        }
    };
    match mode {
        RoundMode::Down => round_down(),
        RoundMode::Up => -round_down(),
        RoundMode::HalfUp => round_half_up(),
        RoundMode::HalfDown => {
            if exact_half(digits, cut) {
                -1
            } else {
                round_half_up()
            }
        }
        RoundMode::HalfEven => {
            if exact_half(digits, cut) && (cut == 0 || matches!(digits[cut - 1], b'0' | b'2' | b'4' | b'6' | b'8')) {
                -1
            } else {
                round_half_up()
            }
        }
        RoundMode::Ceiling => {
            if sign == 1 {
                round_down()
            } else {
                -round_down()
            }
        }
        RoundMode::Floor => {
            if sign == 0 {
                round_down()
            } else {
                -round_down()
            }
        }
        RoundMode::Zero05Up => {
            if cut > 0 && !matches!(digits[cut - 1], b'0' | b'5') {
                round_down()
            } else {
                -round_down()
            }
        }
    }
}

/// `_pydecimal._all_zeros`: whether every digit from `cut` on is `'0'`.
fn all_zeros(digits: &[u8], cut: usize) -> bool {
    digits[cut..].iter().all(|&b| b == b'0')
}

/// `_pydecimal._exact_half`: whether the digits from `cut` are exactly
/// `5000…0`.
fn exact_half(digits: &[u8], cut: usize) -> bool {
    digits[cut] == b'5' && all_zeros(digits, cut + 1)
}

/// Parses an ASCII digit string (a coefficient slice) back into a `BigInt`.
fn parse_digits(s: &str) -> BigInt {
    BigInt::parse_bytes(s.as_bytes(), 10).expect("coefficient slice is ASCII digits")
}

/// Resolves a rounding-mode string (`"ROUND_HALF_UP"`, …) to its
/// [`RoundMode`] via the shared [`ROUNDING_MODES`] table, so the set of valid
/// modes lives in exactly one place. `None` for any other string.
pub(super) fn rounding_mode_from_str(s: &str) -> Option<RoundMode> {
    ROUNDING_MODES
        .into_iter()
        .find(|(name, _)| <&str>::from(*name) == s)
        .map(|(_, mode)| mode)
}

/// CPython's `TypeError` for an invalid per-call `rounding=` value (any
/// non-mode string *or* non-string raises the same message).
pub(super) fn invalid_rounding_error() -> RunError {
    SimpleException::new_msg(
        ExcType::TypeError,
        "valid values for rounding are:\n  [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,\n   \
         ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN,\n   ROUND_05UP]"
            .to_owned(),
    )
    .into()
}

impl Decimal {
    /// Whether the coefficient itself is zero — distinct from
    /// [`Decimal::is_zero`], which is false for specials: `fix`/`rescale` use
    /// this on values already known finite, and NaN payloads reuse it as
    /// "payload is empty".
    pub(super) fn coeff_is_zero(&self) -> bool {
        self.coeff.is_zero()
    }
}
