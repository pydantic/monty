//! Implementation of the sum() builtin function.

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapGuard},
    intern::Interns,
    resource::ResourceTracker,
    types::{MontyIter, PyTrait, Type},
    value::Value,
};

/// Implementation of the sum() builtin function.
///
/// Sums the items of an iterable from left to right with an optional start value.
/// The default start value is 0. String start values are explicitly rejected
/// (use `''.join(seq)` instead for string concatenation).
pub fn builtin_sum(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (iterable, start) = args.get_one_two_args("sum", heap)?;
    defer_drop_mut!(start, heap);

    let iter = MontyIter::new(iterable, heap, interns)?;
    defer_drop_mut!(iter, heap);

    // Get the start value, defaulting to 0
    let accumulator = match start.take() {
        Some(v) => {
            // Reject string start values - Python explicitly forbids this
            if matches!(v.py_type(heap), Type::Str) {
                v.drop_with_heap(heap);
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    "sum() can't sum strings [use ''.join(seq) instead]",
                )
                .into());
            }
            v
        }
        None => Value::Int(0),
    };

    // HeapGuard for accumulator: on success we extract it via into_inner(),
    // on any error path it's dropped automatically
    let mut acc_guard = HeapGuard::new(accumulator, heap);
    let (accumulator, heap) = acc_guard.as_parts_mut();

    // Sum all items
    while let Some(item) = iter.for_next(heap, interns)? {
        defer_drop!(item, heap);

        // Try to add the item to accumulator
        if let Some(new_value) = accumulator.py_add(item, heap, interns)? {
            // Replace the old accumulator with the new value, dropping the old one
            let old = std::mem::replace(accumulator, new_value);
            old.drop_with_heap(heap);
        } else {
            // Types don't support addition
            let acc_type = accumulator.py_type(heap);
            let item_type = item.py_type(heap);
            return Err(ExcType::binary_type_error("+", acc_type, item_type));
        }
    }

    Ok(acc_guard.into_inner())
}
