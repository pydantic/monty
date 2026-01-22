//! Implementation of the enumerate() builtin function.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait, Tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_enumerate(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<Value> {
    let (iterable, start) = args.get_one_two_args("enumerate", heap)?;

    // Get start index (default 0)
    let mut index: i64 = match &start {
        Some(Value::Int(n)) => *n,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(v) => {
            let type_name = v.py_type(heap);
            iterable.drop_with_heap(heap);
            if let Some(s) = start {
                s.drop_with_heap(heap);
            }
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => 0,
    };

    if let Some(s) = start {
        s.drop_with_heap(heap);
    }

    let mut iter = MontyIter::new(iterable, heap, interns)?;
    let mut result: Vec<Value> = Vec::new();

    while let Some(item) = iter.for_next(heap, interns)? {
        // Create tuple (index, item)
        let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(vec![Value::Int(index), item])))?;
        result.push(Value::Ref(tuple_id));
        index += 1;
    }

    iter.drop_with_heap(heap);
    let heap_id = heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}
