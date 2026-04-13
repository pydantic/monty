//! Implementation of the hash() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the hash() builtin function.
///
/// Returns the hash value of an object (if it has one).
/// Raises TypeError for unhashable types like lists and dicts.
pub fn builtin_hash(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("hash", vm.heap)?;
    defer_drop!(value, vm);
    match value.py_hash(vm)? {
        Some(hash) => {
            // Python's hash() returns a signed integer; reinterpret bits for large values
            let hash_i64 = i64::from_ne_bytes(hash.to_ne_bytes());
            Ok(Value::Int(hash_i64))
        }
        None => Err(ExcType::type_error_unhashable(value.py_type(vm))),
    }
}
