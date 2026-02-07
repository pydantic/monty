//! Implementation of the divmod() builtin function.

use num_bigint::BigInt;
use num_integer::Integer;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    types::{LongInt, PyTrait, allocate_tuple},
    value::Value,
};

/// Implementation of the divmod() builtin function.
///
/// Returns a tuple (quotient, remainder) from integer division.
/// Equivalent to (a // b, a % b).
pub fn builtin_divmod(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a, b) = args.get_two_args("divmod", heap)?;
    let a = super::round::normalize_bool_to_int(a);
    let b = super::round::normalize_bool_to_int(b);
    defer_drop!(a, heap);
    defer_drop!(b, heap);

    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                Err(ExcType::divmod_by_zero())
            } else {
                // Python uses floor division (toward negative infinity), not Euclidean
                let (quot, rem) = floor_divmod(*x, *y);
                Ok(allocate_tuple(smallvec![Value::Int(quot), Value::Int(rem)], heap)?)
            }
        }
        (Value::Int(x), Value::Ref(id)) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                if li.is_zero() {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let x_bi = BigInt::from(*x);
                    let (quot, rem) = bigint_floor_divmod(&x_bi, li.inner());
                    let quot_val = LongInt::new(quot).into_value(heap)?;
                    let rem_val = LongInt::new(rem).into_value(heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], heap)?)
                }
            } else {
                let a_type = a.py_type(heap);
                let b_type = b.py_type(heap);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into())
            }
        }
        (Value::Ref(id), Value::Int(y)) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                if *y == 0 {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let y_bi = BigInt::from(*y);
                    let (quot, rem) = bigint_floor_divmod(li.inner(), &y_bi);
                    let quot_val = LongInt::new(quot).into_value(heap)?;
                    let rem_val = LongInt::new(rem).into_value(heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], heap)?)
                }
            } else {
                let a_type = a.py_type(heap);
                let b_type = b.py_type(heap);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into())
            }
        }
        (Value::Ref(id1), Value::Ref(id2)) => {
            let x_bi = if let HeapData::LongInt(li) = heap.get(*id1) {
                li.inner().clone()
            } else {
                let a_type = a.py_type(heap);
                let b_type = b.py_type(heap);
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into());
            };
            if let HeapData::LongInt(li) = heap.get(*id2) {
                if li.is_zero() {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let (quot, rem) = bigint_floor_divmod(&x_bi, li.inner());
                    let quot_val = LongInt::new(quot).into_value(heap)?;
                    let rem_val = LongInt::new(rem).into_value(heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], heap)?)
                }
            } else {
                let a_type = a.py_type(heap);
                let b_type = b.py_type(heap);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into())
            }
        }
        (Value::Float(x), Value::Float(y)) => {
            if *y == 0.0 {
                Err(ExcType::divmod_by_zero())
            } else {
                let quot = (x / y).floor();
                let rem = x - quot * y;
                Ok(allocate_tuple(smallvec![Value::Float(quot), Value::Float(rem)], heap)?)
            }
        }
        (Value::Int(x), Value::Float(y)) => {
            if *y == 0.0 {
                Err(ExcType::divmod_by_zero())
            } else {
                let xf = *x as f64;
                let quot = (xf / y).floor();
                let rem = xf - quot * y;
                Ok(allocate_tuple(smallvec![Value::Float(quot), Value::Float(rem)], heap)?)
            }
        }
        (Value::Float(x), Value::Int(y)) => {
            if *y == 0 {
                Err(ExcType::divmod_by_zero())
            } else {
                let yf = *y as f64;
                let quot = (x / yf).floor();
                let rem = x - quot * yf;
                Ok(allocate_tuple(smallvec![Value::Float(quot), Value::Float(rem)], heap)?)
            }
        }
        _ => {
            let a_type = a.py_type(heap);
            let b_type = b.py_type(heap);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
            )
            .into())
        }
    }
}

/// Computes Python-style floor division and modulo.
///
/// Python's division rounds toward negative infinity (floor division),
/// and the remainder has the same sign as the divisor.
/// This differs from Rust's truncating division and Euclidean division.
fn floor_divmod(a: i64, b: i64) -> (i64, i64) {
    // Use truncating division first
    let quot = a / b;
    let rem = a % b;

    // Adjust for floor division: if signs differ and remainder != 0, adjust
    if rem != 0 && (rem < 0) != (b < 0) {
        (quot - 1, rem + b)
    } else {
        (quot, rem)
    }
}

/// Computes Python-style floor division and modulo for BigInts.
///
/// Uses `div_mod_floor` from num_integer for correct floor semantics.
fn bigint_floor_divmod(a: &BigInt, b: &BigInt) -> (BigInt, BigInt) {
    a.div_mod_floor(b)
}
