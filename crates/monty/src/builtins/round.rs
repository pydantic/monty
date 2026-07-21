//! Implementation of the round() builtin function.

use num_bigint::{BigInt, Sign};

use crate::{
    args::{ArgValues, FromArgs, is_long_int},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    resource::ResourceTracker,
    types::LongInt,
    value::Value,
};

pub fn normalize_bool_to_int(value: Value) -> Value {
    match value {
        Value::Bool(b) => Value::Int(i64::from(b)),
        other => other,
    }
}

/// Argument shape for `round(number, ndigits=None)` — CPython parses it with
/// `PyArg_ParseTupleAndKeywords("O|O:round")`, so both arguments are
/// keyword-capable, missing-argument errors carry `(pos N)` (`c_named`), and
/// the total pre-count reports `round() takes at most 2 arguments (3 given)`.
#[derive(FromArgs)]
#[from_args(name = "round", style = c_named, at_most_total)]
struct RoundArgs {
    number: Value,
    #[from_args(default = Value::None)]
    ndigits: Value,
}

/// Implementation of the round() builtin function.
///
/// Rounds a number to a given precision in decimal digits.
/// If ndigits is omitted or None, returns the nearest integer.
/// Uses banker's rounding (round half to even).
pub fn builtin_round(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let RoundArgs { number, ndigits } = RoundArgs::from_args(args, vm)?;
    let number = normalize_bool_to_int(number);
    defer_drop!(number, vm);
    defer_drop!(ndigits, vm);

    // Determine the number of digits (None means round to integer)
    let digits: Option<i64> = match ndigits {
        Value::None => None,
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        // A genuine int wider than i64: clamp by sign — the saturating paths
        // below then return the number unchanged (huge positive) or 0 / ±0.0
        // (huge negative), matching CPython's `Py_ssize_t` clamp for floats.
        v if is_long_int(v, vm) => Some(if long_int_is_negative(v, vm) {
            i64::MIN
        } else {
            i64::MAX
        }),
        v => {
            let type_name = v.py_type_name(vm);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
    };

    match number {
        Value::Int(n) => {
            if let Some(d) = digits {
                if d >= 0 {
                    // Positive or zero digits: return the integer unchanged
                    Ok(Value::Int(*n))
                } else {
                    // Negative digits: round to the nearest multiple of 10^|d|,
                    // half to even, exactly in integers — f64 division corrupts
                    // large values and an i64 write-back multiply can overflow.
                    // |n| < 10^19, so any |d| >= 20 rounds to 0, and the i128
                    // intermediates stay far below their limits.
                    let result: i128 = match u32::try_from(d.unsigned_abs()) {
                        Ok(exp @ ..=19) => {
                            let factor = 10_i128.pow(exp);
                            let n = i128::from(*n);
                            let mut q = n / factor;
                            let r2 = (n % factor).abs() * 2;
                            if r2 > factor || (r2 == factor && q % 2 != 0) {
                                q += if n < 0 { -1 } else { 1 };
                            }
                            q * factor
                        }
                        _ => 0,
                    };
                    // Rounding up can cross i64::MAX (e.g. round(2**63 - 1, -1)).
                    Ok(match i64::try_from(result) {
                        Ok(i) => Value::Int(i),
                        Err(_) => LongInt::new(BigInt::from(result)).into_value(vm.heap)?,
                    })
                }
            } else {
                // No digits specified: return the integer unchanged
                Ok(Value::Int(*n))
            }
        }
        Value::Float(f) => {
            if let Some(d) = digits {
                // Round to `d` decimal places using banker's rounding.
                Ok(Value::Float(round_float_to_digits(*f, d)))
            } else {
                // No digits: round to nearest integer and return int (banker's rounding)
                if f.is_nan() {
                    Err(SimpleException::new_msg(ExcType::ValueError, "cannot convert float NaN to integer").into())
                } else if f.is_infinite() {
                    Err(
                        SimpleException::new_msg(ExcType::OverflowError, "cannot convert float infinity to integer")
                            .into(),
                    )
                } else {
                    Ok(Value::Int(f64_to_i64(bankers_round(*f))))
                }
            }
        }
        _ => {
            let type_name = number.py_type_name(vm);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("type {type_name} doesn't define __round__ method"),
            )
            .into())
        }
    }
}

/// True when a LongInt-valued `ndigits` (interned or heap-allocated) is
/// negative — decides which i64 extreme [`builtin_round`] clamps it to.
fn long_int_is_negative(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::InternLongInt(id) => vm.interns.get_long_int(*id).sign() == Sign::Minus,
        Value::Ref(id) => matches!(vm.heap.get(*id), HeapData::LongInt(li) if li.is_negative()),
        _ => false,
    }
}

/// Implements banker's rounding (round half to even).
///
/// This is the rounding mode used by Python's `round()` function.
/// When the value is exactly halfway between two integers, it rounds to the nearest even integer.
fn bankers_round(value: f64) -> f64 {
    let floor = value.floor();
    let frac = value - floor;

    if frac < 0.5 {
        floor
    } else if frac > 0.5 {
        floor + 1.0
    } else {
        // Exactly 0.5 - round to even
        if f64_to_i64(floor) % 2 == 0 { floor } else { floor + 1.0 }
    }
}

/// Rounds a finite float to a given number of decimal digits using banker's rounding.
///
/// This is used for `round(x, ndigits)` where Python always returns a float.
///
/// For large `ndigits` values where scaling by `10**ndigits` would overflow/underflow `f64`,
/// CPython returns either the original value (large positive `ndigits`) or a signed zero
/// (large negative `ndigits`). We mirror that behavior and also preserve the sign of `0.0`.
fn round_float_to_digits(value: f64, digits: i64) -> f64 {
    if !value.is_finite() {
        return value;
    }

    let rounded = if digits >= 0 {
        let Ok(exp) = i32::try_from(digits) else {
            return value;
        };
        let multiplier = 10_f64.powi(exp);
        if !multiplier.is_finite() {
            return value;
        }
        let scaled = value * multiplier;
        if !scaled.is_finite() {
            return value;
        }
        bankers_round(scaled) / multiplier
    } else {
        let Ok(exp) = i32::try_from(digits) else {
            return 0.0_f64.copysign(value);
        };
        let multiplier = 10_f64.powi(exp);
        if multiplier == 0.0 {
            return 0.0_f64.copysign(value);
        }
        let scaled = value * multiplier;
        bankers_round(scaled) / multiplier
    };

    if rounded == 0.0 {
        0.0_f64.copysign(value)
    } else {
        rounded
    }
}

/// Converts `f64` to `i64` using saturating float-to-int casting.
///
/// Monty uses `i64` for integer values, so float-to-int conversion must pick a
/// bounded representation:
/// - Values outside the `i64` range saturate to `i64::MIN`/`i64::MAX`
/// - `NaN` converts to `0`
///
/// This behavior is provided by Rust's `as` casting rules for float-to-int.
fn f64_to_i64(value: f64) -> i64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "intentional truncation; float-to-int casts saturate and map NaN to 0"
    )]
    let result = value as i64;
    result
}
