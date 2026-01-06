//! Implementation of the hash() builtin function.

use crate::{
    args::ArgValues, exception_private::ExcType, heap::Heap, intern::Interns, resource::ResourceTracker,
    run_frame::RunResult, types::PyTrait, value::Value,
};

/// Implementation of the hash() builtin function.
///
/// Returns the hash value of an object (if it has one).
/// Raises TypeError for unhashable types like lists and dicts.
pub fn builtin_hash(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("hash")?;
    let result = match value.py_hash(heap, interns) {
        Some(hash) => Ok(Value::Int(hash as i64)),
        None => Err(ExcType::type_error_unhashable(value.py_type(Some(heap)))),
    };
    value.drop_with_heap(heap);
    result
}
