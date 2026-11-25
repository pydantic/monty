//! Implementation of the callable() builtin function.

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcTypeExt, RunResult},
    value::Value,
};

/// Implementation of `callable(obj, /)`.
///
/// Returns `True` if the object argument appears callable, `False` if not.
/// If this returns `True`, it is still possible that a call fails, but if
/// it returns `False`, calling `obj` will never succeed.
///
/// Signature: `callable(obj, /)`
pub fn builtin_callable(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let positional = args.into_pos_only("callable", vm.heap)?;
    defer_drop!(positional, vm);

    let object = match positional.as_slice() {
        [object] => object,
        other => return Err(ExcType::type_error_arg_count("callable", 1, other.len())),
    };

    Ok(Value::Bool(object.is_callable(vm)))
}
