//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::HeapData,
    types::{PyTrait, Type, instance::instance_call_dunder_sync},
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("len", vm.heap)?;
    defer_drop!(value, vm);
    if let Value::Ref(id) = value
        && matches!(vm.heap.get(*id), HeapData::Instance(_))
        && let Some(result) = instance_call_dunder_sync(*id, "__len__", None, vm)?
    {
        defer_drop!(result, vm);
        let size = result.as_index(vm, Type::Int)?;
        if size < 0 {
            return Err(ExcType::value_error("__len__() should return >= 0"));
        }
        return Ok(Value::Int(size));
    }
    if let Some(len) = value.py_len(vm) {
        Ok(Value::Int(
            i64::try_from(len).map_err(|_| ExcType::overflow_c_ssize_t())?,
        ))
    } else {
        let type_name = value.py_type_name(vm);
        Err(SimpleException::new_msg(ExcType::TypeError, format!("object of type '{type_name}' has no len()")).into())
    }
}
