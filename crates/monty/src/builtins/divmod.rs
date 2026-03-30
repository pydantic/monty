//! Implementation of the divmod() builtin function.

use num_bigint::BigInt;
use num_integer::Integer;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    resource::{ResourceTracker, check_div_size},
    types::{LongInt, PyTrait, allocate_tuple},
    value::{Value, floor_divmod},
};

/// Implementation of the divmod() builtin function.
///
/// Returns a tuple (quotient, remainder) from integer division.
/// Equivalent to (a // b, a % b).
pub fn builtin_divmod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (a, b) = args.get_two_args("divmod", vm.heap)?;
    let a = super::round::normalize_bool_to_int(a);
    let b = super::round::normalize_bool_to_int(b);
    defer_drop!(a, vm);
    defer_drop!(b, vm);

    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                Err(ExcType::divmod_by_zero())
            } else if let Some((quot, rem)) = floor_divmod(*x, *y) {
                Ok(allocate_tuple(smallvec![Value::Int(quot), Value::Int(rem)], vm.heap)?)
            } else {
                // Overflow - promote to BigInt
                check_div_size(64, vm.heap.tracker())?;
                let (quot, rem) = bigint_floor_divmod(&BigInt::from(*x), &BigInt::from(*y));
                let quot_val = LongInt::new(quot).into_value(vm.heap)?;
                let rem_val = LongInt::new(rem).into_value(vm.heap)?;
                Ok(allocate_tuple(smallvec![quot_val, rem_val], vm.heap)?)
            }
        }
        (Value::Int(x), Value::Ref(id)) => {
            if let HeapData::LongInt(li) = vm.heap.get(*id) {
                if li.is_zero() {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let x_bi = BigInt::from(*x);
                    let (quot, rem) = bigint_floor_divmod(&x_bi, li.inner());
                    let quot_val = LongInt::new(quot).into_value(vm.heap)?;
                    let rem_val = LongInt::new(rem).into_value(vm.heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], vm.heap)?)
                }
            } else {
                let a_type = a.py_type(vm);
                let b_type = b.py_type(vm);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into())
            }
        }
        (Value::Ref(id), Value::Int(y)) => {
            if let HeapData::LongInt(li) = vm.heap.get(*id) {
                if *y == 0 {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let y_bi = BigInt::from(*y);
                    let (quot, rem) = bigint_floor_divmod(li.inner(), &y_bi);
                    let quot_val = LongInt::new(quot).into_value(vm.heap)?;
                    let rem_val = LongInt::new(rem).into_value(vm.heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], vm.heap)?)
                }
            } else {
                let a_type = a.py_type(vm);
                let b_type = b.py_type(vm);
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into())
            }
        }
        (Value::Ref(id1), Value::Ref(id2)) => {
            let x_bi = if let HeapData::LongInt(li) = vm.heap.get(*id1) {
                li.inner().clone()
            } else {
                let a_type = a.py_type(vm);
                let b_type = b.py_type(vm);
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
                )
                .into());
            };
            if let HeapData::LongInt(li) = vm.heap.get(*id2) {
                if li.is_zero() {
                    Err(ExcType::divmod_by_zero())
                } else {
                    let (quot, rem) = bigint_floor_divmod(&x_bi, li.inner());
                    let quot_val = LongInt::new(quot).into_value(vm.heap)?;
                    let rem_val = LongInt::new(rem).into_value(vm.heap)?;
                    Ok(allocate_tuple(smallvec![quot_val, rem_val], vm.heap)?)
                }
            } else {
                let a_type = a.py_type(vm);
                let b_type = b.py_type(vm);
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
                Ok(allocate_tuple(
                    smallvec![Value::Float(quot), Value::Float(rem)],
                    vm.heap,
                )?)
            }
        }
        (Value::Int(x), Value::Float(y)) => {
            if *y == 0.0 {
                Err(ExcType::divmod_by_zero())
            } else {
                let xf = *x as f64;
                let quot = (xf / y).floor();
                let rem = xf - quot * y;
                Ok(allocate_tuple(
                    smallvec![Value::Float(quot), Value::Float(rem)],
                    vm.heap,
                )?)
            }
        }
        (Value::Float(x), Value::Int(y)) => {
            if *y == 0 {
                Err(ExcType::divmod_by_zero())
            } else {
                let yf = *y as f64;
                let quot = (x / yf).floor();
                let rem = x - quot * yf;
                Ok(allocate_tuple(
                    smallvec![Value::Float(quot), Value::Float(rem)],
                    vm.heap,
                )?)
            }
        }
        _ => {
            let a_type = a.py_type(vm);
            let b_type = b.py_type(vm);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("unsupported operand type(s) for divmod(): '{a_type}' and '{b_type}'"),
            )
            .into())
        }
    }
}

/// Computes Python-style floor division and modulo for BigInts.
///
/// Uses `div_mod_floor` from num_integer for correct floor semantics.
fn bigint_floor_divmod(a: &BigInt, b: &BigInt) -> (BigInt, BigInt) {
    a.div_mod_floor(b)
}
