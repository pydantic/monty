//! Implementation of the id() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, value::Value};

/// Implementation of the id() builtin function.
///
/// Returns the identity of an object (unique integer for the object's lifetime).
#[py_signature("id(obj, /)")]
pub fn builtin_id(vm: &mut VM<'_>, params: &IdArgs) -> RunResult<Value> {
    Ok(params.obj.id().into_value(vm.heap))
}
