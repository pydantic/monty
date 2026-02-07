//! Implementation of the all() builtin function.

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

/// Implementation of the all() builtin function.
///
/// Returns True if all elements of the iterable are true (or if the iterable is empty).
/// Short-circuits on the first falsy value.
pub fn builtin_all(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let iterable = args.get_one_arg("all", heap)?;
    let iter = MontyIter::new(iterable, heap, interns)?;
    defer_drop_mut!(iter, heap);

    while let Some(item) = iter.for_next(heap, interns)? {
        defer_drop!(item, heap);
        let is_truthy = item.py_bool(heap, interns);
        if !is_truthy {
            return Ok(Value::Bool(false));
        }
    }

    Ok(Value::Bool(true))
}
