//! Implementation of the isinstance() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapRead, HeapReadOutput},
    resource::ResourceTracker,
    types::{PyTrait, Tuple, Type},
    value::Value,
};

/// Implementation of the isinstance() builtin function.
///
/// Checks if an object is an instance of a class or a tuple of classes.
pub fn builtin_isinstance(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (obj, classinfo) = args.get_two_args("isinstance", vm.heap)?;
    defer_drop!(obj, vm);
    defer_drop!(classinfo, vm);
    let obj_type = obj.py_type(vm);

    isinstance_check(obj_type, classinfo, vm).map(Value::Bool)
}

/// Checks if `obj_type` matches a single classinfo entry.
///
/// Supports:
/// - Single types: `isinstance(x, int)`
/// - Exception types: `isinstance(err, ValueError)`
/// - Exception hierarchy: `isinstance(err, LookupError)` for KeyError/IndexError
/// - Tuples (possibly nested) of the above
fn isinstance_check(obj_type: Type, classinfo: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<bool> {
    match classinfo {
        Value::Builtin(Builtins::Type(t)) => Ok(obj_type.is_instance_of(*t)),
        Value::Builtin(Builtins::ExcType(handler_type)) => {
            Ok(matches!(obj_type, Type::Exception(exc_type) if exc_type.is_subclass_of(*handler_type)))
        }
        Value::Ref(id) if let HeapReadOutput::Tuple(tuple) = vm.heap.read(*id) => {
            isinstance_check_tuple(obj_type, &tuple, vm)
        }
        _ => Err(ExcType::isinstance_arg2_error()),
    }
}

/// Recursively walks a tuple of classinfo entries.
fn isinstance_check_tuple<'h>(
    obj_type: Type,
    tuple: &HeapRead<'h, Tuple>,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<bool> {
    let len = tuple.get(vm.heap).as_slice().len();
    let token = vm.heap.incr_recursion_depth()?;
    defer_drop!(token, vm);
    for i in 0..len {
        match &tuple.get(vm.heap).as_slice()[i] {
            Value::Builtin(Builtins::Type(t)) => {
                if obj_type.is_instance_of(*t) {
                    return Ok(true);
                }
            }
            Value::Builtin(Builtins::ExcType(exc)) => {
                if matches!(obj_type, Type::Exception(et) if et.is_subclass_of(*exc)) {
                    return Ok(true);
                }
            }
            Value::Ref(nested_id) if let HeapReadOutput::Tuple(tuple) = vm.heap.read(*nested_id) => {
                if isinstance_check_tuple(obj_type, &tuple, vm)? {
                    return Ok(true);
                }
            }
            _ => return Err(ExcType::isinstance_arg2_error()),
        }
    }
    Ok(false)
}
