//! Implementation of the repr() builtin function.

use crate::{
    args::ArgValues,
    exception_private::RunResult,
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
pub fn builtin_repr(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("repr", heap)?;
    let heap_id = heap.allocate(HeapData::Str(value.py_repr(heap, interns).into_owned().into()))?;
    value.drop_with_heap(heap);
    Ok(Value::Ref(heap_id))
}
