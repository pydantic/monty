//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("len", vm.heap)?;
    defer_drop!(value, vm);
    if let Some(len) = value.py_len(vm) {
        Ok(Value::Int(i64::try_from(len).expect("len exceeds i64::MAX")))
    } else {
        let type_name = value.py_type(vm);
        Err(SimpleException::new_msg(ExcType::TypeError, format!("object of type '{type_name}' has no len()")).into())
    }
}
