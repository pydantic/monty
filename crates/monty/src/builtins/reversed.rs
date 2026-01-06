//! Implementation of the reversed() builtin function.

use crate::{
    args::ArgValues,
    for_iterator::ForIterator,
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::List,
    value::Value,
};

/// Implementation of the reversed() builtin function.
///
/// Returns a list with elements in reverse order.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_reversed(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("reversed")?;

    // Collect all items
    let mut iter = ForIterator::new(value, heap, interns)?;
    let mut items = iter.collect(heap, interns)?;
    iter.drop_with_heap(heap);

    // Reverse in place
    items.reverse();

    let heap_id = heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}
