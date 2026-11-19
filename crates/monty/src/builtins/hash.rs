//! Implementation of the hash() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::key_hash::plain_hash, value::Value};

/// Implementation of the hash() builtin function.
///
/// Returns the hash value of an object (if it has one).
/// Raises TypeError for unhashable types like lists and dicts.
#[py_signature("hash(obj, /)")]
pub fn builtin_hash(vm: &mut VM<'_>, args: &HashArgs) -> RunResult<Value> {
    // Python's hash() returns a signed integer; reinterpret bits for large values
    Ok(Value::Int(plain_hash(&args.obj, vm)?.raw().cast_signed()))
}
