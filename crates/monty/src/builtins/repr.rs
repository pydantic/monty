//! Implementation of the repr() builtin function.

use ahash::AHashSet;

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, heap::HeapData, resource::ResourceTracker,
    types::PyTrait, value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
/// Uses `py_repr_fmt` directly to write into a pre-allocated buffer, avoiding
/// the intermediate `Cow` allocation that `py_repr` returns.
pub fn builtin_repr(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    let mut s = String::new();
    let mut heap_ids = AHashSet::new();
    value.py_repr_fmt(&mut s, vm, &mut heap_ids)?;
    let heap_id = vm.heap.allocate(HeapData::Str(s.into()))?;
    Ok(Value::Ref(heap_id))
}
