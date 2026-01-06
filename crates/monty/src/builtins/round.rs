//! Implementation of the round() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::Heap,
    resource::ResourceTracker,
    run_frame::RunResult,
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
                    let factor = 10_i64.saturating_pow((-d) as u32);
                    let rounded = bankers_round(*n as f64 / factor as f64) as i64 * factor;
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
                let multiplier = 10_f64.powi(d as i32);
                let scaled = f * multiplier;
                let rounded = bankers_round(scaled) / multiplier;
                Ok(Value::Float(rounded))
            } else {
                // No digits: round to nearest integer and return int (banker's rounding)
                Ok(Value::Int(bankers_round(*f) as i64))
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
        if floor as i64 % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    }
}
