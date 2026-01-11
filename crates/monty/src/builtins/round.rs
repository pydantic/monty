//! Implementation of the round() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType, RunResult},
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
            let type_name = v.py_type(Some(heap));
            number.drop_with_heap(heap);
            v.drop_with_heap(heap);
            return exc_err_fmt!(ExcType::TypeError; "'{}' object cannot be interpreted as an integer", type_name);
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
                // Round to d decimal places using banker's rounding
                // Clamp exponent to i32 range for powi
                let exp = i32::try_from(d).unwrap_or(if d > 0 { i32::MAX } else { i32::MIN });
                let multiplier = 10_f64.powi(exp);
                let scaled = f * multiplier;
                let rounded = bankers_round(scaled) / multiplier;
                Ok(Value::Float(rounded))
            } else {
                // No digits: round to nearest integer and return int (banker's rounding)
                Ok(Value::Int(f64_to_i64(bankers_round(*f))))
            }
        }
        _ => {
            exc_err_fmt!(ExcType::TypeError; "type {} doesn't define __round__ method", number.py_type(Some(heap)))
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
        if f64_to_i64(floor) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    }
}

/// Converts f64 to i64, clamping to i64 bounds if out of range.
///
/// This matches Python's behavior where very large floats truncate to the
/// maximum/minimum representable integer.
fn f64_to_i64(value: f64) -> i64 {
    // Handle special cases
    if value.is_nan() {
        return 0;
    }
    // Clamp and convert; the truncation is intentional for rounding
    let clamped = value.clamp(i64::MIN as f64, i64::MAX as f64).trunc();
    // SAFETY for clippy: clamped is guaranteed to be in [i64::MIN, i64::MAX]
    // after the clamp() call, so truncation cannot overflow
    #[expect(clippy::cast_possible_truncation, reason = "clamped to i64 range")]
    let result = clamped as i64;
    result
}
