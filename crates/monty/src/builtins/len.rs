//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("len")?;
    let result = match value.py_len(heap, interns) {
        Some(len) => Ok(Value::Int(len as i64)),
        None => {
            exc_err_fmt!(ExcType::TypeError; "object of type {} has no len()", value.py_repr(heap, interns))
        }
    };
    value.drop_with_heap(heap);
    result
}
