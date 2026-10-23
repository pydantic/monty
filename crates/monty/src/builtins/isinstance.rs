//! Implementation of the isinstance() builtin function.

use super::{Builtins, BuiltinsFunctions};
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapRead, HeapReadOutput},
    types::{PyTrait, Tuple, Type, class::ClassBase},
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
/// - User-defined classes: `isinstance(obj, Foo)` (matches the instance's
///   class or any base class in its MRO)
/// - Host classes: `isinstance(obj, Point)` for a `HostClassType` (exact class
///   id; the host sends no bases)
/// - Tuples (possibly nested) of the above
pub(crate) fn isinstance_check(obj: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match classinfo {
        Value::Builtin(Builtins::Type(t)) => Ok(obj.py_type(vm).is_instance_of(*t)),
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => Ok(matches!(
            obj,
            Value::Builtin(Builtins::Type(_) | Builtins::ExcType(_) | Builtins::Function(BuiltinsFunctions::Type))
        ) || matches!(
            obj,
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Class(_) | HeapData::NamedTupleClass(_) | HeapData::HostClassType(_))
        )),
        Value::Builtin(Builtins::ExcType(handler_type)) => {
            let matched = match obj.py_type(vm) {
                Type::Exception(exc_type) => exc_type.is_subclass_of(*handler_type),
                _ => {
                    if let Value::Ref(obj_id) = obj
                        && let HeapData::Instance(inst) = vm.heap.get(*obj_id)
                        && let HeapData::Class(cls) = vm.heap.get(inst.class())
                    {
                        cls.mro()
                            .iter()
                            .any(|b| matches!(b, ClassBase::BuiltinExc(exc) if exc.is_subclass_of(*handler_type)))
                    } else {
                        false
                    }
                }
            };
            Ok(matched)
        }
        // A user-defined class: true iff `obj` is an instance of this class or one of its subclasses.
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

/// Whether `obj` is an instance whose class object is `class_id` or a subclass of it.
fn instance_of_class(obj: &Value, class_id: HeapId, vm: &VM<'_>) -> bool {
    let Value::Ref(obj_id) = obj else {
        return false;
    };
    let HeapData::Instance(inst) = vm.heap.get(*obj_id) else {
        return false;
    };
    inst.class() == class_id
        || matches!(vm.heap.get(inst.class()), HeapData::Class(cls) if cls.is_subclass_of(class_id))
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
    vm.heap.tracker.check_time()?;
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for i in 0..len {
        vm.heap.tracker.check_time_every(i)?;
        let item = tuple.get(vm.heap).as_slice()[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if isinstance_check(obj, item, vm)? {
            return Ok(true);
        }
    }
    Ok(false)
}
