//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::{Instance, PyTrait, instance::list_subclass_len},
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("len", vm.heap)?;
    defer_drop!(value, vm);
    if let Value::Ref(id) = value
        && value
            .get_as::<Instance>(vm.heap)
            .is_some_and(|instance| instance.native_list().is_some())
    {
        return list_subclass_len(*id, vm).map(Value::Int);
    }
    if let Some(len) = value.py_len(vm) {
        Ok(Value::Int(
            i64::try_from(len).map_err(|_| ExcType::overflow_c_ssize_t())?,
        ))
    } else {
        let type_name = value.py_type_name(vm);
        Err(ExcType::type_error_no_len(&type_name))
    }
}
