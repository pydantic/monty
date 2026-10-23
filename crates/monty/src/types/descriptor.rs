//! Descriptor lookup and suspendable continuations for sandbox classes.
//!
//! Reads prefer data descriptors with getters, then instance storage, then other class members.
//! Hooks belong to the descriptor's type, so changing that type can alter precedence without
//! writing to the owning class. Class hints must therefore account for possible future hooks.

use super::{
    Dict, Type, attribute_name_value,
    builtin_descriptor::{self, DescriptorMethod},
    instance::{bind_method, class_defines, class_dunder, class_member, is_method_value},
    tuple::allocate_tuple,
};
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, Heap, HeapData, HeapId},
    modules::{ModuleFunctions, dataclasses},
    value::{EitherStr, Value},
};

pub(crate) const CALL_SOURCE: &str = r"
def _call(get, obj, name, args, kwargs):
    return get(obj, name)(*args, **kwargs)
";
pub(crate) const BIND_CALL_SOURCE: &str = r"
def _bind_call(bind, member, instance, owner, args, kwargs):
    return bind(member, instance, owner)(*args, **kwargs)
";
pub(crate) const DISCARD_SOURCE: &str = r"
def _discard(function, args):
    function(*args)
    return None
";
pub(crate) const NAMES_SOURCE: &str = r"
def _names(cls, members, lookup):
    missing = []
    for descriptor, name in members:
        function = lookup(descriptor, missing)
        if function is not missing:
            function(cls, name)
    return cls
";
pub(crate) const MUTATION_SOURCE: &str = r"
def _mutate(bind, function, descriptor, owner, obj, value, setting):
    bound = bind(function, descriptor, owner)
    if setting:
        bound(obj, value)
    else:
        bound(obj)
    return None
";
pub(crate) const GETATTR_SOURCE: &str = r"
def _getattr(get, obj, name, default, attribute_error):
    try:
        return get(obj, name)
    except attribute_error:
        return default
";
pub(crate) const MATCH_SOURCE: &str = r"
def _match(get, obj, name, attribute_error):
    try:
        return (get(obj, name), True)
    except attribute_error:
        return (None, False)
";
pub(crate) const MATCH_CLASS_SOURCE: &str = r"
def _match_class(get, finish, cls, keywords, nargs, attribute_error):
    try:
        names = get(cls, '__match_args__')
    except attribute_error:
        names = ()
    return (finish(cls, keywords, nargs, names), True)
";
pub(crate) const HASATTR_SOURCE: &str = r"
def _hasattr(get, obj, name, attribute_error):
    try:
        get(obj, name)
        return True
    except attribute_error:
        return False
";

pub(crate) fn attribute_helper() -> Value {
    Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::GetAttribute))
}

/// Instances remain candidates even before their class defines descriptor hooks.
pub(crate) fn may_be_descriptor(value: &Value, heap: &Heap) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::Instance(_) | HeapData::BuiltinDescriptor(_)))
}

/// User-defined members may gain setter/deleter hooks through mutations of their type.
/// Builtin staticmethod/classmethod objects cannot gain those hooks; property is always data.
pub(crate) fn may_be_data_descriptor(value: &Value, heap: &Heap) -> bool {
    matches!(value, Value::Ref(id) if match heap.get(*id) {
        HeapData::Instance(_) => true,
        HeapData::BuiltinDescriptor(d) => d.kind == Type::Property,
        _ => false,
    })
}

pub(crate) fn class_descriptor_free(class: HeapId, vm: &VM<'_>) -> bool {
    matches!(vm.heap.get(class), HeapData::Class(class) if class.descriptor_free())
}

/// Whether resolving this particular attribute can execute a descriptor getter.
pub(crate) fn attribute_uses_descriptor(object: &Value, name: &EitherStr, vm: &VM<'_>) -> bool {
    let Value::Ref(id) = object else { return false };
    let name = name.as_str(vm.interns);
    match vm.heap.get(*id) {
        HeapData::Class(class) => {
            name != "__name__"
                && !class.descriptor_free()
                && class_dunder(*id, name, vm).is_some_and(|member| has_get(member, vm))
        }
        HeapData::Instance(instance) => {
            let class = instance.class();
            if class_descriptor_free(class, vm) {
                return false;
            }
            let member = class_dunder(class, name, vm);
            // Only a data getter can defeat an instance hit; other getters run on a miss.
            if member.is_some_and(|member| is_data(member, vm) && has_get(member, vm)) {
                return true;
            }
            instance.attrs().get_by_str(name, vm.heap, vm.interns).is_none()
                && member.is_some_and(|member| has_get(member, vm))
        }
        _ => false,
    }
}

pub(crate) fn descriptor_class(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Instance(instance) => Some(instance.class()),
            _ => None,
        },
        _ => None,
    }
}

/// The setter/deleter is looked up on the descriptor's type, never its instance.
pub(crate) fn is_data(value: &Value, vm: &VM<'_>) -> bool {
    if let Value::Ref(id) = value
        && let HeapData::BuiltinDescriptor(d) = vm.heap.get(*id)
    {
        return d.kind == Type::Property;
    }
    // Presence, not callability, determines precedence: a broken setter is still a data descriptor.
    descriptor_class(value, vm)
        .is_some_and(|class| class_defines(class, "__set__", vm) || class_defines(class, "__delete__", vm))
}

pub(crate) fn has_get(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_)))
        || descriptor_class(value, vm).is_some_and(|class| class_defines(class, "__get__", vm))
}

/// A plain function binds an instance; arbitrary descriptors may install a Python frame.
pub(crate) fn get(value: &Value, instance: Option<HeapId>, owner: HeapId, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if let Value::Ref(id) = value
        && matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_))
    {
        return builtin_descriptor::get(*id, instance, owner, vm);
    }
    if let Some(class) = descriptor_class(value, vm)
        && let Some(getter) = class_member(class, "__get__", vm)
    {
        defer_drop!(getter, vm);
        let receiver = instance.map_or(Value::None, |id| {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        });
        vm.heap.inc_ref(owner);
        let args = ArgValues::ArgsKargs {
            args: vec![value.clone_with_heap(vm.heap), receiver, Value::Ref(owner)],
            kwargs: KwargsValues::Empty,
        };
        return vm.call_function(getter, args);
    }
    Ok(CallResult::Value(match instance {
        Some(instance) if is_method_value(value, vm) => bind_method(value, instance, vm),
        _ => value.clone_with_heap(vm.heap),
    }))
}

pub(crate) fn get_attribute(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let positional = args.into_pos_only("attribute lookup", vm.heap)?;
    defer_drop!(positional, vm);
    let [object, name] = positional.as_slice() else {
        unreachable!("private lookup signature")
    };
    let name = name
        .as_either_str(vm.heap)
        .expect("attribute name is a string")
        .resolve_interned(vm.interns);
    object.py_getattr(&name, vm)
}

/// Preserve arguments while a getter runs, including across host calls and snapshots.
pub(crate) fn call_attr(object: Value, attr: &EitherStr, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let mut guard = DropGuard::new((object, args), vm);
    let (_, vm) = guard.as_parts_mut();
    let name = attribute_name_value(attr, vm);
    let ((object, args), vm) = guard.into_parts();
    let mut guard = DropGuard::new([object, name], vm);
    let (_, vm) = guard.as_parts_mut();
    let (pos, kwargs) = pack_args(args, vm)?;
    let ([object, name], vm) = guard.into_parts();
    vm.call_frozen_exact(
        FrozenFunction::DescriptorCall,
        [attribute_helper(), object, name, pos, kwargs],
    )
}

fn pack_args(args: ArgValues, vm: &mut VM<'_>) -> RunResult<(Value, Value)> {
    let (pos, kwargs) = args.into_parts();
    let pos = allocate_tuple(pos.collect(), vm.heap);
    defer_drop!(pos, vm);
    let pairs = match kwargs {
        KwargsValues::Empty => vec![],
        KwargsValues::Pairs(pairs) => pairs,
        KwargsValues::Dict(dict) => dict.into_iter().collect(),
        KwargsValues::Inline(pairs) => pairs
            .into_iter()
            .map(|(name, value)| (Value::InternString(name), value))
            .collect(),
    };
    let kwargs = Dict::from_pairs(pairs, vm)?;
    let pos = pos.clone_with_heap(vm.heap);
    let kwargs = Value::Ref(vm.heap.allocate(HeapData::Dict(kwargs)));
    Ok((pos, kwargs))
}

pub(crate) fn bind_descriptor(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let args = args.into_pos_only("descriptor binding", vm.heap)?;
    defer_drop!(args, vm);
    let [member, Value::Ref(instance), Value::Ref(owner)] = args.as_slice() else {
        unreachable!("private binding signature")
    };
    get(member, Some(*instance), *owner, vm)
}

pub(crate) fn call_member(member: &Value, instance: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if !has_get(member, vm) {
        return if is_method_value(member, vm) {
            vm.heap.inc_ref(instance);
            vm.call_function(member, args.prepend(Value::Ref(instance)))
        } else {
            vm.call_function(member, args)
        };
    }
    let (args, kwargs) = pack_args(args, vm)?;
    let HeapData::Instance(object) = vm.heap.get(instance) else {
        unreachable!("special method receiver is an instance")
    };
    let owner = object.class();
    vm.heap.inc_ref(instance);
    vm.heap.inc_ref(owner);
    vm.call_frozen_exact(
        FrozenFunction::DescriptorBindCall,
        [
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::BindDescriptor)),
            member.clone_with_heap(vm.heap),
            Value::Ref(instance),
            Value::Ref(owner),
            args,
            kwargs,
        ],
    )
}

pub(crate) fn discard_call(function: Value, args: Vec<Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let args = allocate_tuple(args.into(), vm.heap);
    vm.call_frozen_exact(FrozenFunction::DescriptorDiscard, [function, args])
}

/// Descriptor writes use a continuation because Python ignores a setter's return value.
pub(crate) fn set_attr(object: &Value, name: &EitherStr, value: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    defer_drop!(value, vm);
    if let Some(class) = descriptor_class(object, vm) {
        let key = attribute_name_value(name, vm);
        defer_drop!(key, vm);
        if let Some(error) = dataclasses::frozen_assignment_error(class, key, vm) {
            return Err(error);
        }
    }
    if let Some(class) = descriptor_class(object, vm)
        && let Some(member) = class_member(class, name.as_str(vm.interns), vm)
    {
        defer_drop!(member, vm);
        if is_data(member, vm) {
            return mutate(member, object, Some(value.clone_with_heap(vm.heap)), vm);
        }
    }
    object.py_set_attr(name, value.clone_with_heap(vm.heap), vm)?;
    Ok(CallResult::Value(Value::None))
}

pub(crate) fn mutate(
    descriptor: &Value,
    object: &Value,
    value: Option<Value>,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let setting = value.is_some();
    let mut guard = DropGuard::new(value, vm);
    let (_, vm) = guard.as_parts_mut();
    if let Value::Ref(id) = descriptor
        && matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_))
    {
        let (value, vm) = guard.into_parts();
        return builtin_descriptor::mutate(*id, object, value, vm);
    }
    let name = if setting { "__set__" } else { "__delete__" };
    let class = descriptor_class(descriptor, vm).expect("data descriptor has a class");
    let Some(function) = class_member(class, name, vm) else {
        return Err(ExcType::attribute_error("descriptor", name));
    };
    let (value, vm) = guard.into_parts();
    let Value::Ref(instance) = descriptor else {
        unreachable!()
    };
    vm.heap.inc_ref(*instance);
    vm.heap.inc_ref(class);
    vm.call_frozen_exact(
        FrozenFunction::DescriptorMutation,
        [
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::BindDescriptor)),
            function,
            Value::Ref(*instance),
            Value::Ref(class),
            object.clone_with_heap(vm.heap),
            value.unwrap_or(Value::None),
            Value::Bool(setting),
        ],
    )
}

/// Notify the original namespace's descriptors, resolving each hook when it runs.
pub(crate) fn finish_class(class: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    defer_drop!(class, vm);
    let Value::Ref(id) = class else { unreachable!() };
    let HeapData::Class(cls) = vm.heap.get(*id) else {
        unreachable!()
    };
    let members = cls
        .namespace()
        .iter()
        .filter(|(_, member)| may_have_name_hook(member, vm))
        .map(|(name, member)| {
            allocate_tuple(
                vec![member.clone_with_heap(vm.heap), name.clone_with_heap(vm.heap)].into(),
                vm.heap,
            )
        })
        .collect::<Vec<_>>();
    if members.is_empty() {
        return Ok(CallResult::Value(class.clone_with_heap(vm.heap)));
    }
    let members = allocate_tuple(members.into(), vm.heap);
    vm.call_frozen_exact(
        FrozenFunction::DescriptorNames,
        [
            class.clone_with_heap(vm.heap),
            members,
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::SetNameLookup)),
        ],
    )
}

/// Earlier name hooks may replace a later descriptor type's hook.
pub(crate) fn lookup_set_name(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let args = args.into_pos_only("descriptor name hook", vm.heap)?;
    defer_drop!(args, vm);
    let [descriptor, missing] = args.as_slice() else {
        unreachable!("private name hook signature")
    };
    if let Value::Ref(id) = descriptor
        && let HeapData::BuiltinDescriptor(d) = vm.heap.get(*id)
        && d.kind == Type::Property
    {
        let function = Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Descriptor(
            DescriptorMethod::SetName,
        )));
        return Ok(CallResult::Value(bind_method(&function, *id, vm)));
    }
    if let Some(owner) = descriptor_class(descriptor, vm)
        && let Some(function) = class_member(owner, "__set_name__", vm)
    {
        defer_drop!(function, vm);
        let Value::Ref(instance) = descriptor else {
            unreachable!()
        };
        return get(function, Some(*instance), owner, vm);
    }
    Ok(CallResult::Value(missing.clone_with_heap(vm.heap)))
}

fn may_have_name_hook(value: &Value, vm: &VM<'_>) -> bool {
    descriptor_class(value, vm).is_some()
        || matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(d) if d.kind == Type::Property))
}
