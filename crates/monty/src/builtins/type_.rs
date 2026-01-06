//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues, heap::Heap, resource::ResourceTracker, run_frame::RunResult, types::PyTrait, value::Value,
};

/// Implementation of the type() builtin function.
///
/// Returns the type of an object.
pub fn builtin_type(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("type")?;
    let type_obj = value.py_type(Some(heap));
    value.drop_with_heap(heap);
    Ok(Value::Builtin(Builtins::Type(type_obj)))
}
