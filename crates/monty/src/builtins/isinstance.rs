//! Implementation of the isinstance() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapRead, HeapReadOutput},
    types::{
        PyTrait, Tuple, Type,
        class::{class_is_subclass, native_base},
        native_class,
    },
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
/// Supports:
/// - Single builtin types: `isinstance(x, int)`
/// - Exception types and their hierarchy: `isinstance(err, LookupError)`
/// - User-defined classes: `isinstance(obj, Foo)` (the instance's class or an ancestor)
/// - Host classes: `isinstance(obj, Point)` for a `HostClassType` (exact class
///   id; the host sends no bases)
/// - Tuples (possibly nested) of the above
fn isinstance_check(obj: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match classinfo {
        Value::Builtin(Builtins::Type(t)) => Ok(obj.py_type(vm).is_instance_of(*t)
            || matches!(obj.py_type(vm),Type::Instance(id) if native_base(id,vm).is_some_and(|native|native.is_instance_of(*t)))),
        Value::Builtin(Builtins::ExcType(handler_type)) => Ok(exception_matches(obj, *handler_type, vm)),
        // A user-defined class matches the instance's class or an ancestor.
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Class(_)) => Ok(instance_of_class(obj, *id, vm)),
        // A `collections.namedtuple` class, matched by the instance's `class_id`.
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::NamedTupleClass(_)) => {
            Ok(instance_of_namedtuple_class(obj, *id, vm))
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::HostClassType(_)) => {
            Ok(instance_of_host_class(obj, *id, vm))
        }
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

/// Whether `obj` is a host instance whose class entry is `class_id` (exact
/// class only: the host never sends bases, so subclasses are unknown).
fn instance_of_host_class(obj: &Value, class_id: HeapId, vm: &VM<'_>) -> bool {
    matches!(obj, Value::Ref(obj_id) if matches!(vm.heap.get(*obj_id), HeapData::HostClass(hc) if hc.class_id() == class_id))
}

fn exception_matches(obj: &Value, handler: ExcType, vm: &VM<'_>) -> bool {
    matches!(obj.py_type(vm), Type::Exception(exc) if exc.is_subclass_of(handler))
        || matches!(obj,Value::Ref(id) if native_class::exception_type(*id,vm).is_some_and(|exc|exc.is_subclass_of(handler)))
}

/// Whether `obj` belongs to `class_id` or one of its descendants.
fn instance_of_class(obj: &Value, class_id: HeapId, vm: &VM<'_>) -> bool {
    let Value::Ref(obj_id) = obj else {
        return false;
    };
    let HeapData::Instance(instance) = vm.heap.get(*obj_id) else {
        return false;
    };
    class_is_subclass(instance.class(), class_id, vm)
}

/// Whether `obj` is a namedtuple instance built from the class `class_id`.
///
/// Instances created by Monty internally (`sys.version_info`, host imports)
/// carry no `class_id`, so they never match a factory class.
fn instance_of_namedtuple_class(obj: &Value, class_id: HeapId, vm: &VM<'_>) -> bool {
    matches!(obj, Value::Ref(obj_id) if matches!(vm.heap.get(*obj_id), HeapData::NamedTuple(nt) if nt.class_id() == Some(class_id)))
}

/// Recursively walks a tuple of classinfo entries.
fn isinstance_check_tuple<'h>(obj: &Value, tuple: &HeapRead<'h, Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for i in 0..len {
        match &tuple.get(vm.heap).as_slice()[i] {
            Value::Builtin(Builtins::Type(t)) => {
                if obj.py_type(vm).is_instance_of(*t)
                    || matches!(obj.py_type(vm),Type::Instance(id) if native_base(id,vm).is_some_and(|native|native.is_instance_of(*t)))
                {
                    return Ok(true);
                }
            }
            Value::Builtin(Builtins::ExcType(exc)) => {
                if exception_matches(obj, *exc, vm) {
                    return Ok(true);
                }
            }
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Class(_)) => {
                if instance_of_class(obj, *id, vm) {
                    return Ok(true);
                }
            }
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::NamedTupleClass(_)) => {
                if instance_of_namedtuple_class(obj, *id, vm) {
                    return Ok(true);
                }
            }
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::HostClassType(_)) => {
                if instance_of_host_class(obj, *id, vm) {
                    return Ok(true);
                }
            }
            Value::Ref(nested_id) if let HeapReadOutput::Tuple(nested) = vm.heap.read(*nested_id) => {
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
