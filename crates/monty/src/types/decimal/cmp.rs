//! `Decimal` comparisons and hashing: the `_pydecimal._cmp` core (770-815),
//! the `_convert_for_comparison` numeric-tower dispatch (6015-6049), exact
//! `Decimal` ↔ `int` comparison at any magnitude, `float`/`int` conversion
//! helpers, and Monty's cross-type-consistent hash.
//!
//! NaN policy (from `_pydecimal`'s rich-comparison notes): `==`/`!=` treat a
//! quiet NaN as unequal/unordered but raise `InvalidOperation` for an sNaN;
//! `<`/`<=`/`>`/`>=` raise `InvalidOperation` for *any* NaN
//! (`_compare_check_nans`, 730-761) — the trap is always armed under the
//! fixed context.

use std::cmp::Ordering;

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::Zero;

use super::{Decimal, methods, parse, pow10_bounded};
use crate::{
    exception_private::{ExcType, RunResult},
    hash::{HashValue, hash_python_long_int},
    heap::{Heap, HeapData},
    resource::{ResourceTracker, check_pow_size},
    value::Value,
};

/// Ordering comparison (`<`, `<=`, `>`, `>=`) of a `Decimal` against another
/// `Value`, with CPython's NaN behaviour (`_compare_check_nans`, 730-761): an
/// ordering involving *any* NaN — quiet or signaling, on either side (a float
/// NaN converts to a quiet NaN first) — raises `InvalidOperation`, unlike
/// `==`/`!=` which return `False`/`True` for a quiet NaN.
///
/// Returns `Ok(None)` when `other` is not a number `Decimal` compares with;
/// the caller maps that to `CmpOrder::Incomparable`, which the VM's ordering
/// machinery raises as CPython's `TypeError: '<' not supported …`. `reversed`
/// flips the result for the operand order `other < Decimal`.
pub(crate) fn cmp_value(
    d: &Decimal,
    other: &Value,
    heap: &Heap<impl ResourceTracker>,
    reversed: bool,
) -> RunResult<Option<Ordering>> {
    match as_cmp_operand(other, heap) {
        None => Ok(None),
        Some(operand) if d.is_nan() || operand_is_nan(&operand) => Err(ExcType::decimal_invalid_operation()),
        Some(operand) => {
            let ordering = cmp_operand(d, operand);
            Ok(Some(if reversed { ordering.reverse() } else { ordering }))
        }
    }
}

/// Equality of a `Decimal` against another `Value` — the port of
/// `_pydecimal.__eq__` (837-843) over `_convert_for_comparison` (6015-6049).
///
/// `Ok(None)` (`NotImplemented`) for any non-number so `Value::py_eq` can try
/// the reflected comparison and finally fall back to unequal — matching how
/// `int`/`float` report cross-type equality. An sNaN on either side raises
/// `InvalidOperation` (`__eq__` goes through `_check_nans`, whose trap is
/// always armed here); a quiet NaN compares unequal to everything, itself
/// included. `int`s of any width compare exactly with no digit cap (see
/// [`cmp_decimal_int`]), and a `float` compares via its exact binary
/// expansion (`Decimal('0.1') != 0.1` but `Decimal('0.5') == 0.5`).
pub(super) fn eq_value(d: &Decimal, other: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<Option<bool>> {
    match as_cmp_operand(other, heap) {
        None => Ok(None),
        Some(operand) if d.is_snan() || matches!(operand, CmpOperand::Dec(o) if o.is_snan()) => {
            Err(ExcType::decimal_invalid_operation())
        }
        Some(operand) if d.is_nan() || operand_is_nan(&operand) => Ok(Some(false)),
        Some(operand) => Ok(Some(cmp_operand(d, operand) == Ordering::Equal)),
    }
}

/// A numeric operand that a `Decimal` can be compared against, extracted from
/// a `Value`. These are exactly the types CPython lets `Decimal` compare with
/// (`int`, `bool`, `float`, and another `Decimal`); everything else is not
/// comparable. `Copy` (an `i64`, an `f64`, or a borrow) so it can be passed
/// by value out of [`as_cmp_operand`].
#[derive(Clone, Copy)]
pub(super) enum CmpOperand<'a> {
    Int(i64),
    Big(&'a BigInt),
    Float(f64),
    Dec(&'a Decimal),
}

/// Resolves a `Value` to a comparable numeric operand, or `None` if `Decimal`
/// does not compare with it (so `Decimal('1') == 'x'` is `False`, not an
/// error).
pub(super) fn as_cmp_operand<'a>(value: &'a Value, heap: &'a Heap<impl ResourceTracker>) -> Option<CmpOperand<'a>> {
    match value {
        Value::Bool(b) => Some(CmpOperand::Int(i64::from(*b))),
        Value::Int(i) => Some(CmpOperand::Int(*i)),
        Value::Float(f) => Some(CmpOperand::Float(*f)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::LongInt(li) => Some(CmpOperand::Big(li.inner())),
            HeapData::Decimal(d) => Some(CmpOperand::Dec(d)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether the comparison operand is a NaN of any kind. A float NaN counts:
/// CPython's `_convert_for_comparison` converts it via `Decimal.from_float`,
/// yielding a quiet NaN.
fn operand_is_nan(operand: &CmpOperand<'_>) -> bool {
    match operand {
        CmpOperand::Float(f) => f.is_nan(),
        CmpOperand::Dec(d) => d.is_nan(),
        CmpOperand::Int(_) | CmpOperand::Big(_) => false,
    }
}

/// Dispatches a NaN-free comparison to the right core — the moral equivalent
/// of `_pydecimal._convert_for_comparison` (6015-6049), except that `int`s go
/// through [`cmp_decimal_int`] (exact at any magnitude, no `Decimal`
/// conversion and hence no digit cap) and a `float` through its exact
/// [`parse::from_float`] binary expansion.
fn cmp_operand(d: &Decimal, operand: CmpOperand<'_>) -> Ordering {
    match operand {
        CmpOperand::Int(i) => cmp_decimal_int(d, &BigInt::from(i)),
        CmpOperand::Big(n) => cmp_decimal_int(d, n),
        CmpOperand::Float(f) => cmp_decimal(d, &parse::from_float(f)),
        CmpOperand::Dec(other) => cmp_decimal(d, other),
    }
}

/// Compares two non-NaN decimals — port of `_pydecimal._cmp` (770-815):
/// infinity/zero/sign shortcuts, then the `adjusted()` comparison, and only
/// when the adjusted exponents tie the zero-padded coefficient-string
/// comparison.
///
/// The padding is bounded: equal adjusted exponents mean
/// `len_a + exp_a == len_b + exp_b`, so each pad equals the digit-count
/// difference — at most [`super::DECIMAL_MAX_DIGITS`] + 1 — and differing
/// adjusted exponents short-circuit without padding at all.
fn cmp_decimal(a: &Decimal, b: &Decimal) -> Ordering {
    debug_assert!(!a.is_nan() && !b.is_nan(), "callers filter NaNs");
    if a.is_special() || b.is_special() {
        // Only infinities remain among specials: compare the ∞-codes
        // (-1 / 0 / 1 — equal codes, including inf vs same-signed inf, tie).
        return a.infinity_sign().cmp(&b.infinity_sign());
    }

    // Check for zeros; Decimal('0') == Decimal('-0').
    if a.is_zero() {
        return if b.is_zero() {
            Ordering::Equal
        } else if b.sign == 0 {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if b.is_zero() {
        return if a.sign == 0 { Ordering::Greater } else { Ordering::Less };
    }

    // If different signs, the negative one is less.
    if b.sign < a.sign {
        return Ordering::Less;
    }
    if a.sign < b.sign {
        return Ordering::Greater;
    }

    // Same nonzero sign: compare magnitudes, flipping for negatives.
    let a_adjusted = a.adjusted();
    let b_adjusted = b.adjusted();
    let magnitude = if a_adjusted == b_adjusted {
        // CPython pads the higher-exponent coefficient string with zeros
        // (`self._int + '0'*(self._exp - other._exp)`); scaling the
        // coefficient by the same power of ten compares identically without
        // building strings. Equal adjusted exponents mean the pad equals the
        // digit-count difference, ≤ [`super::DECIMAL_MAX_DIGITS`] + 1.
        match a.exp.cmp(&b.exp) {
            Ordering::Equal => a.coeff.cmp(&b.coeff),
            Ordering::Greater => {
                let pad = u64::try_from(a.exp - b.exp).expect("positive difference");
                (&a.coeff * pow10_bounded(pad)).cmp(&b.coeff)
            }
            Ordering::Less => {
                let pad = u64::try_from(b.exp - a.exp).expect("positive difference");
                a.coeff.cmp(&(&b.coeff * pow10_bounded(pad)))
            }
        }
    } else {
        a_adjusted.cmp(&b_adjusted)
    };
    if a.sign == 1 { magnitude.reverse() } else { magnitude }
}

/// Compares a non-NaN `Decimal` against an integer exactly and at any
/// magnitude by comparing the integer part as a `BigInt` — so neither a huge
/// `int` nor a huge-exponent `Decimal` needs to convert to the other's type
/// (and the constructor digit cap never applies to comparisons).
///
/// A cheap sign/magnitude pre-check ([`cmp_decimal_int_by_magnitude`])
/// settles most pairs without materialising the integer part; the exact
/// [`integer_part_with_fraction`] build only runs when the magnitudes
/// genuinely overlap.
fn cmp_decimal_int(d: &Decimal, n: &BigInt) -> Ordering {
    if d.is_infinite() {
        if d.sign == 1 { Ordering::Less } else { Ordering::Greater }
    } else if let Some(ordering) = cmp_decimal_int_by_magnitude(d, n) {
        ordering
    } else {
        // Magnitudes overlap: materialise the integer part for an exact
        // compare.
        let (int_part, has_fraction) = integer_part_with_fraction(d);
        match int_part.cmp(n) {
            // Integer parts equal: d's fractional part breaks the tie — a
            // positive d with a fraction exceeds its truncation, a negative
            // one falls below it.
            Ordering::Equal if has_fraction => {
                if d.sign == 1 {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            ordering => ordering,
        }
    }
}

/// Cheaply resolves [`cmp_decimal_int`] from sign and integer-digit count
/// alone, returning `None` only when the magnitudes overlap and an exact
/// comparison is required. This is the DoS guard: it lets a tiny
/// `Decimal('1E+999999')` be compared against an `int` without ever
/// materialising the ~million-digit integer part, which would otherwise burn
/// CPU and memory on every comparison. `d` must be finite (NaN/∞ are handled
/// by the caller).
///
/// The `int`'s digit count comes from `n.bits()` (`digits ≈ bits · log10 2`,
/// computed in u128 with an 11-decimal-place under-approximation of
/// `log10 2`, so the true count is `estimate` or `estimate + 1` for any int
/// below ~10^11 bits); the exact-but-O(len²) `to_string` fallback only runs
/// when the estimate window actually straddles `d`'s digit count.
fn cmp_decimal_int_by_magnitude(d: &Decimal, n: &BigInt) -> Option<Ordering> {
    let d_sign: i8 = if d.is_zero() {
        0
    } else if d.sign == 1 {
        -1
    } else {
        1
    };
    let n_sign: i8 = match n.sign() {
        Sign::Minus => -1,
        Sign::NoSign => 0,
        Sign::Plus => 1,
    };
    if d_sign != n_sign {
        // Opposite signs (or one operand is zero): the larger sign is the
        // larger value, so comparing the signs themselves settles it.
        Some(d_sign.cmp(&n_sign))
    } else if d_sign == 0 {
        Some(Ordering::Equal)
    } else {
        // Same nonzero sign: a k-digit integer lies in `[10^(k-1), 10^k)`, so
        // a differing integer-digit count settles the magnitude outright.
        // `adjusted` is cheap (coefficient length + exponent); `|d| < 1` has
        // zero integer digits and is thus smaller in magnitude than any
        // nonzero int.
        let int_digits_d = match d.adjusted() {
            adj if adj >= 0 => u64::try_from(adj).expect("non-negative adjusted fits u64") + 1,
            _ => 0,
        };
        let estimate = u64::try_from(u128::from(n.bits()) * 30_102_999_566 / 100_000_000_000)
            .expect("digit estimate of an in-memory int fits u64");
        let n_digits = if int_digits_d < estimate || int_digits_d > estimate + 1 {
            // The `[estimate, estimate + 1]` window misses `d`'s count, so
            // the estimate is exact enough to order by.
            estimate
        } else {
            u64::try_from(n.magnitude().to_string().len()).expect("digit count fits u64")
        };
        match int_digits_d.cmp(&n_digits) {
            // Equal digit counts → magnitudes overlap; caller compares
            // exactly.
            Ordering::Equal => None,
            // For negatives, the larger magnitude is the smaller value.
            magnitude => Some(if d_sign < 0 { magnitude.reverse() } else { magnitude }),
        }
    }
}

/// The signed integer part of a finite decimal (truncated toward zero) plus
/// whether any nonzero fractional digits were discarded — the exact-compare
/// step of [`cmp_decimal_int`].
///
/// Bounded: only called when the operands' integer-digit counts match, so a
/// positive `exp` is at most the `int`'s (already materialised) digit count,
/// and a negative one is below the coefficient's digit count
/// (≤ [`super::DECIMAL_MAX_DIGITS`] + 1).
fn integer_part_with_fraction(d: &Decimal) -> (BigInt, bool) {
    let (magnitude, has_fraction) = if d.exp >= 0 {
        let exp = u64::try_from(d.exp).expect("non-negative exponent fits u64");
        (&d.coeff * pow10_bounded(exp), false)
    } else {
        let (quotient, remainder) = d.coeff.div_rem(&pow10_bounded(d.exp.unsigned_abs()));
        (quotient, !remainder.is_zero())
    };
    (if d.sign == 1 { -magnitude } else { magnitude }, has_fraction)
}

/// Computes the Monty-consistent hash of a `Decimal` so equal numbers hash
/// equally across types (`hash(Decimal(5)) == hash(5)`,
/// `hash(Decimal('1.5')) == hash(1.5)`).
///
/// This mirrors **Monty's** runtime hash scheme, NOT CPython's
/// `_PyHASH_MODULUS`: integral values hash as the integer (via
/// [`hash_python_long_int`], which matches both `int` and `LongInt`);
/// non-integral and infinite values hash as the `f64` bit pattern (matching
/// Monty's `float` hash); a quiet NaN hashes as `f64::NAN`'s bits and never
/// raises, while a signaling NaN raises CPython's exact
/// `TypeError: Cannot hash a signaling NaN value`.
pub(super) fn hash_decimal(d: &Decimal, tracker: &impl ResourceTracker) -> RunResult<HashValue> {
    if d.is_snan() {
        Err(ExcType::decimal_snan_hash())
    } else if d.is_qnan() {
        Ok(HashValue::new(f64::NAN.to_bits()))
    } else if d.is_infinite() {
        let inf = if d.sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY };
        Ok(HashValue::new(inf.to_bits()))
    } else if is_integral(d) {
        Ok(hash_python_long_int(&integral_to_bigint(d, tracker)?))
    } else {
        // Finite non-integral: hash as the nearest f64's bits, exactly like a
        // `float` value (the specials are pre-checked above, so
        // `methods::to_float` cannot fail here).
        Ok(HashValue::new(methods::to_float(d)?.to_bits()))
    }
}

/// Whether a finite decimal's value is an exact integer: a non-negative
/// exponent, a zero coefficient, or every dropped digit zero (`1.00` is
/// integral, `1.5` is not).
///
/// The divisibility check is bounded: it only runs when the fractional digit
/// count is below the coefficient's digit count
/// (≤ [`super::DECIMAL_MAX_DIGITS`] + 1); more fractional digits than
/// coefficient digits means a nonzero magnitude below `1`, never integral.
fn is_integral(d: &Decimal) -> bool {
    if d.exp >= 0 || d.coeff_is_zero() {
        true
    } else {
        let frac_digits = d.exp.unsigned_abs();
        frac_digits < digits_u64(d) && (&d.coeff % pow10_bounded(frac_digits)).is_zero()
    }
}

/// The exact integer value of an integral finite `Decimal` as a `BigInt`
/// (truncating toward zero for a non-integral input — `int(Decimal('1.5'))`
/// is `1`). Shared by hashing (to match Python's `int` hash) and the
/// `int()`/`round()` conversions in `methods.rs`.
///
/// The `10**exp` factor for a positive exponent derives from the value's
/// (constructor-bounded, up to ~10^18) exponent, so it is pre-checked with
/// [`check_pow_size`] (base 10 has 4 significant bits):
/// `int(Decimal('1E+999999999999'))` raises a `ResourceError` under limits
/// instead of attempting an unbounded allocation. The negative-exponent side
/// divides by `10**k` with `k` capped at the coefficient's digit count (a
/// larger `k` truncates to zero outright, without materialising the power).
pub(super) fn integral_to_bigint(d: &Decimal, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    let magnitude = if d.coeff_is_zero() {
        // Any zero — even one at a huge exponent — is exactly 0; never
        // materialise its 10**exp scale factor.
        BigInt::ZERO
    } else if d.exp >= 0 {
        let exp = u64::try_from(d.exp).expect("non-negative exponent fits u64");
        check_pow_size(4, exp, tracker)?;
        &d.coeff * pow10_bounded(exp)
    } else {
        let frac_digits = d.exp.unsigned_abs();
        if frac_digits >= digits_u64(d) {
            BigInt::ZERO // |d| < 1 truncates to zero
        } else {
            &d.coeff / pow10_bounded(frac_digits)
        }
    };
    Ok(if d.sign == 1 { -magnitude } else { magnitude })
}

/// The coefficient digit count as a `u64` (`1` for a zero coefficient).
fn digits_u64(d: &Decimal) -> u64 {
    u64::try_from(d.digits()).expect("digit count fits u64")
}
