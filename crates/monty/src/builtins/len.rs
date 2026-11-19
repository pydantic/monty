//! Implementation of the len() builtin function.

use monty_macros::py_signature;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
#[py_signature("len(obj, /)", style = one_arg)]
pub fn builtin_len(vm: &mut VM<'_>, obj: &Value) -> RunResult<Value> {
    if let Some(len) = obj.py_len(vm) {
        Ok(Value::Int(
            i64::try_from(len).map_err(|_| ExcType::overflow_c_ssize_t())?,
        ))
    } else {
        let type_name = obj.py_type_name(vm);
        Err(ExcType::type_error_no_len(&type_name))
    }
}
