//! Implementation of the issubclass() builtin function.

use super::{Builtins, BuiltinsFunctions};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapRead, HeapReadOutput},
    types::{Tuple, Type, class::ClassBase},
    value::Value,
};

#[derive(FromArgs)]
#[from_args(name = "issubclass", style = unpack)]
struct IssubclassArgs {
    #[from_args(pos_only)]
    cls: Value,
    #[from_args(pos_only)]
    classinfo: Value,
}

/// Implementation of the issubclass() builtin function.
///
/// Checks if a class is a subclass of another class or a tuple/union of classes.
pub fn builtin_issubclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IssubclassArgs { cls, classinfo } = IssubclassArgs::from_args(args, vm)?;
    defer_drop!(cls, vm);
    defer_drop!(classinfo, vm);

    issubclass_check(cls, classinfo, vm).map(Value::Bool)
}

/// Validates that `val` is a class object.
fn validate_is_class(val: &Value, vm: &VM<'_>) -> RunResult<()> {
    match val {
        Value::Builtin(Builtins::Type(_) | Builtins::ExcType(_) | Builtins::Function(BuiltinsFunctions::Type)) => {
            Ok(())
        }
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Class(_) | HeapData::NamedTupleClass(_) | HeapData::HostClassType(_) => Ok(()),
            _ => Err(ExcType::issubclass_arg1_error()),
        },
        _ => Err(ExcType::issubclass_arg1_error()),
    }
}

/// Checks if `cls` is a subclass of `classinfo`.
pub(crate) fn issubclass_check(cls: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    if let Value::Ref(id) = classinfo {
        match vm.heap.read(*id) {
            HeapReadOutput::Tuple(tuple) => return issubclass_check_tuple(cls, &tuple, vm),
            HeapReadOutput::Union(union) => {
                let args = union.get(vm.heap).args(vm.heap);
                defer_drop!(args, vm);
                let Some(HeapReadOutput::Tuple(members)) = args.read_heap(vm) else {
                    unreachable!("Union::args is always a tuple")
                };
                return issubclass_check_tuple(cls, &members, vm);
            }
            _ => {}
        }
    }

    validate_is_class(cls, vm)?;

    match classinfo {
        Value::Builtin(Builtins::Type(t)) => Ok(class_is_subclass_of_type(cls, (*t).into(), vm)),
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => {
            Ok(class_is_subclass_of_type(cls, Type::Type, vm))
        }
        Value::Builtin(Builtins::ExcType(handler_type)) => {
            if let Value::Builtin(Builtins::ExcType(exc_type)) = cls {
                Ok(exc_type.is_subclass_of(*handler_type))
            } else if let Value::Ref(id) = cls
                && let HeapData::Class(class) = vm.heap.get(*id)
            {
                Ok(class
                    .mro()
                    .iter()
                    .any(|b| matches!(b, ClassBase::BuiltinExc(exc) if exc.is_subclass_of(*handler_type))))
            } else {
                Ok(false)
            }
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Class(_)) => {
            Ok(class_is_subclass_of_class(cls, *id, vm))
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::NamedTupleClass(_)) => {
            Ok(class_is_subclass_of_namedtuple(cls, *id))
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::HostClassType(_)) => {
            Ok(class_is_subclass_of_host_class(cls, *id, vm))
        }
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::GenericAlias(_)) => {
            Err(ExcType::issubclass_parameterized_generic())
        }
        _ => Err(ExcType::issubclass_arg2_error()),
    }
}

fn class_is_subclass_of_type(cls: &Value, target: Type, vm: &VM<'_>) -> bool {
    if target == Type::Object {
        return true;
    }
    match cls {
        Value::Builtin(Builtins::Type(t)) => {
            let t_type: Type = (*t).into();
            if t_type == target || (t_type == Type::Bool && target == Type::Int) {
                true
            } else {
                t_type.is_instance_of(target)
            }
        }
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => target == Type::Type,
        Value::Builtin(Builtins::ExcType(_)) => false,
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::NamedTupleClass(_) => target == Type::Tuple,
            _ => false,
        },
        _ => false,
    }
}

fn class_is_subclass_of_class(cls: &Value, target_id: HeapId, vm: &VM<'_>) -> bool {
    let Value::Ref(cls_id) = cls else {
        return false;
    };
    if *cls_id == target_id {
        return true;
    }
    let HeapData::Class(cls_data) = vm.heap.get(*cls_id) else {
        return false;
    };
    cls_data.is_subclass_of(target_id)
}

fn class_is_subclass_of_namedtuple(cls: &Value, target_id: HeapId) -> bool {
    matches!(cls, Value::Ref(cls_id) if *cls_id == target_id)
}

fn class_is_subclass_of_host_class(cls: &Value, target_id: HeapId, vm: &VM<'_>) -> bool {
    let Value::Ref(cls_id) = cls else {
        return false;
    };
    if *cls_id == target_id {
        return true;
    }
    match (vm.heap.get(*cls_id), vm.heap.get(target_id)) {
        (HeapData::HostClassType(c1), HeapData::HostClassType(c2)) => c1.type_id() == c2.type_id(),
        _ => false,
    }
}

fn issubclass_check_tuple<'h>(cls: &Value, tuple: &HeapRead<'h, Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
    vm.heap.tracker.check_time()?;
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for i in 0..len {
        vm.heap.tracker.check_time_every(i)?;
        let item = tuple.get(vm.heap).as_slice()[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if issubclass_check(cls, item, vm)? {
            return Ok(true);
        }
    }
    Ok(false)
}
