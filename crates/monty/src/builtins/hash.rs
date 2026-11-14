//! Implementation of the hash() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, types::unhashable::plain_hash,
    value::Value,
};

/// Implementation of the hash() builtin function.
///
/// Returns the hash value of an object (if it has one).
/// Raises TypeError for unhashable types like lists and dicts.
pub fn builtin_hash(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hash", vm.heap)?;
    defer_drop!(value, vm);
    // Python's hash() returns a signed integer; reinterpret bits for large values
    Ok(Value::Int(plain_hash(value, vm)?.raw().cast_signed()))
}
