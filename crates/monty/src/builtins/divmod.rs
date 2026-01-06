//! Implementation of the divmod() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Tuple},
    value::Value,
};

/// Implementation of the divmod() builtin function.
///
/// Returns a tuple (quotient, remainder) from integer division.
/// Equivalent to (a // b, a % b).
pub fn builtin_divmod(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a, b) = args.get_two_args("divmod")?;
    let a = super::round::normalize_bool_to_int(a);
    let b = super::round::normalize_bool_to_int(b);

    let result = match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                exc_err_fmt!(ExcType::ZeroDivisionError; "integer division or modulo by zero")
            } else {
                // Python uses floor division (toward negative infinity), not Euclidean
                let (quot, rem) = floor_divmod(*x, *y);
                let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(vec![Value::Int(quot), Value::Int(rem)])))?;
                Ok(Value::Ref(tuple_id))
            }
        }
        (Value::Float(x), Value::Float(y)) => {
            if *y == 0.0 {
                exc_err_fmt!(ExcType::ZeroDivisionError; "float divmod()")
            } else {
                let quot = (x / y).floor();
                let rem = x - quot * y;
                let tuple_id =
                    heap.allocate(HeapData::Tuple(Tuple::new(vec![Value::Float(quot), Value::Float(rem)])))?;
                Ok(Value::Ref(tuple_id))
            }
        }
        (Value::Int(x), Value::Float(y)) => {
            if *y == 0.0 {
                exc_err_fmt!(ExcType::ZeroDivisionError; "float divmod()")
            } else {
                let xf = *x as f64;
                let quot = (xf / y).floor();
                let rem = xf - quot * y;
                let tuple_id =
                    heap.allocate(HeapData::Tuple(Tuple::new(vec![Value::Float(quot), Value::Float(rem)])))?;
                Ok(Value::Ref(tuple_id))
            }
        }
        (Value::Float(x), Value::Int(y)) => {
            if *y == 0 {
                exc_err_fmt!(ExcType::ZeroDivisionError; "float divmod()")
            } else {
                let yf = *y as f64;
                let quot = (x / yf).floor();
                let rem = x - quot * yf;
                let tuple_id =
                    heap.allocate(HeapData::Tuple(Tuple::new(vec![Value::Float(quot), Value::Float(rem)])))?;
                Ok(Value::Ref(tuple_id))
            }
        }
        _ => {
            exc_err_fmt!(ExcType::TypeError; "unsupported operand type(s) for divmod(): '{}' and '{}'", a.py_type(Some(heap)), b.py_type(Some(heap)))
        }
    };

    a.drop_with_heap(heap);
    b.drop_with_heap(heap);
    result
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
