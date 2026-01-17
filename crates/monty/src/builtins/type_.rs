//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues, exception_private::RunResult, heap::Heap, resource::ResourceTracker, types::PyTrait, value::Value,
};

/// Implementation of the type() builtin function.
///
/// Returns the type of an object.
pub fn builtin_type(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("type", heap)?;
    let type_obj = value.py_type(heap);
    value.drop_with_heap(heap);
    Ok(Value::Builtin(Builtins::Type(type_obj)))
}
