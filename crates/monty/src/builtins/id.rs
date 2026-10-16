//! Implementation of the id() builtin function.

use crate::{args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, heap::RuntimeRef, value::Value};

/// Implementation of the id() builtin function.
///
/// Returns the identity of an object (unique integer for the object's lifetime).
pub fn builtin_id<H: RuntimeRef>(vm: &mut VM<'_, H>, args: ArgValues<H>) -> RunResult<Value<H>> {
    let value = args.get_one_arg("id", vm.heap)?;
    defer_drop!(value, vm);

    Ok(value.id().into_value(vm.heap))
}
