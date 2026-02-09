//! Implementation of the len() builtin function.

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
    intern::Interns,
    resource::{DepthGuard, ResourceTracker},
    types::PyTrait,
    value::Value,
};

/// Implementation of the len() builtin function.
///
/// Returns the length of an object (number of items in a container).
pub fn builtin_len(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("len", heap)?;
    defer_drop!(value, heap);
    if let Some(len) = value.py_len(heap, interns) {
        Ok(Value::Int(i64::try_from(len).expect("len exceeds i64::MAX")))
    } else {
        let mut guard = DepthGuard::default();
        Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!(
                "object of type {} has no len()",
                value.py_repr(heap, &mut guard, interns)
            ),
        )
        .into())
    }
}
