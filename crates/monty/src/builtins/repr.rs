//! Implementation of the repr() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::HeapData,
    resource::ResourceTracker,
    types::{PyTrait, long_int::check_value_str_digits},
    value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
pub fn builtin_repr(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    check_value_str_digits(value, vm.heap, vm.interns)?;
    let heap_id = vm.heap.allocate(HeapData::Str(value.py_repr(vm).into_owned().into()))?;
    Ok(Value::Ref(heap_id))
}
