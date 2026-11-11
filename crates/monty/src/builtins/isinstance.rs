//! Implementation of the isinstance() builtin function.

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapRead, HeapReadOutput},
    types::{Tuple, TypeRef},
    value::Value,
};

/// Implementation of the isinstance() builtin function.
///
/// Checks if an object is an instance of a class or a tuple of classes.
pub fn builtin_isinstance(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (obj, classinfo) = args.get_two_args("isinstance", vm.heap)?;
    defer_drop!(obj, vm);
    defer_drop!(classinfo, vm);

    isinstance_check(obj, classinfo, vm).map(Value::Bool)
}

/// Checks if `obj` matches a single classinfo entry.
///
/// A type (native or a class) matches when `type(obj)` is it or inherits from
/// it; a tuple or a union matches when any member does, tested in order.
pub(crate) fn isinstance_check(obj: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    if let Some(class) = TypeRef::from_value(classinfo, vm.heap) {
        return Ok(instance_of(obj, class, vm));
    }
    match classinfo {
        Value::Ref(id) if let HeapReadOutput::Tuple(tuple) = vm.heap.read(*id) => {
            isinstance_check_tuple(obj, &tuple, vm)
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::GenericAlias(_)) => {
            Err(ExcType::isinstance_parameterized_generic())
        }
        // `int | None`: true when any member matches, tested in order.
        Value::Ref(id) if let HeapData::Union(union) = vm.heap.get(*id) => {
            let args = union.args(vm.heap);
            defer_drop!(args, vm);
            let Some(HeapReadOutput::Tuple(members)) = args.read_heap(vm) else {
                unreachable!("Union::args is always a tuple")
            };
            isinstance_check_tuple(obj, &members, vm)
        }
        _ => Err(ExcType::isinstance_arg2_error()),
    }
}

/// Whether `obj` is an instance of `class` or of a subclass of it.
fn instance_of(obj: &Value, class: TypeRef, vm: &VM<'_>) -> bool {
    obj.py_class(vm).is_subclass_of(class, vm.heap)
}

/// Recursively walks a tuple of classinfo entries.
fn isinstance_check_tuple<'h>(obj: &Value, tuple: &HeapRead<'h, Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for i in 0..len {
        // Each entry may walk the whole MRO, so a long flat tuple needs polling too.
        vm.heap.tracker.check_time_every(i)?;
        let item = &tuple.get(vm.heap).as_slice()[i];
        if let Some(class) = TypeRef::from_value(item, vm.heap) {
            if instance_of(obj, class, vm) {
                return Ok(true);
            }
            continue;
        }
        match item {
            Value::Ref(nested_id) if let HeapReadOutput::Tuple(nested) = vm.heap.read(*nested_id) => {
                // A tuple can hold itself many times over, so walking nested
                // tuples is exponential in their depth: bound it by time.
                vm.heap.tracker.check_time()?;
                if isinstance_check_tuple(obj, &nested, vm)? {
                    return Ok(true);
                }
            }
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::GenericAlias(_)) => {
                return Err(ExcType::isinstance_parameterized_generic());
            }
            Value::Ref(id) if let HeapData::Union(union) = vm.heap.get(*id) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let Some(HeapReadOutput::Tuple(members)) = args.read_heap(vm) else {
                    unreachable!("Union::args is always a tuple")
                };
                if isinstance_check_tuple(obj, &members, vm)? {
                    return Ok(true);
                }
            }
            _ => return Err(ExcType::isinstance_arg2_error()),
        }
    }
    Ok(false)
}
