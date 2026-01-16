//! Implementation of the abs() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the abs() builtin function.
///
/// Returns the absolute value of a number. Works with integers and floats.
pub fn builtin_abs(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("abs")?;

    let result = match &value {
        Value::Int(n) => {
            // Handle potential overflow for i64::MIN
            Ok(Value::Int(n.checked_abs().unwrap_or(i64::MIN)))
        }
        Value::Float(f) => Ok(Value::Float(f.abs())),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("bad operand type for abs(): '{}'", value.py_type(heap)),
        )
        .into()),
    };

    value.drop_with_heap(heap);
    result
}
