//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, heap::HeapData, resource::ResourceTracker,
    types::PyTrait, value::Value,
};

/// Implementation of the type() builtin function.
///
/// Returns the type of an object. For an instance of a user-defined class the
/// type *is* the class object itself, so `type(x) is Foo` holds via reference
/// identity; for everything else it returns the builtin `Type` marker.
pub fn builtin_type(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("type", vm.heap)?;
    defer_drop!(value, vm);
    if let Value::Ref(id) = &value
        && let HeapData::Instance(inst) = vm.heap.get(*id)
    {
        let class_id = inst.class();
        vm.heap.inc_ref(class_id);
        Ok(Value::Ref(class_id))
    } else {
        Ok(Value::Builtin(Builtins::Type(value.py_type(vm))))
    }
}
