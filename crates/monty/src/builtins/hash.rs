//! Implementation of the hash() builtin function.

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the hash() builtin function.
///
/// Returns the hash value of an object (if it has one).
/// Raises TypeError for unhashable types like lists and dicts.
pub fn builtin_hash(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("hash", heap)?;
    defer_drop!(value, heap);
    match value.py_hash(heap, interns)? {
        Some(hash) => {
            // Python's hash() returns a signed integer; reinterpret bits for large values
            let hash_i64 = i64::from_ne_bytes(hash.to_ne_bytes());
            Ok(Value::Int(hash_i64))
        }
        None => Err(ExcType::type_error_unhashable(value.py_type(heap))),
    }
}
