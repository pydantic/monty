//! Implementation of the enumerate() builtin function.

use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::HeapData,
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait, allocate_tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_enumerate(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, start) = args.get_one_two_args("enumerate", vm.heap)?;
    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);
    defer_drop!(start, vm);

    // Get start index (default 0)
    let mut index: i64 = match start {
        Some(Value::Int(n)) => *n,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(v) => {
            let type_name = v.py_type(vm);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => 0,
    };

    let mut result: Vec<Value> = Vec::new();

    while let Some(item) = iter.for_next(vm)? {
        // Create tuple (index, item)
        let tuple_val = allocate_tuple(smallvec![Value::Int(index), item], vm.heap)?;
        result.push(tuple_val);
        index += 1;
    }

    let heap_id = vm.heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}
