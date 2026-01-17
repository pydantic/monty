//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("len", heap)?;
    let result = match value.py_len(heap, interns) {
        Some(len) => Ok(Value::Int(i64::try_from(len).expect("len exceeds i64::MAX"))),
        None => Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("object of type {} has no len()", value.py_repr(heap, interns)),
        )
        .into()),
    };
    value.drop_with_heap(heap);
    result
}
