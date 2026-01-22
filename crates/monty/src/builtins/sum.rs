//! Implementation of the sum() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::Heap,
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

    // Get the start value, defaulting to 0
    let mut accumulator = match start {
        Some(v) => {
            // Reject string start values - Python explicitly forbids this
            let is_str = matches!(v.py_type(heap), Type::Str);
            if is_str {
                iterable.drop_with_heap(heap);
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

    // Create iterator from the iterable
    let mut iter = MontyIter::new(iterable, heap, interns)?;

    // Sum all items
    while let Some(item) = iter.for_next(heap, interns)? {
        // Get item type before any operations (needed for error messages)
        let item_type = item.py_type(heap);

        // Try to add the item to accumulator
        let add_result = accumulator.py_add(&item, heap, interns);
        item.drop_with_heap(heap);

        match add_result {
            Ok(Some(new_value)) => {
                accumulator.drop_with_heap(heap);
                accumulator = new_value;
            }
            Ok(None) => {
                // Types don't support addition - use binary_type_error for consistent messages
                let acc_type = accumulator.py_type(heap);
                accumulator.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                return Err(ExcType::binary_type_error("+", acc_type, item_type));
            }
            Err(e) => {
                accumulator.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                return Err(e.into());
            }
        }
    }

    iter.drop_with_heap(heap);
    Ok(accumulator)
}
