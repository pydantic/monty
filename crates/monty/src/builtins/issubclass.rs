//! Implementation of the issubclass() builtin function.

use super::{Builtins, BuiltinsFunctions};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapRead, HeapReadOutput},
    types::{
        Tuple, Type,
        class::{class_is_subclass, native_base},
    },
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

pub fn builtin_issubclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IssubclassArgs { cls, classinfo } = IssubclassArgs::from_args(args, vm)?;
    defer_drop!(cls, vm);
    defer_drop!(classinfo, vm);
    subclass_check(cls, classinfo, vm).map(Value::Bool)
}

#[derive(Clone, Copy)]
enum ClassKind {
    Builtin(Type),
    Exception(ExcType),
    Sandbox(HeapId),
    NamedTuple(HeapId),
    Host(HeapId),
}

fn class_kind(value: &Value, vm: &VM<'_>) -> Option<ClassKind> {
    match value {
        Value::Builtin(Builtins::Type(t)) => Some(ClassKind::Builtin(*t)),
        Value::Builtin(Builtins::ExcType(exc)) => Some(ClassKind::Exception(*exc)),
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => Some(ClassKind::Builtin(Type::Type)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Class(_) => Some(ClassKind::Sandbox(*id)),
            HeapData::NamedTupleClass(_) => Some(ClassKind::NamedTuple(*id)),
            HeapData::HostClassType(_) => Some(ClassKind::Host(*id)),
            _ => None,
        },
        _ => None,
    }
}

fn subclass_check(cls: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    // Check containers before validating cls: issubclass(42, ()) is False.
    if let Value::Ref(id) = classinfo {
        match vm.heap.read(*id) {
            HeapReadOutput::Tuple(tuple) => return check_tuple(cls, &tuple, vm),
            HeapReadOutput::Union(union) => {
                let args = union.get(vm.heap).args(vm.heap);
                defer_drop!(args, vm);
                let Some(HeapReadOutput::Tuple(members)) = args.read_heap(vm) else {
                    unreachable!("Union::args is always a tuple")
                };
                return check_tuple(cls, &members, vm);
            }
            _ => {}
        }
    }
    let cls = class_kind(cls, vm).ok_or_else(ExcType::issubclass_arg1_error)?;
    if matches!(classinfo, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::GenericAlias(_))) {
        return Err(ExcType::issubclass_parameterized_generic());
    }
    let parent = class_kind(classinfo, vm).ok_or_else(ExcType::issubclass_arg2_error)?;
    Ok(match (cls, parent) {
        (_, ClassKind::Builtin(Type::Object)) => true,
        (ClassKind::Builtin(cls), ClassKind::Builtin(parent)) => cls.is_instance_of(parent),
        (ClassKind::Exception(cls), ClassKind::Exception(parent)) => cls.is_subclass_of(parent),
        (ClassKind::Sandbox(cls), ClassKind::Sandbox(parent)) => class_is_subclass(cls, parent, vm),
        (ClassKind::Sandbox(cls), ClassKind::Builtin(parent)) => {
            native_base(cls, vm).is_some_and(|native| native.is_instance_of(parent))
        }
        (ClassKind::Sandbox(cls), ClassKind::Exception(parent)) => {
            matches!(native_base(cls,vm),Some(Type::Exception(exc)) if exc.is_subclass_of(parent))
        }
        (ClassKind::NamedTuple(_), ClassKind::Builtin(parent)) => Type::NamedTuple.is_instance_of(parent),
        (ClassKind::NamedTuple(cls), ClassKind::NamedTuple(parent))
        | (ClassKind::Host(cls), ClassKind::Host(parent)) => cls == parent,
        _ => false,
    })
}

fn check_tuple<'h>(cls: &Value, tuple: &HeapRead<'h, Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for index in 0..len {
        let parent = tuple.clone_item(index, vm);
        defer_drop!(parent, vm);
        if subclass_check(cls, parent, vm)? {
            return Ok(true);
        }
    }
    Ok(false)
}
