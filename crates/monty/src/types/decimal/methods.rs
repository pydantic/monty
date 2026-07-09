//! `Decimal` methods beyond arithmetic — `quantize`, `to_integral_value`,
//! `copy_sign`, `normalize`, `as_tuple` — plus the number-protocol
//! conversions (`int`/`float`/`round`/`floor`/`ceil`/`trunc`), each a port of
//! the corresponding `_pydecimal.py` method under the fixed context.
//!
//! Method operands follow the C module's `_convert_other(raiseit=True)`
//! semantics (`_pydecimal.py:5996-6013`): only `Decimal` and integers convert
//! implicitly; `float` and `str` — both accepted by the *constructor* — raise
//! `TypeError: conversion from {type} to Decimal is not supported`.

use num_bigint::BigInt;
use num_traits::Pow;

use super::{DEFAULT_PREC, Decimal, EMAX, ETINY, PREC, RoundMode, Special, allocate, check_nans, fix, parse};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData},
    intern::StaticStrings,
    resource::{ResourceTracker, check_pow_size},
    types::{LongInt, NamedTuple, allocate_tuple, str::allocate_string},
    value::{EitherStr, Value},
};

/// Validates that a zero-argument `Decimal` method received no arguments,
/// dropping any that were passed (so reference counts stay balanced) and
/// raising the CPython `Decimal.<method>() takes no arguments (N given)`
/// TypeError otherwise. The qualified name is built only on the error path, so
/// the common (correct) call costs nothing extra.
pub(super) fn check_no_args(args: ArgValues, attr: &EitherStr, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<()> {
    match args {
        ArgValues::Empty => Ok(()),
        other => {
            let count = other.count();
            other.drop_with_heap(vm.heap);
            Err(ExcType::type_error_no_args(
                &format!("Decimal.{}", attr.as_str(vm.interns)),
                count,
            ))
        }
    }
}

/// `Decimal.normalize()` — strips trailing zeros and maps any zero to a
/// (sign-preserving) `0E0`; the port of `_pydecimal.py:2475-2498`. The result
/// is `_fix`ed first, so a constructor literal beyond the context range can
/// raise `Overflow` here (`Decimal('9.9E999999999').normalize()`).
pub(super) fn normalize(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if d.is_special()
        && let Some(nan) = check_nans(&d, None)?
    {
        return allocate(nan, vm);
    }
    let dup = fix::fix(d, RoundMode::HalfEven)?;
    if dup.is_infinite() {
        return allocate(dup, vm);
    }
    // `if not dup: return _dec_from_triple(dup._sign, '0', 0)` — a zero
    // normalizes to exponent 0 whatever exponent it carried (`-0E5` → `-0`).
    if dup.is_zero() {
        return allocate(Decimal::from_triple(dup.sign, BigInt::ZERO, 0), vm);
    }
    // Strip trailing coefficient zeros into the exponent, but never past the
    // largest representable exponent (`exp_max` is `Emax` under clamp=0, so
    // `Decimal('10000000E999992').normalize()` stops at `1E+999999`).
    let digits = dup.coeff_str();
    let bytes = digits.as_bytes();
    let mut end = bytes.len();
    let mut exp = dup.exp;
    while bytes[end - 1] == b'0' && exp < EMAX {
        exp += 1;
        end -= 1;
    }
    let coeff = BigInt::parse_bytes(&bytes[..end], 10).expect("coefficient slice is ASCII digits");
    allocate(Decimal::from_triple(dup.sign, coeff, exp), vm)
}

/// `Decimal.as_tuple()` — the `DecimalTuple(sign, digits, exponent)` named
/// tuple (`_pydecimal.py:922-927`): `sign` is `0`/`1`, `digits` a tuple of
/// coefficient digits, and `exponent` an `int` — or `'F'`/`'n'`/`'N'` for
/// ∞ / NaN / sNaN. An infinity's digits are `(0,)`; a NaN's are its payload
/// digits, `()` when there is no payload (`Decimal('NaN').as_tuple().digits`).
pub(super) fn as_tuple(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let sign = Value::Int(i64::from(d.sign));
    let (digits, exponent) = match d.special {
        Special::Qnan => (payload_digit_values(d), allocate_string("n", vm.heap)?),
        Special::Snan => (payload_digit_values(d), allocate_string("N", vm.heap)?),
        Special::Inf => (vec![Value::Int(0)], allocate_string("F", vm.heap)?),
        Special::Finite => (coeff_digit_values(d), Value::Int(d.exp)),
    };
    let digits_tuple = allocate_tuple(digits.into(), vm.heap)?;
    let field_names = vec![
        EitherStr::from(StaticStrings::DecimalSign),
        EitherStr::from(StaticStrings::DecimalDigits),
        EitherStr::from(StaticStrings::DecimalExponent),
    ];
    let named = NamedTuple::new(
        StaticStrings::DecimalTuple,
        field_names,
        vec![sign, digits_tuple, exponent],
    );
    Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(named))?))
}

/// The coefficient's digits as `Value::Int`s, in order (`"0"` yields `[0]`).
fn coeff_digit_values(d: &Decimal) -> Vec<Value> {
    d.coeff_str().bytes().map(|b| Value::Int(i64::from(b - b'0'))).collect()
}

/// A NaN payload's digits as `Value::Int`s — empty when there is no payload
/// (CPython stores an empty `_int` string; here that's a zero coefficient).
fn payload_digit_values(d: &Decimal) -> Vec<Value> {
    if d.coeff_is_zero() {
        Vec::new()
    } else {
        coeff_digit_values(d)
    }
}

/// Arguments of `Decimal.quantize(exp, rounding=None)`.
///
/// The C module's signature also has a trailing `context=None`; Monty has no
/// `Context` objects, so passing one (positionally or by name) is a
/// `TypeError` (see `limitations/decimal.md`). `c_error` reproduces the C
/// module's wording exactly: `Decimal(1).quantize()` raises `function missing
/// required argument 'exp' (pos 1)` and surplus positionals raise `function
/// takes at most N arguments (M given)`.
#[derive(FromArgs)]
#[from_args(name = "quantize", style = c)]
struct QuantizeArgs {
    exp: Value,
    #[from_args(default)]
    rounding: Option<Value>,
}

/// `Decimal.quantize(exp, rounding=None)` — returns `self` with the exponent
/// of `exp`, rounding under the per-call mode; the port of
/// `_pydecimal.py:2500-2559` (see [`quantize_core`]).
pub(super) fn quantize_method(
    d: Decimal,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    let QuantizeArgs { exp, rounding } = QuantizeArgs::from_args(args, vm)?;
    defer_drop!(exp, vm);
    // The rounding argument validates *before* the operand converts, matching
    // the C module (`Decimal(1).quantize(2.5, rounding='X')` raises the
    // rounding TypeError, not the float-conversion one).
    let rounding = resolve_rounding_arg(rounding, vm)?;
    let exp_target = operand_to_decimal(exp, vm)?;
    let ans = quantize_core(d, &exp_target, rounding)?;
    Ok(CallResult::Value(allocate(ans, vm)?))
}

/// Arguments of `Decimal.to_integral_value(rounding=None)`. As with
/// [`QuantizeArgs`], the C module's trailing `context` parameter is not
/// supported (see `limitations/decimal.md`).
#[derive(FromArgs)]
#[from_args(name = "to_integral_value", style = c)]
struct ToIntegralValueArgs {
    #[from_args(default)]
    rounding: Option<Value>,
}

/// `Decimal.to_integral_value(rounding=None)` — rounds to the nearest integer
/// *quietly* (no `_fix`, no precision cap); the port of
/// `_pydecimal.py:2662-2679`. A value with `exp >= 0` is already integral and
/// passes through untouched, exponent identity included
/// (`Decimal('1E+30').to_integral_value() == Decimal('1E+30')`).
pub(super) fn to_integral_value_method(
    d: Decimal,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    let ToIntegralValueArgs { rounding } = ToIntegralValueArgs::from_args(args, vm)?;
    let rounding = resolve_rounding_arg(rounding, vm)?;
    let ans = if d.is_special() {
        match check_nans(&d, None)? {
            Some(nan) => nan,
            // An infinity is returned unchanged (`Decimal(self)`).
            None => d,
        }
    } else if d.exp >= 0 {
        d
    } else {
        // `rescale(0)` on `exp < 0` only drops digits, so its internal
        // pad guard is unreachable from here.
        fix::rescale(&d, 0, rounding)?
    };
    Ok(CallResult::Value(allocate(ans, vm)?))
}

/// `Decimal.copy_sign(other)` — `self`'s digits with `other`'s sign; the port
/// of `_pydecimal.py:2995-2999`. A *copy* operation: quiet even when either
/// side is an sNaN (`Decimal('sNaN123').copy_sign(-1)` is `-sNaN123`). The
/// operand converts with method (not constructor) semantics, so
/// `copy_sign(-2.5)` raises the float-conversion `TypeError`.
pub(super) fn copy_sign_method(
    d: Decimal,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    let CopySignArgs { other } = CopySignArgs::from_args(args, vm)?;
    defer_drop!(other, vm);
    let other = operand_to_decimal(other, vm)?;
    let ans = Decimal { sign: other.sign, ..d };
    Ok(CallResult::Value(allocate(ans, vm)?))
}

/// Arguments of `Decimal.copy_sign(other)` — the C module's parser wording
/// (`function missing required argument 'other' (pos 1)`).
#[derive(FromArgs)]
#[from_args(name = "copy_sign", style = c)]
struct CopySignArgs {
    other: Value,
}

/// `int(Decimal)` / `Decimal.__trunc__` — truncation toward zero; the port of
/// `_pydecimal.py:1573-1586`. NaN (quiet *or* signaling) raises the CPython
/// `ValueError`, an infinity the `OverflowError`.
pub(crate) fn to_int(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if d.is_nan() {
        // `int(Decimal('sNaN'))` is the same ValueError as a quiet NaN.
        return Err(ExcType::decimal_nan_to_int());
    }
    if d.is_infinite() {
        return Err(ExcType::decimal_infinity_to_int());
    }
    let magnitude = if d.exp >= 0 {
        // `int(self._int) * 10**self._exp` — the exponent comes straight from
        // the value (a constructor literal can carry exp up to ~1e18), so the
        // `10**exp` materialisation is pre-checked against the tracker.
        let exp = u64::try_from(d.exp).expect("non-negative exponent fits u64");
        check_pow_size(4, exp, vm.heap.tracker())?;
        &d.coeff * BigInt::from(10u8).pow(exp)
    } else {
        // `int(self._int[:self._exp] or '0')` — CPython slices the digit
        // *string*: truncation just drops the last `-exp` digits, so no
        // `10**-exp` is ever materialised (exp can be as low as ~-2e18).
        let digits = d.coeff_str();
        let keep = i64::try_from(digits.len()).expect("digit count fits i64") + d.exp;
        match usize::try_from(keep) {
            Ok(keep) if keep > 0 => {
                BigInt::parse_bytes(&digits.as_bytes()[..keep], 10).expect("coefficient slice is ASCII digits")
            }
            _ => BigInt::ZERO,
        }
    };
    let signed = if d.sign == 1 { -magnitude } else { magnitude };
    Ok(LongInt::new(signed).into_value(vm.heap)?)
}

/// `float(Decimal)` — CPython's `float(str(self))` (`_pydecimal.py:1563-1571`):
/// the nearest `f64`, overflowing to ±∞ and underflowing to (signed) zero
/// exactly as `float(<decimal string>)` does. Signed NaNs keep their sign bit;
/// a *signaling* NaN raises CPython's `ValueError`. Consumed by the `float()`
/// constructor and every float-consuming `math` function.
pub(crate) fn to_float(d: &Decimal) -> RunResult<f64> {
    match d.special {
        Special::Snan => Err(ExcType::decimal_snan_to_float()),
        Special::Qnan => Ok(f64::NAN.copysign(if d.sign == 1 { -1.0 } else { 1.0 })),
        Special::Inf => Ok(if d.sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY }),
        Special::Finite => {
            let s = format!("{}{}E{}", if d.sign == 1 { "-" } else { "" }, d.coeff_str(), d.exp);
            // Rust's f64 parser accepts `<digits>E<exp>` for any exponent,
            // saturating to ±∞ / ±0 — the same behaviour as CPython's
            // `float(str)`, and never an error for this shape.
            Ok(s.parse::<f64>().expect("<digits>E<exp> always parses as f64"))
        }
    }
}

/// `round(Decimal)` (one-argument `__round__`, `_pydecimal.py:1831-1843`) —
/// rounds HALF_EVEN to an `int` (`round(Decimal('2.5')) == 2`). NaN raises the
/// `int()` ValueError, infinity the OverflowError (the C module reuses the
/// conversion messages, not `_pydecimal`'s "cannot round a NaN").
#[expect(
    clippy::needless_pass_by_value,
    reason = "the round() builtin hands over its cloned Decimal"
)]
pub(crate) fn round_to_int(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    rescale_to_int(&d, RoundMode::HalfEven, vm)
}

/// `round(Decimal, ndigits)` (two-argument `__round__`,
/// `_pydecimal.py:1826-1830`) — exactly `self.quantize(Decimal('1E-n'))` under
/// the context rounding, full quantize checks included: a quiet NaN passes
/// through, but an infinity — or a result wider than the working precision —
/// raises `InvalidOperation`.
pub(crate) fn round_with_digits(d: Decimal, ndigits: i64, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    // `exp = -ndigits`; `i64::MIN` cannot be negated, but its target exponent
    // (2^63) is far outside quantize's `[Etiny, Emax]` bound, so it raises the
    // same `InvalidOperation` CPython produces for that call.
    let exp = ndigits.checked_neg().ok_or_else(ExcType::decimal_invalid_operation)?;
    let target = Decimal::from_triple(0, BigInt::from(1u8), exp);
    let ans = quantize_core(d, &target, RoundMode::HalfEven)?;
    allocate(ans, vm)
}

/// `math.floor(Decimal)` / `Decimal.__floor__` (`_pydecimal.py:1845-1858`) —
/// the greatest integer `<= self`; specials raise the `int()` errors.
pub(crate) fn floor_to_int(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    rescale_to_int(d, RoundMode::Floor, vm)
}

/// `math.ceil(Decimal)` / `Decimal.__ceil__` (`_pydecimal.py:1860-1873`) —
/// the least integer `>= self`; specials raise the `int()` errors.
pub(crate) fn ceil_to_int(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    rescale_to_int(d, RoundMode::Ceiling, vm)
}

/// `math.trunc(Decimal)` / `Decimal.__trunc__` — an alias of `__int__`
/// (`_pydecimal.py:1588`).
pub(crate) fn trunc_to_int(d: &Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    to_int(d, vm)
}

/// Shared core of the one-argument `__round__`/`__floor__`/`__ceil__`:
/// `int(self._rescale(0, rounding))` with the specials raising the `int()`
/// conversion errors first (so `round(Decimal('sNaN'))` is the NaN
/// `ValueError`, *not* `InvalidOperation`).
fn rescale_to_int(d: &Decimal, rounding: RoundMode, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    if d.is_nan() {
        Err(ExcType::decimal_nan_to_int())
    } else if d.is_infinite() {
        Err(ExcType::decimal_infinity_to_int())
    } else if d.exp >= 0 {
        // Already integral: CPython's `_rescale(0)` would pad the coefficient
        // with `exp` zeros only for `int()` to re-parse them; converting
        // directly yields the same integer with the `10**exp` materialisation
        // guarded by `to_int`'s `check_pow_size` instead of an unbounded pad.
        to_int(d, vm)
    } else {
        // `exp < 0` only drops digits, so rescale's pad guard is unreachable.
        to_int(&fix::rescale(d, 0, rounding)?, vm)
    }
}

/// The post-conversion body of `quantize` — `_pydecimal.py:2510-2559` with
/// the untrapped signal sites (`Subnormal`/`Inexact`/`Rounded`) omitted.
/// Shared by [`quantize_method`] and [`round_with_digits`], whose CPython
/// counterpart literally calls `quantize`, so the two surfaces cannot drift.
fn quantize_core(d: Decimal, exp_target: &Decimal, rounding: RoundMode) -> RunResult<Decimal> {
    if d.is_special() || exp_target.is_special() {
        if let Some(nan) = check_nans(&d, Some(exp_target))? {
            return Ok(nan);
        }
        // Both infinite is a plain copy; one infinite is invalid. The C module
        // raises its bare `[<class 'decimal.InvalidOperation'>]` message here
        // (not `_pydecimal`'s "quantize with one INF" wording).
        if exp_target.is_infinite() || d.is_infinite() {
            return if exp_target.is_infinite() && d.is_infinite() {
                Ok(d)
            } else {
                Err(ExcType::decimal_invalid_operation())
            };
        }
    }

    // The target exponent must lie within `[Etiny, Emax]`.
    if !(ETINY..=EMAX).contains(&exp_target.exp) {
        return Err(ExcType::decimal_invalid_operation());
    }

    // A zero self just adopts the target exponent (then `_fix` clamps).
    if d.is_zero() {
        return fix::fix(Decimal::from_triple(d.sign, BigInt::ZERO, exp_target.exp), rounding);
    }

    // The result may neither exceed Emax nor need more than `prec` digits.
    // The digit bound also caps `rescale`'s zero pad at `prec`, keeping its
    // internal pad guard unreachable.
    let self_adjusted = d.adjusted();
    if self_adjusted > EMAX {
        return Err(ExcType::decimal_invalid_operation());
    }
    if self_adjusted - exp_target.exp + 1 > PREC {
        return Err(ExcType::decimal_invalid_operation());
    }

    let ans = fix::rescale(&d, exp_target.exp, rounding)?;
    // Re-check after rescaling: a rounding carry can add a digit
    // (`Decimal('9.999…995').quantize(Decimal('1e-27'))` raises).
    if ans.adjusted() > EMAX {
        return Err(ExcType::decimal_invalid_operation());
    }
    if ans.digits() > DEFAULT_PREC {
        return Err(ExcType::decimal_invalid_operation());
    }

    // The final `_fix` under the same per-call rounding — beyond exponent
    // clamping it is a no-op here, kept for line-for-line fidelity.
    fix::fix(ans, rounding)
}

/// Resolves the per-call `rounding=` argument of `quantize` /
/// `to_integral_value`: absent or `None` selects the fixed context's
/// `ROUND_HALF_EVEN`; one of the eight `ROUND_*` strings selects that mode;
/// anything else — a non-mode string *or* a non-string — raises the C
/// module's single "valid values for rounding are: …" TypeError.
fn resolve_rounding_arg(rounding: Option<Value>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<RoundMode> {
    let Some(value) = rounding else {
        return Ok(RoundMode::HalfEven);
    };
    if matches!(value, Value::None) {
        return Ok(RoundMode::HalfEven);
    }
    let mode = match &value {
        Value::InternString(id) => fix::rounding_mode_from_str(vm.interns.get_str(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => fix::rounding_mode_from_str(s.as_str()),
            _ => None,
        },
        _ => None,
    };
    value.drop_with_heap(vm);
    mode.ok_or_else(fix::invalid_rounding_error)
}

/// Converts a method operand with the C module's `_convert_other(raiseit=True)`
/// semantics: `Decimal` passes through, integers (`bool` included) convert
/// exactly, and everything else — notably `float` and `str`, which the
/// *constructor* accepts — raises `TypeError: conversion from {type} to
/// Decimal is not supported`.
fn operand_to_decimal(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Decimal> {
    match value {
        Value::Bool(_) | Value::Int(_) => parse::decimal_from_value(value, vm),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_) | HeapData::Decimal(_)) => {
            parse::decimal_from_value(value, vm)
        }
        other => Err(ExcType::decimal_unsupported_conversion(other.py_type_name(vm))),
    }
}
