//! Implementation of the id() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, resource::ResourceTracker, value::Value,
};

/// Implementation of the id() builtin function.
///
/// Returns the identity of an object (unique integer for the object's lifetime).
pub fn builtin_id(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("id", vm.heap)?;
    defer_drop!(value, vm);

    let id = value.id();

    // Python's id() returns a signed integer; reinterpret bits for large values
    // On 64-bit: large addresses wrap to negative; on 32-bit: always fits positive
    #[expect(
        clippy::cast_possible_wrap,
        reason = "Python id() returns signed; wrapping intentional"
    )]
    let id_i64 = id as i64;
    Ok(Value::Int(id_i64))
}
