//! Implementation of the abs() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    py_value::PyValue,
    types::timedelta,
    value::Value,
};

/// Returns the absolute value of a number or timedelta, promoting integers on overflow.
pub fn builtin_abs(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("abs", vm.heap)?;
    defer_drop!(value, vm);

    match value.py_value(vm) {
        PyValue::Int(integer) => Ok(integer.abs(vm)),
        PyValue::Float(value) => Ok(Value::Float(value.abs())),
        PyValue::Bool(value) => Ok(Value::Int(i64::from(value))),
        PyValue::Complex(value) => Ok(Value::Float(value.get(vm.heap).abs()?)),
        PyValue::TimeDelta(value) => {
            let total = timedelta::total_microseconds(value.get(vm.heap));
            let abs_total = total.checked_abs().unwrap_or(total);
            let delta = timedelta::from_total_microseconds(abs_total)?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))))
        }
        _ => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("bad operand type for abs(): '{}'", value.py_type_name(vm)),
        )
        .into()),
    }
}
