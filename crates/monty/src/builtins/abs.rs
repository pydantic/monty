//! Implementation of the abs() builtin function.

use num_bigint::BigInt;
use num_traits::Signed;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    types::{LongInt, PyTrait},
    value::Value,
};

/// Implementation of the abs() builtin function.
///
/// Returns the absolute value of a number. Works with integers, floats, and LongInts.
/// For `i64::MIN`, which overflows on negation, promotes to LongInt.
pub fn builtin_abs(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("abs", heap)?;

    let result = match &value {
        Value::Int(n) => {
            // Handle potential overflow for i64::MIN → promote to LongInt
            if let Some(abs_val) = n.checked_abs() {
                Ok(Value::Int(abs_val))
            } else {
                // i64::MIN.abs() overflows, promote to LongInt
                let bi = BigInt::from(*n).abs();
                Ok(LongInt::new(bi).into_value(heap)?)
            }
        }
        Value::Float(f) => Ok(Value::Float(f.abs())),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        Value::Ref(id) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                Ok(li.abs().into_value(heap)?)
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
