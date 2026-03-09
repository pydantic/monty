//! Implementation of the reversed() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    heap::HeapData,
    resource::ResourceTracker,
    types::{List, MontyIter},
    value::Value,
};

/// Implementation of the reversed() builtin function.
///
/// Returns a list with elements in reverse order.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_reversed(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("reversed", vm.heap)?;

    // Collect all items
    let mut items: Vec<_> = MontyIter::new(value, vm)?.collect(vm)?;

    // Reverse in place
    items.reverse();

    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}
