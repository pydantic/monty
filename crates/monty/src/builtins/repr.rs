//! Implementation of the repr() builtin function.

use ahash::AHashSet;

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
/// Calls `py_repr_fmt` directly (instead of the `py_repr` convenience method)
/// so that `fmt::Error` from an oversized LongInt inside a container is
/// converted to a `ValueError` rather than silently producing truncated output.
pub fn builtin_repr(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    check_value_str_digits(value, vm.heap, vm.interns)?;
    let mut s = String::new();
    let mut heap_ids = AHashSet::new();
    value.py_repr_fmt(&mut s, vm, &mut heap_ids)?;
    let heap_id = vm.heap.allocate(HeapData::Str(s.into()))?;
    Ok(Value::Ref(heap_id))
}
