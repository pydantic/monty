//! `Decimal` arithmetic — line-for-line ports of `_pydecimal.__add__` /
//! `__sub__` / `__mul__` / `__truediv__` / `_divide` / `__divmod__` /
//! `__mod__` / `__floordiv__` and the unary `__neg__` / `__pos__` /
//! `__abs__`, plus the `_WorkRep` / `_normalize` helpers they share.
//!
//! Every operator runs under the fixed context (prec 28, `ROUND_HALF_EVEN`).
//! CPython code paths that consult `context.rounding` (e.g. `__add__`'s
//! `ROUND_FLOOR` negative-zero rule) are kept and coded against a passed
//! [`RoundMode`] for faithfulness, with the operator entry points supplying
//! [`RoundMode::HalfEven`]. Results are finalised by [`fix::fix`] exactly
//! where CPython calls `._fix(context)`; untrapped signals
//! (`Inexact`/`Rounded`/`Clamped`/`Subnormal`/`Underflow`) have their call
//! sites omitted entirely (see the signal table in the module docs).
//!
//! Sandbox bounds: every `10**k` materialised here has `k` bounded by the
//! operands' digit counts (≤ [`super::DECIMAL_MAX_DIGITS`] + 1 each) plus the
//! precision — each bound is documented at its call site — never by a raw
//! exponent an attacker controls.

use std::mem;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Zero};
use smallvec::smallvec;

use super::{
    DEFAULT_PREC, Decimal, ETINY, PREC, RoundMode, allocate, check_nans, fix, magnitude_digits, parse, pow,
    pow10_bounded,
};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    types::{Type, allocate_tuple},
    value::Value,
};

/// The binary arithmetic operators `Decimal` supports. Carried so the
/// zero-divisor and invalid-operation paths can pick the exact CPython
/// condition class.
#[derive(Clone, Copy)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

/// The Decimal dispatch probe for the VM's binary operators — the single
/// point where `Value`-level arithmetic detects a `Decimal` operand.
///
/// `Ok(None)` when neither operand is a heap `Decimal`, or when the *other*
/// operand is not a number `Decimal` operates with (a `float`, a `str`, …) —
/// in both cases the caller's remaining dispatch (and ultimately the VM's
/// generic `TypeError`) proceeds unchanged. Promotion runs *before* the
/// `Decimal` is cloned out of the heap, so an unsupported pairing
/// (`Decimal('1') + 1.5`) costs no coefficient allocation. Centralising the
/// probe also centralises the ordering invariant: callers invoke it before
/// any generic `Ref`-inspecting arm can capture a `Decimal` operand.
pub(crate) fn binary_op_value(
    lhs: &Value,
    rhs: &Value,
    op: BinOp,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let (id, other, swapped) = if let Value::Ref(id) = lhs
        && matches!(vm.heap.get(*id), HeapData::Decimal(_))
    {
        (*id, rhs, false)
    } else if let Value::Ref(id) = rhs
        && matches!(vm.heap.get(*id), HeapData::Decimal(_))
    {
        (*id, lhs, true)
    } else {
        return Ok(None);
    };
    let Some(operand) = promote(other, vm.heap)? else {
        // CPython special-cases sequence repetition: `'a' * Decimal(2)` (in
        // either order) raises "can't multiply sequence by non-int of type
        // 'decimal.Decimal'", not the generic unsupported-operands TypeError.
        if matches!(op, BinOp::Mul) && is_sequence(other, vm.heap) {
            return Err(ExcType::sequence_repeat_non_int(Type::Decimal));
        }
        return Ok(None);
    };
    // Just verified to be a Decimal above; `Ok(None)` is a defensive
    // fallthrough (the VM would raise its generic TypeError).
    let HeapData::Decimal(d) = vm.heap.get(id) else {
        return Ok(None);
    };
    let d = d.clone();
    let (a, b) = if swapped { (operand, d) } else { (d, operand) };
    compute(a, &b, op, vm).map(Some)
}

/// Computes `a <op> b` on promoted operands and allocates the result — the
/// core of [`binary_op_value`]. Every operator runs under the fixed
/// context's `ROUND_HALF_EVEN`.
fn compute(a: Decimal, b: &Decimal, op: BinOp, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let result = match op {
        BinOp::Add => add(&a, b, RoundMode::HalfEven)?,
        BinOp::Sub => sub(&a, b, RoundMode::HalfEven)?,
        BinOp::Mul => mul(&a, b, RoundMode::HalfEven)?,
        BinOp::Div => true_div(&a, b, RoundMode::HalfEven)?,
        BinOp::FloorDiv => floor_div(&a, b, RoundMode::HalfEven)?,
        BinOp::Mod => modulo(&a, b, RoundMode::HalfEven)?,
        BinOp::Pow => return pow::power(a, b, vm),
    };
    allocate(result, vm)
}

/// Whether a value is one of the sequence types whose `*` CPython reports with
/// the "can't multiply sequence by non-int" TypeError (`str`, `bytes`, `list`,
/// `tuple` — named tuples included; `range` has no repeat and keeps the
/// generic message).
fn is_sequence(value: &Value, heap: &Heap<impl ResourceTracker>) -> bool {
    match value {
        Value::InternString(_) | Value::InternBytes(_) => true,
        Value::Ref(id) => matches!(
            heap.get(*id),
            HeapData::Str(_) | HeapData::Bytes(_) | HeapData::List(_) | HeapData::Tuple(_) | HeapData::NamedTuple(_)
        ),
        _ => false,
    }
}

/// Promotes a `Value` to a `Decimal` arithmetic operand, or `Ok(None)` if it
/// is not a number `Decimal` operates with. `float` is deliberately excluded
/// (so `Decimal + float` is a `TypeError`, matching CPython). A heap
/// `LongInt` converts exactly via [`parse::decimal_from_bigint`], so
/// `Decimal(1) + 10**100` works; only an int wider than
/// [`super::DECIMAL_MAX_DIGITS`] digits errors (the conversion's
/// Monty-specific `ValueError` propagates — a documented divergence).
pub(super) fn promote(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<Option<Decimal>> {
    Ok(match value {
        Value::Bool(b) => Some(Decimal::from_i64(i64::from(*b))),
        Value::Int(i) => Some(Decimal::from_i64(*i)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Decimal(d) => Some(d.clone()),
            HeapData::LongInt(li) => Some(parse::decimal_from_bigint(li.inner())?),
            _ => None,
        },
        _ => None,
    })
}

/// `divmod(decimal, other)` (or swapped) → `(a // b, a % b)` — port of
/// `_pydecimal.__divmod__` (1376-1418), sharing the [`divide`] core with `//`
/// and `%` so the three always agree.
///
/// Returns `Ok(None)` when `other` is not a number `Decimal` operates with.
/// An infinite dividend raises `InvalidOperation` (CPython raises through a
/// tuple slot's `_raise_error`: `divmod(INF, INF)` from the first, `INF % x`
/// from the second — the armed trap fires either way). A zero divisor raises
/// the divmod-specific condition class: `divmod(0, 0)` is
/// `DivisionUndefined`, otherwise both `InvalidOperation` *and*
/// `DivisionByZero` are reported, matching CPython's combined message.
pub(crate) fn divmod(
    d: Decimal,
    other: &Value,
    swapped: bool,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let Some(operand) = promote(other, vm.heap)? else {
        return Ok(None);
    };
    let (a, b) = if swapped { (operand, d) } else { (d, operand) };

    let (quotient, remainder) = if let Some(nan) = check_nans(&a, Some(&b))? {
        // A quiet NaN operand fills both slots.
        (nan.clone(), nan)
    } else if a.is_infinite() {
        return Err(ExcType::decimal_invalid_operation());
    } else if b.is_zero() {
        return Err(if a.is_zero() {
            ExcType::decimal_division_undefined() // divmod(0, 0)
        } else {
            ExcType::decimal_divmod_by_zero() // x // 0 and x % 0 combined
        });
    } else {
        divide(&a, &b, RoundMode::HalfEven)?
    };

    // CPython `_fix`es the remainder; the quotient from `divide` is already
    // canonical (≤ prec digits at exponent 0), so fixing it too is a no-op
    // that keeps both results uniformly finalised.
    let quotient = fix::fix(quotient, RoundMode::HalfEven)?;
    let remainder = fix::fix(remainder, RoundMode::HalfEven)?;
    let quotient = allocate(quotient, vm)?;
    let remainder = allocate(remainder, vm)?;
    Ok(Some(allocate_tuple(smallvec![quotient, remainder], vm.heap)?))
}

/// Unary `-decimal` — port of `_pydecimal.__neg__` (1045-1066), `fix`ed to
/// the working precision. Under any rounding mode but `ROUND_FLOOR`, negating
/// a zero yields a *positive* zero (`-Decimal('0')` and `-Decimal('-0')` are
/// both `Decimal('0')`) — CPython's negative-zero rule, applied here under
/// the fixed `HalfEven`.
pub(crate) fn neg(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let result = neg_core(d, RoundMode::HalfEven)?;
    allocate(result, vm)
}

/// Unary `+decimal` — port of `_pydecimal.__pos__` (1067-1087).
/// Value-preserving but still rounds to the working precision (and normalises
/// `-0` to `0` outside `ROUND_FLOOR`), matching CPython.
pub(crate) fn pos(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let result = pos_core(d, RoundMode::HalfEven)?;
    allocate(result, vm)
}

/// `abs(decimal)` — port of `_pydecimal.__abs__` (1088-1109) with its default
/// `round=True`: dispatches to `__neg__` / `__pos__` by sign (so the result
/// is rounded and `-0`-normalised), unlike the quiet `copy_abs`.
pub(crate) fn abs(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let result = abs_core(d, RoundMode::HalfEven)?;
    allocate(result, vm)
}

/// The `__neg__` body, threaded over `rounding` for faithfulness to CPython's
/// `context.rounding` branch (operator callers pass `HalfEven`). Consumes `d`
/// (the sign flip moves the coefficient instead of cloning it).
fn neg_core(d: Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_special()
        && let Some(nan) = check_nans(&d, None)?
    {
        return Ok(nan);
    }
    let ans = if d.is_zero() && rounding != RoundMode::Floor {
        // -Decimal('0') is Decimal('0'), not Decimal('-0'), except in
        // ROUND_FLOOR rounding mode (`copy_abs`, by move).
        Decimal { sign: 0, ..d }
    } else {
        // `copy_negate`, by move.
        Decimal { sign: d.sign ^ 1, ..d }
    };
    fix::fix(ans, rounding)
}

/// The `__pos__` body — a rounded copy, with `+(-0)` giving `0` outside
/// `ROUND_FLOOR`.
fn pos_core(d: Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_special()
        && let Some(nan) = check_nans(&d, None)?
    {
        return Ok(nan);
    }
    let ans = if d.is_zero() && rounding != RoundMode::Floor {
        // +(-0) = 0, except in ROUND_FLOOR rounding mode (`copy_abs`, by move).
        Decimal { sign: 0, ..d }
    } else {
        d
    };
    fix::fix(ans, rounding)
}

/// The `__abs__(round=True)` body: `__neg__` for a negative value, `__pos__`
/// otherwise (each re-checks NaNs harmlessly, as CPython's do).
fn abs_core(d: Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_signed() {
        neg_core(d, rounding)
    } else {
        pos_core(d, rounding)
    }
}

/// `a + b` — port of `_pydecimal.__add__` (1110-1196). `-INF + INF` (either
/// order) raises `InvalidOperation`; a zero result of opposite-signed
/// operands is negative only under `ROUND_FLOOR` (the `negativezero` rule).
fn add(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if a.is_special() || b.is_special() {
        if let Some(nan) = check_nans(a, Some(b))? {
            return Ok(nan);
        }
        if a.is_infinite() {
            // If both INF, same sign => same as both, opposite => error.
            if a.sign != b.sign && b.is_infinite() {
                return Err(ExcType::decimal_invalid_operation());
            }
            return Ok(a.clone());
        }
        if b.is_infinite() {
            return Ok(b.clone()); // can't both be infinity here
        }
    }

    let exp = a.exp.min(b.exp);
    // If the answer is 0, the sign should be negative, in this case.
    let negativezero = rounding == RoundMode::Floor && a.sign != b.sign;

    if a.is_zero() && b.is_zero() {
        let sign = if negativezero { 1 } else { a.sign.min(b.sign) };
        return fix::fix(Decimal::from_triple(sign, BigInt::ZERO, exp), rounding);
    }
    if a.is_zero() {
        let exp = exp.max(b.exp - PREC - 1);
        return fix::fix(fix::rescale(b, exp, rounding)?, rounding);
    }
    if b.is_zero() {
        let exp = exp.max(a.exp - PREC - 1);
        return fix::fix(fix::rescale(a, exp, rounding)?, rounding);
    }

    let (mut op1, mut op2) = normalize_ops(WorkRep::new(a), WorkRep::new(b));

    let sign = if op1.sign != op2.sign {
        // Equal and opposite.
        if op1.int == op2.int {
            return fix::fix(
                Decimal::from_triple(u8::from(negativezero), BigInt::ZERO, exp),
                rounding,
            );
        }
        if op1.int < op2.int {
            mem::swap(&mut op1, &mut op2);
            // OK, now abs(op1) > abs(op2).
        }
        if op1.sign == 1 {
            mem::swap(&mut op1.sign, &mut op2.sign);
            1
        } else {
            // So we know the sign, and op1 > 0.
            0
        }
    } else if op1.sign == 1 {
        op1.sign = 0;
        op2.sign = 0;
        1
    } else {
        0
    };
    // Now, op1 > abs(op2) > 0.

    let int = if op2.sign == 0 {
        op1.int + op2.int
    } else {
        op1.int - op2.int
    };
    fix::fix(Decimal::from_triple(sign, int, op1.exp), rounding)
}

/// `a - b` — port of `_pydecimal.__sub__` (1198-1212): computed as
/// `a + (-b)` after the NaN check (so an sNaN in `b` raises before the
/// negation copies it).
fn sub(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if (a.is_special() || b.is_special())
        && let Some(nan) = check_nans(a, Some(b))?
    {
        return Ok(nan);
    }
    add(a, &b.copy_negate(), rounding)
}

/// `a * b` — port of `_pydecimal.__mul__` (1229-1275). `(±)INF * 0` (either
/// order) raises `InvalidOperation`; the coefficient-`1` shortcuts keep
/// `x * 10**k` from multiplying coefficients. The general product is bounded
/// by its inputs (two ≤ [`super::DECIMAL_MAX_DIGITS`]+1-digit coefficients),
/// so no tracker check is needed before the multiply.
fn mul(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    let sign = a.sign ^ b.sign;

    if a.is_special() || b.is_special() {
        if let Some(nan) = check_nans(a, Some(b))? {
            return Ok(nan);
        }
        if a.is_infinite() {
            return if b.is_zero() {
                Err(ExcType::decimal_invalid_operation()) // (+-)INF * 0
            } else {
                Ok(Decimal::infinity(sign))
            };
        }
        if b.is_infinite() {
            return if a.is_zero() {
                Err(ExcType::decimal_invalid_operation()) // 0 * (+-)INF
            } else {
                Ok(Decimal::infinity(sign))
            };
        }
    }

    let exp = a.exp + b.exp;

    // Special case for multiplying by zero.
    if a.is_zero() || b.is_zero() {
        // Fixing in case the exponent is out of bounds.
        return fix::fix(Decimal::from_triple(sign, BigInt::ZERO, exp), rounding);
    }
    // Special case for multiplying by a power of 10.
    if a.coeff.is_one() {
        return fix::fix(Decimal::from_triple(sign, b.coeff.clone(), exp), rounding);
    }
    if b.coeff.is_one() {
        return fix::fix(Decimal::from_triple(sign, a.coeff.clone(), exp), rounding);
    }

    fix::fix(Decimal::from_triple(sign, &a.coeff * &b.coeff, exp), rounding)
}

/// `a / b` — port of `_pydecimal.__truediv__` (1277-1334).
///
/// The `10**shift` alignment is bounded: `shift = len(b) - len(a) + prec + 1`,
/// so when non-negative it is at most `b`'s digit count + 29, and `-shift` is
/// at most `a`'s digit count (digit counts capped at
/// [`super::DECIMAL_MAX_DIGITS`] + 1) — never an attacker-scaled exponent.
///
/// The `coeff % 5 == 0 → coeff += 1` step is CPython's inexactness marker:
/// an inexact quotient ending in `0` or `5` is nudged off the rounding
/// boundary so the later `fix` (which only sees `prec + 1` digits) rounds it
/// exactly as the infinitely precise quotient would.
fn true_div(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    let sign = a.sign ^ b.sign;

    if a.is_special() || b.is_special() {
        if let Some(nan) = check_nans(a, Some(b))? {
            return Ok(nan);
        }
        if a.is_infinite() && b.is_infinite() {
            return Err(ExcType::decimal_invalid_operation()); // (+-)INF/(+-)INF
        }
        if a.is_infinite() {
            return Ok(Decimal::infinity(sign));
        }
        if b.is_infinite() {
            // Division by infinity: Clamped is untrapped, and the zero-at-Etiny
            // result is returned *without* `fix`, exactly as CPython does.
            return Ok(Decimal::from_triple(sign, BigInt::ZERO, ETINY));
        }
    }

    // Special cases for zeroes.
    if b.is_zero() {
        return Err(if a.is_zero() {
            ExcType::decimal_division_undefined() // 0 / 0
        } else {
            ExcType::decimal_division_by_zero() // x / 0
        });
    }

    let (coeff, exp) = if a.is_zero() {
        (BigInt::ZERO, a.exp - b.exp)
    } else {
        // OK, so neither = 0, INF or NaN.
        let shift = digit_len(b) - digit_len(a) + PREC + 1;
        let mut exp = a.exp - b.exp - shift;
        let (mut coeff, remainder) = if shift >= 0 {
            (&a.coeff * pow10_bounded(shift.unsigned_abs())).div_rem(&b.coeff)
        } else {
            a.coeff.div_rem(&(&b.coeff * pow10_bounded(shift.unsigned_abs())))
        };
        if remainder.is_zero() {
            // Result is exact; get as close to the ideal exponent as possible.
            let ideal_exp = a.exp - b.exp;
            let ten = BigInt::from(10u8);
            while exp < ideal_exp && (&coeff % &ten).is_zero() {
                coeff /= &ten;
                exp += 1;
            }
        } else if (&coeff % BigInt::from(5u8)).is_zero() {
            // Result is not exact; adjust to ensure correct rounding.
            coeff += BigInt::one();
        }
        (coeff, exp)
    };

    fix::fix(Decimal::from_triple(sign, coeff, exp), rounding)
}

/// `(a // b, a % b)` to `prec` precision — port of `_pydecimal._divide`
/// (1336-1367). Assumes neither operand is a NaN, `a` is not infinite and
/// `b` is nonzero (it may be infinite: quotient `0`, remainder `a`).
///
/// The `10**|op1.exp - op2.exp|` alignment is bounded because it only runs
/// when `-2 < expdiff <= prec`: the exponent gap then equals
/// `expdiff + len(b) - len(a)`, at most `prec` plus the operands' digit
/// counts (≤ [`super::DECIMAL_MAX_DIGITS`] + 1 each). The early-exit
/// rescale's zero-pad is bounded by `len(b)` for the same reason
/// (`expdiff <= -2` gives `a.exp - b.exp <= len(b) - len(a) - 2`). Raises
/// `InvalidOperation [DivisionImpossible]` when the integer quotient would
/// need more than `prec` digits — CPython refuses to materialise a quotient
/// wider than the working precision (`Decimal('1e40') // 3` raises rather
/// than rounding).
fn divide(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<(Decimal, Decimal)> {
    let sign = a.sign ^ b.sign;
    let ideal_exp = if b.is_infinite() { a.exp } else { a.exp.min(b.exp) };

    let expdiff = a.adjusted() - b.adjusted();
    if a.is_zero() || b.is_infinite() || expdiff <= -2 {
        return Ok((
            Decimal::from_triple(sign, BigInt::ZERO, 0),
            fix::rescale(a, ideal_exp, rounding)?,
        ));
    }
    if expdiff <= PREC {
        let mut op1 = WorkRep::new(a);
        let mut op2 = WorkRep::new(b);
        if op1.exp >= op2.exp {
            let gap = u64::try_from(op1.exp - op2.exp).expect("gap bounded by expdiff <= prec (see doc)");
            op1.int *= pow10_bounded(gap);
        } else {
            let gap = u64::try_from(op2.exp - op1.exp).expect("gap bounded by expdiff > -2 (see doc)");
            op2.int *= pow10_bounded(gap);
        }
        let (q, r) = op1.int.div_rem(&op2.int);
        let prec_limit = pow10_bounded(u64::try_from(DEFAULT_PREC).expect("prec fits u64"));
        if q < prec_limit {
            return Ok((
                Decimal::from_triple(sign, q, 0),
                Decimal::from_triple(a.sign, r, ideal_exp),
            ));
        }
    }

    // Here the quotient is too large to be representable.
    Err(ExcType::decimal_division_impossible())
}

/// `a % b` — port of `_pydecimal.__mod__` (1420-1444): the remainder half of
/// [`divide`], `fix`ed. `INF % x` and `x % 0` (nonzero `x`) raise
/// `InvalidOperation` (*not* `DivisionByZero` — only `//` and `/` signal
/// that); `0 % 0` is `DivisionUndefined`.
fn modulo(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if let Some(nan) = check_nans(a, Some(b))? {
        return Ok(nan);
    }
    if a.is_infinite() {
        Err(ExcType::decimal_invalid_operation()) // INF % x
    } else if b.is_zero() {
        Err(if a.is_zero() {
            ExcType::decimal_division_undefined() // 0 % 0
        } else {
            ExcType::decimal_invalid_operation() // x % 0
        })
    } else {
        let (_, remainder) = divide(a, b, rounding)?;
        fix::fix(remainder, rounding)
    }
}

/// `a // b` — port of `_pydecimal.__floordiv__` (1518-1540): the quotient
/// half of [`divide`], returned *without* a further `fix` (it is already an
/// at-most-`prec`-digit integer at exponent 0), exactly as CPython does.
/// `INF // INF` raises; `INF // x` is a signed infinity; `x // 0` raises
/// `DivisionByZero` and `0 // 0` `DivisionUndefined`.
fn floor_div(a: &Decimal, b: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if let Some(nan) = check_nans(a, Some(b))? {
        return Ok(nan);
    }
    if a.is_infinite() {
        if b.is_infinite() {
            Err(ExcType::decimal_invalid_operation()) // INF // INF
        } else {
            Ok(Decimal::infinity(a.sign ^ b.sign))
        }
    } else if b.is_zero() {
        Err(if a.is_zero() {
            ExcType::decimal_division_undefined() // 0 // 0
        } else {
            ExcType::decimal_division_by_zero() // x // 0
        })
    } else {
        Ok(divide(a, b, rounding)?.0)
    }
}

/// `_pydecimal._WorkRep` (5597-5621): the mutable `(sign, int, exp)` working
/// form of a finite `Decimal` used by the add/div kernels — the coefficient
/// as a `BigInt` rather than a digit string, so alignment shifts are integer
/// multiplies.
struct WorkRep {
    sign: u8,
    int: BigInt,
    exp: i64,
}

impl WorkRep {
    /// Extracts the working representation of a finite `Decimal`.
    fn new(d: &Decimal) -> Self {
        Self {
            sign: d.sign,
            int: d.coeff.clone(),
            exp: d.exp,
        }
    }

    /// `len(str(w.int))` as the `i64` the exponent arithmetic works in.
    fn digits(&self) -> i64 {
        magnitude_digits(&self.int)
    }
}

/// `_pydecimal._normalize` (5623-5649): aligns two working reps to a common
/// exponent (and comparable coefficient length) so addition can operate on
/// the raw coefficients.
///
/// The `min(-1, tmp_len - prec - 2)` clamp is the padding guard: adding
/// `10**exp` (with `exp = tmp.exp + min(-1, tmp_len - prec - 2)`) to `tmp`
/// rounds identically to adding any smaller positive quantity, so an `other`
/// below that threshold is *replaced* by the sentinel `1·10**exp`. This
/// bounds the `10**(tmp.exp - other.exp)` pad applied to `tmp` at
/// `max(1, prec + 2 - tmp_len) + other_len - 1` digits — at most `prec + 1`
/// plus `other`'s digit count (≤ [`super::DECIMAL_MAX_DIGITS`] + 1) — so a
/// pair like `1E+999999 + 1E-999999` cannot demand a two-million-digit pad.
fn normalize_ops(op1: WorkRep, op2: WorkRep) -> (WorkRep, WorkRep) {
    let (mut tmp, mut other, swapped) = if op1.exp < op2.exp {
        (op2, op1, true)
    } else {
        (op1, op2, false)
    };

    let tmp_len = tmp.digits();
    let other_len = other.digits();
    let exp = tmp.exp + (-1i64).min(tmp_len - PREC - 2);
    if other_len + other.exp - 1 < exp {
        other.int = BigInt::one();
        other.exp = exp;
    }
    let pad = u64::try_from(tmp.exp - other.exp).expect("tmp has the larger exponent, clamped (see doc)");
    tmp.int *= pow10_bounded(pad);
    tmp.exp = other.exp;

    // Hand the reps back in argument order (CPython mutates through aliases).
    if swapped { (other, tmp) } else { (tmp, other) }
}

/// `len(self._int)` as the `i64` the exponent arithmetic works in (`1` for a
/// zero coefficient, like CPython's `'0'` string).
fn digit_len(d: &Decimal) -> i64 {
    i64::try_from(d.digits()).expect("digit count fits i64")
}
