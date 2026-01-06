//! Implementation of the any() builtin function.

use crate::{
    args::ArgValues, for_iterator::ForIterator, heap::Heap, intern::Interns, resource::ResourceTracker,
    run_frame::RunResult, types::PyTrait, value::Value,
};

/// Implementation of the any() builtin function.
///
/// Returns True if any element of the iterable is true.
/// Returns False for an empty iterable. Short-circuits on the first truthy value.
pub fn builtin_any(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let iterable = args.get_one_arg("any")?;
    let mut iter = ForIterator::new(iterable, heap, interns)?;

    while let Some(item) = iter.for_next(heap, interns)? {
        let is_truthy = item.py_bool(heap, interns);
        item.drop_with_heap(heap);
        if is_truthy {
            iter.drop_with_heap(heap);
            return Ok(Value::Bool(true));
        }
    }

    iter.drop_with_heap(heap);
    Ok(Value::Bool(false))
}
