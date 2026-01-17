//! Implementation of the abs() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    types::{PyTrait, bigint::bigint_to_value},
    value::Value,
};

/// Implementation of the abs() builtin function.
///
/// Returns the absolute value of a number. Works with integers, floats, and BigInts.
/// For `i64::MIN`, which overflows on negation, promotes to BigInt.
pub fn builtin_abs(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("abs")?;

    let result = match &value {
        Value::Int(n) => {
            // Handle potential overflow for i64::MIN → promote to BigInt
            if let Some(abs_val) = n.checked_abs() {
                Ok(Value::Int(abs_val))
            } else {
                // i64::MIN.abs() overflows, promote to BigInt
                let bi = BigInt::from(*n).abs();
                Ok(bigint_to_value(bi, heap)?)
            }
        }
        Value::Float(f) => Ok(Value::Float(f.abs())),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        Value::Ref(id) => {
            if let HeapData::BigInt(bi) = heap.get(*id) {
                let abs_bi = bi.abs();
                Ok(bigint_to_value(abs_bi, heap)?)
            } else {
                Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("bad operand type for abs(): '{}'", value.py_type(heap)),
                )
                .into())
            }
        }
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("bad operand type for abs(): '{}'", value.py_type(heap)),
        )
        .into()),
    };

    value.drop_with_heap(heap);
    result
}
