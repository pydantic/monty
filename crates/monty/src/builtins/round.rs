//! Implementation of the round() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

pub fn normalize_bool_to_int(value: Value) -> Value {
    match value {
        Value::Bool(b) => Value::Int(i64::from(b)),
        other => other,
    }
}

/// Implementation of the round() builtin function.
///
/// Rounds a number to a given precision in decimal digits.
/// If ndigits is omitted or None, returns the nearest integer.
/// Uses banker's rounding (round half to even).
pub fn builtin_round(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (number, ndigits) = args.get_one_two_args("round")?;
    let number = normalize_bool_to_int(number);

    // Determine the number of digits (None means round to integer)
    // Extract digits value before potentially consuming ndigits for error handling
    let (digits, ndigits_to_drop): (Option<i64>, Option<Value>) = match ndigits {
        Some(Value::None) => (None, Some(Value::None)),
        Some(Value::Int(n)) => (Some(n), Some(Value::Int(n))),
        Some(Value::Bool(b)) => (Some(i64::from(b)), Some(Value::Bool(b))),
        Some(v) => {
            let type_name = v.py_type(heap);
            number.drop_with_heap(heap);
            v.drop_with_heap(heap);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => (None, None),
    };

    let result = match &number {
        Value::Int(n) => {
            if let Some(d) = digits {
                if d >= 0 {
                    // Positive or zero digits: return the integer unchanged
                    Ok(Value::Int(*n))
                } else {
                    // Negative digits: round to tens, hundreds, etc. using banker's rounding
                    // -d is positive since d < 0; use try_from to safely convert
                    let exp = u32::try_from(-d).unwrap_or(u32::MAX);
                    let factor = 10_i64.saturating_pow(exp);
                    let rounded_f = bankers_round(*n as f64 / factor as f64);
                    let rounded = f64_to_i64(rounded_f) * factor;
                    Ok(Value::Int(rounded))
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
            let type_name = number.py_type(heap);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("type {type_name} doesn't define __round__ method"),
            )
            .into())
        }
    };

    number.drop_with_heap(heap);
    if let Some(v) = ndigits_to_drop {
        v.drop_with_heap(heap);
    }
    result
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
