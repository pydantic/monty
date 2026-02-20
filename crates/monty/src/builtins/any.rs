//! Implementation of the any() builtin function.

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::RunResult,
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait},
    value::Value,
};

/// Implementation of the any() builtin function.
///
/// Returns True if any element of the iterable is true.
/// Returns False for an empty iterable. Short-circuits on the first truthy value.
pub fn builtin_any(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let iterable = args.get_one_arg("any", heap)?;
    let iter = MontyIter::new(iterable, heap, interns)?;
    defer_drop_mut!(iter, heap);

    while let Some(item) = iter.for_next(heap, interns)? {
        defer_drop!(item, heap);
        let is_truthy = item.py_bool(heap, interns);
        if is_truthy {
            return Ok(Value::Bool(true));
        }
    }

    Ok(Value::Bool(false))
}
