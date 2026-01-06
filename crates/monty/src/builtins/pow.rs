//! Implementation of the pow() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::Heap,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::PyTrait,
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Returns base to the power exp. With three arguments, returns (base ** exp) % mod.
/// Handles negative exponents by returning a float.
pub fn builtin_pow(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // pow() accepts 2 or 3 arguments
    let (positional, kwargs) = args.split();
    if !kwargs.is_empty() {
        for (k, v) in kwargs {
            k.drop_with_heap(heap);
            v.drop_with_heap(heap);
        }
        for v in positional {
            v.drop_with_heap(heap);
        }
        return exc_err_fmt!(ExcType::TypeError; "pow() takes no keyword arguments");
    }

    let (base, exp, modulo) = match positional.len() {
        2 => {
            let mut iter = positional.into_iter();
            (iter.next().unwrap(), iter.next().unwrap(), None)
        }
        3 => {
            let mut iter = positional.into_iter();
            (iter.next().unwrap(), iter.next().unwrap(), Some(iter.next().unwrap()))
        }
        n => {
            for v in positional {
                v.drop_with_heap(heap);
            }
            return exc_err_fmt!(ExcType::TypeError; "pow expected 2 or 3 arguments, got {}", n);
        }
    };

    let base = super::round::normalize_bool_to_int(base);
    let exp = super::round::normalize_bool_to_int(exp);
    let modulo = modulo.map(super::round::normalize_bool_to_int);

    let result = if let Some(m) = &modulo {
        // Three-argument pow: modular exponentiation
        match (&base, &exp, &m) {
            (Value::Int(b), Value::Int(e), Value::Int(m_val)) => {
                if *m_val == 0 {
                    exc_err_fmt!(ExcType::ValueError; "pow() 3rd argument cannot be 0")
                } else if *e < 0 {
                    exc_err_fmt!(ExcType::ValueError; "pow() 2nd argument cannot be negative when 3rd argument specified")
                } else {
                    // Use modular exponentiation
                    let result = mod_pow(*b, *e as u64, *m_val);
                    Ok(Value::Int(result))
                }
            }
            _ => {
                exc_err_fmt!(ExcType::TypeError; "pow() 3rd argument not allowed unless all arguments are integers")
            }
        }
    } else {
        // Two-argument pow
        match (&base, &exp) {
            (Value::Int(b), Value::Int(e)) => {
                if *e < 0 {
                    // Negative exponent returns float
                    if *b == 0 {
                        return exc_err_fmt!(ExcType::ZeroDivisionError; "0.0 cannot be raised to a negative power");
                    }
                    Ok(Value::Float((*b as f64).powf(*e as f64)))
                } else {
                    match u32::try_from(*e) {
                        Ok(exp_u32) => match checked_pow_i64(*b, exp_u32) {
                            Some(v) => Ok(Value::Int(v)),
                            None => {
                                // TODO: replace with BigInt once available to match CPython semantics.
                                exc_err_fmt!(ExcType::OverflowError; "result too large to represent")
                            }
                        },
                        Err(_) => {
                            // TODO: replace with BigInt once available to match CPython semantics.
                            exc_err_fmt!(ExcType::OverflowError; "result too large to represent")
                        }
                    }
                }
            }
            (Value::Float(b), Value::Float(e)) => {
                if *b == 0.0 && *e < 0.0 {
                    return exc_err_fmt!(ExcType::ZeroDivisionError; "0.0 cannot be raised to a negative power");
                }
                Ok(Value::Float(b.powf(*e)))
            }
            (Value::Int(b), Value::Float(e)) => {
                if *b == 0 && *e < 0.0 {
                    return exc_err_fmt!(ExcType::ZeroDivisionError; "0.0 cannot be raised to a negative power");
                }
                Ok(Value::Float((*b as f64).powf(*e)))
            }
            (Value::Float(b), Value::Int(e)) => {
                if *b == 0.0 && *e < 0 {
                    return exc_err_fmt!(ExcType::ZeroDivisionError; "0.0 cannot be raised to a negative power");
                }
                if let Ok(exp_i32) = i32::try_from(*e) {
                    Ok(Value::Float(b.powi(exp_i32)))
                } else {
                    Ok(Value::Float(b.powf(*e as f64)))
                }
            }
            _ => {
                exc_err_fmt!(ExcType::TypeError; "unsupported operand type(s) for ** or pow(): '{}' and '{}'", base.py_type(Some(heap)), exp.py_type(Some(heap)))
            }
        }
    };

    base.drop_with_heap(heap);
    exp.drop_with_heap(heap);
    if let Some(m) = modulo {
        m.drop_with_heap(heap);
    }
    result
}

/// Computes (base^exp) % modulo using binary exponentiation.
///
/// Handles negative bases correctly using Python's modulo semantics.
fn mod_pow(base: i64, exp: u64, modulo: i64) -> i64 {
    if modulo == 1 {
        return 0;
    }

    let modulo_u = u128::from(modulo.unsigned_abs());
    let mut result: u128 = 1;
    let mut b = base.rem_euclid(modulo) as u128;
    let mut e = exp;

    while e > 0 {
        if e % 2 == 1 {
            result = (result * b) % modulo_u;
        }
        e /= 2;
        b = (b * b) % modulo_u;
    }

    // Convert back to signed, handling negative modulo
    let result_i64 = result as i64;
    if modulo < 0 && result_i64 > 0 {
        result_i64 + modulo
    } else {
        result_i64
    }
}

fn checked_pow_i64(mut base: i64, mut exp: u32) -> Option<i64> {
    let mut result: i64 = 1;

    while exp > 0 {
        if exp & 1 == 1 {
            result = result.checked_mul(base)?;
        }
        exp >>= 1;
        if exp > 0 {
            base = base.checked_mul(base)?;
        }
    }

    Some(result)
}
