//! Implementation of the sum() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{exc_err_fmt, ExcType},
    for_iterator::ForIterator,
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Type},
    value::Value,
};

/// Implementation of the sum() builtin function.
///
/// Sums the items of an iterable from left to right with an optional start value.
/// The default start value is 0. String start values are explicitly rejected
/// (use `''.join(seq)` instead for string concatenation).
pub fn builtin_sum(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (iterable, start) = args.get_one_two_args("sum")?;

    // Get the start value, defaulting to 0
    let mut accumulator = match start {
        Some(v) => {
            // Reject string start values - Python explicitly forbids this
            let is_str = matches!(v.py_type(Some(heap)), Type::Str);
            if is_str {
                iterable.drop_with_heap(heap);
                v.drop_with_heap(heap);
                return exc_err_fmt!(ExcType::TypeError; "sum() can't sum strings [use ''.join(seq) instead]");
            }
            v
        }
        None => Value::Int(0),
    };

    // Create iterator from the iterable
    let mut iter = ForIterator::new(iterable, heap, interns)?;

    // Sum all items
    while let Some(item) = iter.for_next(heap, interns)? {
        // Get item type before any operations (needed for error messages)
        let item_type = item.py_type(Some(heap));

        // Try to add the item to accumulator
        let add_result = accumulator.py_add(&item, heap, interns);
        item.drop_with_heap(heap);

        match add_result {
            Ok(Some(new_value)) => {
                accumulator.drop_with_heap(heap);
                accumulator = new_value;
            }
            Ok(None) => {
                // Types don't support addition
                let acc_type = accumulator.py_type(Some(heap));
                accumulator.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                return exc_err_fmt!(ExcType::TypeError; "unsupported operand type(s) for +: '{}' and '{}'", acc_type, item_type);
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
