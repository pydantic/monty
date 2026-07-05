//! `Decimal ** Decimal` and `pow(Decimal, Decimal, Decimal)` — ports of
//! `_pydecimal.__pow__` (lines 2252–2466), `_power_exact` (2004–2250) and
//! `_power_modulo` (1919–2002), under the fixed context (`prec = 28`,
//! `ROUND_HALF_EVEN`, `Emax = 999999`, `clamp = 0`).
//!
//! The two-argument power first resolves the special cases (NaNs, `x ** 0`,
//! zeros, infinities, `1 ** y`, the crude overflow/underflow bound), then
//! attempts an *exact* result via [`power_exact`], and only falls back to the
//! correctly-rounded `exp(y·log(x))` kernel ([`trans::dpower`]) when
//! exactness is ruled out. Every `len(str(...))` early-exit and `emax` cap in
//! `_power_exact` is a load-bearing DoS guard bounding the `10**e` / `5**e` /
//! `2**e` / `xc**m` materialisations; [`pow10`] and
//! [`check_pow_size`] re-check as defence in depth (guard 2 in the module
//! docs), and the Newton nth-root / roundability-refinement / modular-power
//! loops poll `check_time` (guard 3).

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Pow, ToPrimitive, Zero};

use super::{
    DEFAULT_PREC, Decimal, EMAX, ETINY, PREC, RoundMode, allocate, check_nans, fix, magnitude_digits, pow10, trans,
};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, RunResult},
    resource::{ResourceTracker, check_pow_size},
    value::Value,
};

/// `Decimal ** Decimal` — the two-argument `__pow__` (`_pydecimal`
/// 2252–2466, `modulo=None` path), dispatched from `arith`'s `BinOp::Pow`.
///
/// Special cases return directly (CPython does not `_fix` them); everything
/// past the "from here on" comment (2365) is finalised by [`fix::fix`]. Under
/// the fixed context the only trapped signals are `InvalidOperation` (sNaN
/// operands, `0 ** 0`, a negative base with a non-integral exponent) and
/// `Overflow`; the `Inexact`/`Rounded`/`Underflow`/`Subnormal` raises in the
/// original are untrapped no-ops and are omitted.
pub(super) fn power(a: Decimal, b: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    // Either argument is a NaN => the result is a NaN (2286-2289). This comes
    // BEFORE the `x ** 0` check: `Decimal('NaN') ** 0` is NaN, not 1.
    if let Some(nan) = check_nans(&a, Some(b))? {
        return allocate(nan, vm);
    }

    // 0**0 raises InvalidOperation(!); x**0 = 1 for any other x, including
    // ±Infinity (2291-2296).
    if b.is_zero() {
        return if a.is_zero() {
            Err(ExcType::decimal_invalid_operation())
        } else {
            allocate(Decimal::from_i64(1), vm)
        };
    }

    // The result is negative iff the base is negative and the exponent an odd
    // integer; a nonzero negative base with a non-integral exponent is
    // invalid, and `(-0) ** noninteger` falls through as `0 ** noninteger`
    // (2298-2311). The base is then made positive for the computation.
    let mut result_sign = 0u8;
    let a = if a.sign == 1 {
        if is_integer(b) {
            if !is_even(b) {
                result_sign = 1;
            }
        } else if !a.is_zero() {
            // -ve ** noninteger = NaN ('x ** y with x negative and y not an
            // integer' in _pydecimal; the C module's fixed message here).
            return Err(ExcType::decimal_invalid_operation());
        }
        a.copy_negate()
    } else {
        a
    };

    // 0**(+ve or Inf) = 0; 0**(-ve or -Inf) = Infinity (2313-2318).
    if a.is_zero() {
        return allocate(
            if b.sign == 0 {
                Decimal::from_triple(result_sign, BigInt::ZERO, 0)
            } else {
                Decimal::infinity(result_sign)
            },
            vm,
        );
    }

    // Inf**(+ve or Inf) = Inf; Inf**(-ve or -Inf) = 0 (2320-2325).
    if a.is_infinite() {
        return allocate(
            if b.sign == 0 {
                Decimal::infinity(result_sign)
            } else {
                Decimal::from_triple(result_sign, BigInt::ZERO, 0)
            },
            vm,
        );
    }

    // 1**other = 1, with the exponent CPython prescribes (2327-2352). This
    // deliberately precedes the b-infinite case: `Decimal(1) ** Decimal('Inf')`
    // is `1.000...0` at full precision. The multiplier is int(b) clamped into
    // [0, prec], evaluated without materialising a huge int(b) (b could be
    // 1E999999999); the untrapped Inexact/Rounded raises are omitted.
    if is_numerically_one(&a) {
        let exp = if is_integer(b) {
            let multiplier = if b.sign == 1 {
                0
            } else if b.adjusted() >= 2 {
                // b >= 100 > prec, settled without computing int(b).
                PREC
            } else {
                // b < 100: int(b) is tiny; clamp at prec.
                integral_magnitude(b, vm.heap.tracker())?
                    .to_i64()
                    .expect("integer below 100 fits i64")
                    .min(PREC)
            };
            // a == 1 has a.exp <= 0, so the product is <= 0.
            a.exp.saturating_mul(multiplier).max(1 - PREC)
        } else {
            1 - PREC
        };
        // '1' + '0'*-exp with exp in [1-prec, 0] (2352).
        let coeff = pow10(-exp, vm.heap.tracker())?;
        return allocate(Decimal::from_triple(result_sign, coeff, exp), vm);
    }

    // Adjusted exponent of the (now positive, non-1) base (2354-2355).
    let self_adj = a.adjusted();

    // a ** ±Infinity resolves by whether a is above or below 1 (2357-2363).
    if b.is_infinite() {
        return allocate(
            if (b.sign == 0) == (self_adj < 0) {
                Decimal::from_triple(result_sign, BigInt::ZERO, 0)
            } else {
                Decimal::infinity(result_sign)
            },
            vm,
        );
    }

    // From here on the result always goes through `fix` (2365-2368).
    let mut exact = false;

    // Crude bound catching extreme overflow/underflow WITHOUT computing the
    // power (2370-2386): if log10(a)·b is at least 10**len(str(Emax)) the
    // result is past Emax (or below Etiny, in the mirrored case), so a
    // sentinel value lets `fix` do the raising (Overflow) or the quiet
    // rounding to a signed zero (underflow, untrapped).
    let bound = trans::log10_exp_bound(&a, vm.heap.tracker())?.saturating_add(b.adjusted());
    let mut ans = if (self_adj >= 0) == (b.sign == 0) {
        // a > 1 with b positive, or a < 1 with b negative: possible overflow.
        (bound >= dec_len(EMAX)).then(|| Decimal::from_triple(result_sign, BigInt::from(1), EMAX + 1))
    } else {
        // a > 1 with b negative, or a < 1 with b positive: possible underflow.
        (bound >= dec_len(-ETINY)).then(|| Decimal::from_triple(result_sign, BigInt::from(1), ETINY - 1))
    };

    // Try for an exact result with precision + 1 (2388-2394).
    if ans.is_none() {
        ans = power_exact(&a, b, PREC + 1, vm.heap.tracker())?;
        if let Some(exact_ans) = ans.as_mut() {
            exact_ans.sign = result_sign;
            exact = true;
        }
    }

    // Usual case: inexact result, x**y computed as exp(y·log(x)) (2396-2415).
    let ans = if let Some(ans) = ans {
        ans
    } else {
        // _WorkRep values: unlike _power_exact these keep any trailing
        // zeros (2399-2404).
        let xc = a.coeff;
        let xe = a.exp;
        let mut yc = b.coeff.clone();
        let ye = b.exp;
        if b.sign == 1 {
            yc = -yc;
        }
        // Start at precision + 3, widening until the result is
        // unambiguously roundable — i.e. the digits past the precision are
        // not an exact `...5000`/`...0000` tail (2406-2414). The iteration
        // cap and per-pass time poll are sandbox guards (guard 3);
        // CPython's loop terminates on the same condition but unboundedly.
        let mut extra: i64 = 3;
        loop {
            vm.heap.check_time()?;
            let (coeff, exp) = trans::dpower(&xc, xe, &yc, ye, PREC + extra, vm.heap.tracker())?;
            // coeff has p+extra or p+extra+1 digits (dpower's contract),
            // so the cut is at least extra - 1 >= 2.
            let cut = i64::try_from(coeff.to_string().len()).expect("digit count fits i64") - PREC - 1;
            let modulus = BigInt::from(5) * pow10(cut, vm.heap.tracker())?;
            if !(&coeff % &modulus).is_zero() {
                break Decimal::from_triple(result_sign, coeff, exp);
            }
            extra += 3;
            if extra > 301 {
                return Err(RunError::internal("decimal pow did not converge"));
            }
        }
    };

    // The power function respects the context rounding mode (2417-2418) —
    // already ROUND_HALF_EVEN under the fixed context, nothing to switch.
    //
    // Exact result with a non-integer exponent (2430-2461): CPython pads the
    // coefficient to prec+1 digits, `_fix`es in a traps-cleared copy of the
    // context, then re-raises the trapped signals from the original. Under
    // the fixed context that dance collapses: Inexact/Underflow/Subnormal/
    // Rounded/Clamped are untrapped no-ops, so the only surviving effect is
    // that a result past Emax still raises Overflow (2457-2458) — which the
    // plain `fix` below already does.
    let ans = if exact && !is_integer(b) && ans.digits() <= DEFAULT_PREC {
        let expdiff = PREC + 1 - i64::try_from(ans.digits()).expect("digit count fits i64");
        Decimal::from_triple(
            ans.sign,
            ans.coeff * pow10(expdiff, vm.heap.tracker())?,
            ans.exp - expdiff,
        )
    } else {
        ans
    };
    allocate(fix::fix(ans, RoundMode::HalfEven)?, vm)
}

/// Three-argument `pow(Decimal, Decimal, Decimal)` — `_pydecimal._power_modulo`
/// (1919–2002), exported for the `pow()` builtin.
///
/// The restrictions of Python's integer `pow` apply, each raising
/// `InvalidOperation` — with the C module's fixed
/// `[<class 'decimal.InvalidOperation'>]` message rather than `_pydecimal`'s
/// prose ("pow() 3rd argument not allowed unless all arguments are integers",
/// etc.): all three operands integral (which also rejects infinities), the
/// exponent non-negative, the modulus nonzero with at most `prec` digits, and
/// not `0 ** 0`. The result is exact modular exponentiation and — like
/// CPython — is returned without `_fix`.
pub(crate) fn power_modulo(
    a: Decimal,
    b: Decimal,
    m: Decimal,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    // NaN handling matches fma: any sNaN raises, otherwise the first quiet
    // NaN in argument order propagates (1932-1951).
    if a.is_snan() || b.is_snan() || m.is_snan() {
        return Err(ExcType::decimal_invalid_operation());
    }
    if a.is_qnan() {
        return allocate(fix::fix_nan(a), vm);
    }
    if b.is_qnan() {
        return allocate(fix::fix_nan(b), vm);
    }
    if m.is_qnan() {
        return allocate(fix::fix_nan(m), vm);
    }

    // Same restrictions as Python's pow() (1953-1959).
    if !(is_integer(&a) && is_integer(&b) && is_integer(&m)) {
        return Err(ExcType::decimal_invalid_operation());
    }
    // The exponent cannot be negative (1960-1963); `-0` is not negative.
    if b.sign == 1 && !b.is_zero() {
        return Err(ExcType::decimal_invalid_operation());
    }
    // The modulus cannot be zero (1964-1966).
    if m.is_zero() {
        return Err(ExcType::decimal_invalid_operation());
    }
    // The modulus must be less than 10**prec in absolute value (1968-1974).
    if m.adjusted() >= PREC {
        return Err(ExcType::decimal_invalid_operation());
    }
    // 0**0 is undefined, for consistency with two-argument pow (1976-1982).
    if b.is_zero() && a.is_zero() {
        return Err(ExcType::decimal_invalid_operation());
    }

    // The result is negative only for a negative base with an odd exponent
    // (1984-1988); the modulus sign is ignored.
    let sign = if is_even(&b) { 0 } else { a.sign };

    // modulo = abs(int(modulo)) — at most prec digits by the check above
    // (1990-1992) — and base/exponent as integer WorkReps with exp >= 0.
    let modulo = integral_magnitude(&m, vm.heap.tracker())?;
    let (base_coeff, base_exp) = integral_workrep(&a)?;
    let (exp_coeff, exp_exp) = integral_workrep(&b)?;

    // base = (base.int % modulo * pow(10, base.exp, modulo)) % modulo (1997):
    // the power-of-ten factor is folded in modularly, so a huge base exponent
    // never materialises.
    let ten = BigInt::from(10);
    let mut base = (&base_coeff % &modulo) * ten.modpow(&BigInt::from(base_exp), &modulo) % &modulo;

    // for i in range(exponent.exp): base = pow(base, 10, modulo) (1998-1999).
    // `exponent.exp` is attacker-sized (`pow(x, Decimal('1E18'), m)` loops
    // that many times — in CPython too), so each cheap modular pass polls the
    // time limit (guard 3).
    for _ in 0..exp_exp {
        vm.heap.check_time()?;
        base = base.modpow(&ten, &modulo);
    }
    // base = pow(base, exponent.int, modulo) (2000).
    base = base.modpow(&exp_coeff, &modulo);

    allocate(Decimal::from_triple(sign, base, 0), vm)
}

/// `_pydecimal._power_exact` (2004–2250): attempts to compute `a ** b`
/// exactly with `p` digits of precision, returning `None` when the result is
/// not exactly representable (the caller then computes it inexactly).
///
/// Preconditions, established by [`power`]: both operands finite, `a`
/// positive and not numerically 1, `b` nonzero. The method is engineered to
/// detect *failure* cheaply: every `len(str(...))` early-exit and `emax` cap
/// is kept in CPython's order because together they bound all the big-integer
/// materialisations; `Ok(None)` on the (unreachable) out-of-`i64` exponent
/// paths falls back to the inexact kernel, whose `fix` overflows exactly
/// where CPython's would.
#[expect(clippy::many_single_char_names, reason = "ported line-for-line from _pydecimal")]
fn power_exact(a: &Decimal, b: &Decimal, p: i64, tracker: &impl ResourceTracker) -> RunResult<Option<Decimal>> {
    // WorkReps with powers of 10 shifted out of the coefficients (2062-2072):
    // x = xc·10^xe and |y| = yc·10^ye with xc, yc not divisible by 10.
    let mut xc = a.coeff.clone();
    let mut xe = a.exp;
    shift_out_tens(&mut xc, &mut xe);
    let mut yc = b.coeff.clone();
    let mut ye = b.exp;
    shift_out_tens(&mut yc, &mut ye);

    // Case xc == 1: the result is 10**(xe·y), with xe·y required to be an
    // integer (2074-2093).
    if xc.is_one() {
        // xe *= yc, then shift the 10s into ye; nonzero because a != 1 forces
        // xe != 0 here (2077-2081). BigInt: xe·yc can exceed i64.
        let mut xe = BigInt::from(xe) * &yc;
        shift_out_tens(&mut xe, &mut ye);
        if ye < 0 {
            return Ok(None);
        }
        // exponent = xe·10^ye (2084-2086), bounded to ~10**len(str(Emax)) by
        // __pow__'s overflow/underflow shortcut; `pow10` re-checks (guard 2).
        let mut exponent = xe * pow10(ye, tracker)?;
        if b.sign == 1 {
            exponent = -exponent;
        }
        // For a non-negative integer b, shed zeros toward the ideal exponent
        // self._exp·int(b) (2087-2092); `exponent - ideal` is non-negative
        // because the shifted-out 10s only ever raised xe above a.exp.
        let zeros = if is_integer(b) && b.sign == 0 {
            let ideal = BigInt::from(a.exp) * integral_magnitude(b, tracker)?;
            (&exponent - ideal)
                .min(BigInt::from(p - 1))
                .to_i64()
                .ok_or_else(|| RunError::internal("decimal pow: ideal-exponent shift out of bounds"))?
        } else {
            0
        };
        // '1' + '0'*zeros at exponent - zeros (2093).
        return Ok(match (exponent - BigInt::from(zeros)).to_i64() {
            Some(exp) => Some(Decimal::from_triple(0, pow10(zeros, tracker)?, exp)),
            // Unreachable (the shortcut bounds |log10(x)·y|): bail to inexact.
            None => None,
        });
    }

    let ten = BigInt::from(10);

    // Case y negative: xc must be a power of 2 or a power of 5 (2095-2183).
    if b.sign == 1 {
        let last_digit = (&xc % &ten).to_u8().expect("remainder below 10 fits u8");
        // `e` with xc = 2**e (resp. 5**e); the result coefficient will be
        // `coeff_base**e` = 5**e (resp. 2**e).
        let (e, emax, coeff_base): (i64, i64, u8) = match last_digit {
            2 | 4 | 6 | 8 => {
                // Quick power-of-2 test — CPython's `xc & -xc == xc` (2100-2104).
                if xc.magnitude().count_ones() != 1 {
                    return Ok(None);
                }
                // The exact result is 5**(-e·y) · 10**(e·y + xe·y). `emax` is
                // the largest e with 5**e < 10**p (93/65 bounds
                // log(10)/log(5)); a ye at which -e·y necessarily exceeds it
                // exits before anything is materialised (2106-2133).
                let emax = p * 93 / 65;
                if ye >= dec_len(emax) {
                    return Ok(None);
                }
                (i64::try_from(xc.bits()).expect("bit count fits i64") - 1, emax, 5)
            }
            5 => {
                // e >= log5(xc) whenever xc is a power of 5 (2145-2154); e is
                // bounded by the digit cap on xc, `check_pow_size` as defence.
                let mut e = i64::try_from(xc.bits()).expect("bit count fits i64") * 28 / 65;
                check_pow_size(3, u64::try_from(e).expect("non-negative"), tracker)?;
                let (q, r) = BigInt::from(5)
                    .pow(u32::try_from(e).expect("bounded by coefficient bits"))
                    .div_rem(&xc);
                if !r.is_zero() {
                    return Ok(None);
                }
                xc = q;
                let five = BigInt::from(5);
                loop {
                    let (q, r) = xc.div_rem(&five);
                    if r.is_zero() {
                        xc = q;
                        e -= 1;
                    } else {
                        break;
                    }
                }
                // Same ye guard, with 10/3 bounding log(10)/log(2) (2156-2161).
                let emax = p * 10 / 3;
                if ye >= dec_len(emax) {
                    return Ok(None);
                }
                (e, emax, 2)
            }
            // Odd last digit other than 5: not a power of 2 or 5 (2171-2172).
            _ => return Ok(None),
        };

        // -e·y and -xe·y must both be integers (2135-2139 / 2163-2166), and
        // the coefficient exponent may not exceed emax (2141 / 2168).
        let Some(e_big) = trans::decimal_lshift_exact(&(BigInt::from(e) * &yc), ye) else {
            return Ok(None);
        };
        let Some(xe_big) = trans::decimal_lshift_exact(&(BigInt::from(xe) * &yc), ye) else {
            return Ok(None);
        };
        if e_big > BigInt::from(emax) {
            return Ok(None);
        }
        let exponent = e_big.to_u32().expect("0 < e <= emax");
        let xc = BigInt::from(coeff_base).pow(exponent);
        // An exact power of 10 is impossible here (asserts at 2177-2178); a
        // coefficient wider than p digits is not representable (2179-2181).
        if i64::try_from(xc.to_string().len()).expect("digit count fits i64") > p {
            return Ok(None);
        }
        // xe = -e - xe (2182-2183); an out-of-i64 exponent is unreachable
        // (the __pow__ shortcut bounds |log10(x)·y|) — bail to inexact.
        return Ok((-e_big - xe_big).to_i64().map(|exp| Decimal::from_triple(0, xc, exp)));
    }

    // Now y is positive: write y = m/n in lowest terms (2185-2200).
    let (m, n) = if ye >= 0 {
        (&yc * pow10(ye, tracker)?, BigInt::from(1))
    } else {
        // |y| < 1/|xe| or |y| <= 1/nbits(xc) means x**y cannot be exactly
        // representable (2187-2193); these two exits also bound the
        // 10**(-ye) denominator materialised just below.
        if xe != 0 && magnitude_digits(&(&yc * BigInt::from(xe))) <= -ye {
            return Ok(None);
        }
        let xc_bits = i64::try_from(xc.bits()).expect("bit count fits i64");
        if magnitude_digits(&(&yc * BigInt::from(xc_bits))) <= -ye {
            return Ok(None);
        }
        let mut m = yc.clone();
        let mut n = pow10(-ye, tracker)?;
        // Cancel common factors of 2 and 5 (2195-2200).
        for factor in [2u8, 5] {
            let factor = BigInt::from(factor);
            loop {
                let (mq, mr) = m.div_rem(&factor);
                let (nq, nr) = n.div_rem(&factor);
                if mr.is_zero() && nr.is_zero() {
                    m = mq;
                    n = nq;
                } else {
                    break;
                }
            }
        }
        (m, n)
    };

    // Compute the nth root of xc·10^xe when n > 1 (2202-2222).
    if n > BigInt::from(1) {
        // 1 < xc < 2**n cannot be an nth power (2204-2206) — this comparison
        // is also what shrinks n from a potentially huge 10**(-ye) down to
        // less than the coefficient's bit count.
        let xc_bits = i64::try_from(xc.bits()).expect("bit count fits i64");
        if BigInt::from(xc_bits) <= n {
            return Ok(None);
        }
        let n = n.to_i64().expect("n below xc bit count fits i64");
        // xe must be divisible by n (2208-2210); Python divmod is floored.
        let (quot, rem) = xe.div_mod_floor(&n);
        if rem != 0 {
            return Ok(None);
        }
        xe = quot;
        // Newton's method from above (2212-2222): the initial estimate
        // 2**ceil(bits/n) is >= the true root, so the iterate decreases
        // monotonically; the per-pass time poll is guard 3.
        let mut root =
            BigInt::from(1) << u64::try_from(num_integer::Integer::div_ceil(&xc_bits, &n)).expect("positive shift");
        let root_exp = u32::try_from(n - 1).expect("n bounded by coefficient bit count");
        let (root, quot, rem) = loop {
            tracker.check_time()?;
            let (q, r) = xc.div_rem(&Pow::pow(&root, root_exp));
            if root <= q {
                break (root, q, r);
            }
            root = (&root * BigInt::from(n - 1) + q) / BigInt::from(n);
        };
        if !(root == quot && rem.is_zero()) {
            return Ok(None);
        }
        xc = root;
    }

    // Compute the mth power of the root (2224-2240). The `_log10_lb` bound is
    // the load-bearing guard: past it xc**m > 10**p, so an unrepresentable
    // power is never materialised (xc > 1 always holds here — kept as CPython
    // wrote it, and it also shields `log10_lb` from a zero denominator).
    if xc > BigInt::from(1) && m > BigInt::from(p * 100 / trans::log10_lb(&xc)) {
        return Ok(None);
    }
    let m_small = m
        .to_u32()
        .ok_or_else(|| RunError::internal("decimal pow: unbounded exact exponent"))?;
    check_pow_size(xc.bits(), u64::from(m_small), tracker)?;
    xc = xc.pow(m_small);
    // Unreachable overflow (the __pow__ shortcut bounds |xe·m|): bail to inexact.
    let Ok(xe) = i64::try_from(i128::from(xe) * i128::from(m_small)) else {
        return Ok(None);
    };
    let str_xc = xc.to_string();
    if i64::try_from(str_xc.len()).expect("digit count fits i64") > p {
        return Ok(None);
    }

    // The result is exactly representable: shed zeros toward the ideal
    // exponent self._exp·int(b) (2242-2250). For a (positive) integer b the
    // reduction left n == 1 and m == int(b) exactly, so m is reused here.
    let zeros = if is_integer(b) && b.sign == 0 {
        let ideal = BigInt::from(a.exp) * &m;
        let cap = p - i64::try_from(str_xc.len()).expect("digit count fits i64");
        (BigInt::from(xe) - ideal)
            .min(BigInt::from(cap))
            .to_i64()
            .ok_or_else(|| RunError::internal("decimal pow: ideal-exponent shift out of bounds"))?
    } else {
        0
    };
    Ok(Some(Decimal::from_triple(0, xc * pow10(zeros, tracker)?, xe - zeros)))
}

/// `abs(int(d))` for a finite integral `d` — exact by construction (a
/// negative `d.exp` on an integral value only strips trailing zeros). Serves
/// `_power_modulo`'s `abs(int(modulo))` and the ideal-exponent `int(other)`
/// computations, whose call sites all bound `d` first; [`pow10`] re-checks.
fn integral_magnitude(d: &Decimal, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    debug_assert!(is_integer(d), "integral_magnitude requires an integral operand");
    Ok(if d.exp >= 0 {
        &d.coeff * pow10(d.exp, tracker)?
    } else {
        &d.coeff / pow10(-d.exp, tracker)?
    })
}

/// `_WorkRep(d.to_integral_value())` (1993–1994): the `(coeff, exp)` pair of
/// an integral `d` with the exponent forced non-negative. The rescale is
/// exact — `_power_modulo` verified integrality — so the default rounding
/// mode never actually rounds.
fn integral_workrep(d: &Decimal) -> RunResult<(BigInt, i64)> {
    if d.exp >= 0 {
        Ok((d.coeff.clone(), d.exp))
    } else {
        let rescaled = fix::rescale(d, 0, RoundMode::HalfEven)?;
        Ok((rescaled.coeff, rescaled.exp))
    }
}

/// `Decimal._isinteger` (2856–2863): finite with no nonzero fractional digit
/// (so `Decimal('1.00')` and `Decimal('1E5')` are integers, specials never).
fn is_integer(d: &Decimal) -> bool {
    if d.is_special() {
        false
    } else if d.exp >= 0 {
        true
    } else {
        // CPython's `self._int[self._exp:]`: the last -exp coefficient digits
        // (all of them when -exp exceeds the digit count) must be zeros.
        let s = d.coeff_str();
        let frac = usize::try_from(-d.exp).unwrap_or(usize::MAX).min(s.len());
        s.as_bytes()[s.len() - frac..].iter().all(|&b| b == b'0')
    }
}

/// `Decimal._iseven` (2865–2869) — assumes `d` is a finite integer: zero and
/// positive-exponent values (trailing base-10 zeros) are even; otherwise the
/// last integral-position digit decides.
fn is_even(d: &Decimal) -> bool {
    if d.is_zero() || d.exp > 0 {
        true
    } else {
        // CPython's `self._int[-1 + self._exp]`. A nonzero integer always has
        // more digits than trailing fractional zeros, so the index is valid;
        // the defensive `is_none_or` avoids a panic on a (contract-violating)
        // non-integer.
        let s = d.coeff_str();
        usize::try_from(-d.exp)
            .ok()
            .and_then(|shift| s.len().checked_sub(shift + 1))
            .is_none_or(|idx| matches!(s.as_bytes()[idx], b'0' | b'2' | b'4' | b'6' | b'8'))
    }
}

/// Whether `d` is numerically equal to 1 in any representation — CPython's
/// `self == _One` (2330): positive, coefficient `10**k`, exponent `-k`
/// (`1`, `1.0`, `1.00`, … all match; `10` does not).
fn is_numerically_one(d: &Decimal) -> bool {
    if !d.is_finite() || d.sign != 0 {
        return false;
    }
    let s = d.coeff_str();
    let bytes = s.as_bytes();
    bytes[0] == b'1'
        && bytes[1..].iter().all(|&b| b == b'0')
        && i64::try_from(s.len()).expect("digit count fits i64") == 1 - d.exp
}

/// Shifts factors of 10 out of `c` into `e` — the `while c % 10 == 0`
/// `_WorkRep` normalisation in `_power_exact` (2064–2072, 2079–2081). `c`
/// must be nonzero (guaranteed: the operands are nonzero) or the loop would
/// never terminate; iterations are bounded by `c`'s digit count.
fn shift_out_tens(c: &mut BigInt, e: &mut i64) {
    debug_assert!(!c.is_zero(), "shift_out_tens requires a nonzero value");
    let ten = BigInt::from(10);
    loop {
        let (q, r) = c.div_rem(&ten);
        if r.is_zero() {
            *c = q;
            *e += 1;
        } else {
            break;
        }
    }
}

/// `len(str(n))` for a non-negative `n` — CPython's digit-count idiom used by
/// the `_power_exact` guards and the `__pow__` overflow bound.
fn dec_len(n: i64) -> i64 {
    debug_assert!(n >= 0, "dec_len takes a non-negative value");
    i64::try_from(n.to_string().len()).expect("length fits i64")
}
