//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues, defer_drop, exception_private::RunResult, heap::Heap, resource::ResourceTracker, types::PyTrait,
    value::Value,
};

/// Implementation of the type() builtin function.
///
/// Returns the type of an object.
pub fn builtin_type(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("type", heap)?;
    defer_drop!(value, heap);
    Ok(Value::Builtin(Builtins::Type(value.py_type(heap))))
}
