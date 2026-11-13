//! Descriptor lookup and suspendable continuations for sandbox classes.
//!
//! Reads prefer data descriptors with getters, then instance storage, then other class members.
//! Hooks belong to the descriptor's type, so changing that type can alter precedence without
//! writing to the owning class. Class hints must therefore account for possible future hooks.

use super::{
    Dict, Type,
    builtin_descriptor::{self, DescriptorMethod},
    instance::{bind_method, class_defines, class_dunder, class_member, is_method_value},
    tuple::allocate_tuple,
};
use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, Heap, HeapData, HeapId},
    modules::{ModuleFunctions, dataclasses},
    value::{EitherStr, Value},
};

/// Resolve an attribute and call it after any getter suspension.
pub(crate) const CALL_SOURCE: &str = r"
def _call(get, obj, name, args, kwargs):
    return get(obj, name)(*args, **kwargs)
";

/// Bind a selected member once, then call the getter's result without rebinding it.
pub(crate) const BIND_CALL_SOURCE: &str = r"
def _bind_call(bind, member, instance, owner, args, kwargs):
    return bind(member, instance, owner)(*args, **kwargs)
";

/// Ignore a hook's result while allowing it to suspend.
pub(crate) const DISCARD_SOURCE: &str = r"
def _discard(function, args):
    function(*args)
    return None
";

/// Notify descriptors in namespace order, looking up each hook when it runs.
pub(crate) const NAMES_SOURCE: &str = r"
def _names(cls, members, lookup):
    missing = []
    for descriptor, name in members:
        function = lookup(descriptor, missing)
        if function is not missing:
            function(cls, name)
    return cls
";

/// Bind a mutation hook and discard its result after completion.
pub(crate) const MUTATION_SOURCE: &str = r"
def _mutate(bind, function, descriptor, owner, obj, value, setting):
    bound = bind(function, descriptor, owner)
    if setting:
        bound(obj, value)
    else:
        bound(obj)
    return None
";

/// Apply getattr's default to AttributeError raised inside a suspended getter.
pub(crate) const GETATTR_SOURCE: &str = r"
def _getattr(get, obj, name, default, attribute_error):
    try:
        return get(obj, name)
    except attribute_error:
        return default
";

/// Return the value and match flag consumed by the MatchAttr continuation.
pub(crate) const MATCH_SOURCE: &str = r"
def _match(get, obj, name, attribute_error):
    try:
        return (get(obj, name), True)
    except attribute_error:
        return (None, False)
";

/// Read descriptor-backed pattern metadata before validating positional names.
pub(crate) const MATCH_CLASS_SOURCE: &str = r"
def _match_class(get, finish, cls, keywords, nargs, attribute_error):
    try:
        names = get(cls, '__match_args__')
    except attribute_error:
        names = ()
    return (finish(cls, keywords, nargs, names), True)
";

/// Catch AttributeError across a getter suspension for hasattr.
pub(crate) const HASATTR_SOURCE: &str = r"
def _hasattr(get, obj, name, attribute_error):
    try:
        get(obj, name)
        return True
    except attribute_error:
        return False
";

/// Private callable passed explicitly to frozen attribute continuations.
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
        HeapData::BuiltinDescriptor(d) => d.kind() == Type::Property,
        _ => false,
    })
}

/// Whether namespace values can be read without checking descriptor hooks.
pub(crate) fn class_descriptor_free(class: HeapId, vm: &VM<'_>) -> bool {
    matches!(vm.heap.get(class), HeapData::Class(class) if class.descriptor_free())
}

/// Selection without executing Python; the owning object keeps borrowed members alive.
pub(crate) enum AttributeResolution<'a> {
    /// Instance storage or a class value that needs no binding.
    Value(&'a Value),
    /// A native function that binds the instance when read or called.
    Method(&'a Value),
    /// A descriptor whose getter may suspend.
    Getter(&'a Value),
    /// The synthesized instance __class__ attribute.
    Class(HeapId),
    /// Neither storage nor the class provides this name.
    Missing,
}

impl AttributeResolution<'_> {
    /// Complete a selection that needs no Python execution, binding methods only on reads.
    pub(crate) fn into_value(self, instance: HeapId, vm: &VM<'_>) -> Option<Value> {
        match self {
            Self::Value(value) => Some(value.clone_with_heap(vm.heap)),
            Self::Method(value) => Some(bind_method(value, instance, vm)),
            Self::Class(class) => {
                vm.heap.inc_ref(class);
                Some(Value::Ref(class))
            }
            Self::Getter(_) | Self::Missing => None,
        }
    }
}

/// Apply read precedence once: data getter, instance storage, then the class member.
pub(crate) fn resolve_instance<'a>(id: HeapId, name: &str, vm: &'a VM<'_>) -> AttributeResolution<'a> {
    let HeapData::Instance(instance) = vm.heap.get(id) else {
        unreachable!("instance resolver requires an instance")
    };
    let class_id = instance.class();
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("instance owns its class")
    };
    let stored = instance.attrs().get_by_str(name, vm.heap, vm.interns);
    if (class.descriptor_free() || class.data_descriptor_free())
        && let Some(value) = stored
    {
        return AttributeResolution::Value(value);
    }
    let member = class_dunder(class_id, name, vm);
    let getter = !class.descriptor_free() && member.is_some_and(|member| has_get(member, vm));
    // A setter alone never overrides a read; only a data descriptor with a getter does.
    if getter
        && let Some(member) = member
        && is_data(member, vm)
    {
        return AttributeResolution::Getter(member);
    }
    if let Some(value) = stored {
        AttributeResolution::Value(value)
    } else {
        match member {
            Some(member) if getter => AttributeResolution::Getter(member),
            Some(member) if is_method_value(member, vm) => AttributeResolution::Method(member),
            Some(member) => AttributeResolution::Value(member),
            None if name == "__class__" => AttributeResolution::Class(class_id),
            None => AttributeResolution::Missing,
        }
    }
}

/// Class reads bind descriptors with no instance; ordinary functions stay unbound.
pub(crate) fn resolve_class<'a>(id: HeapId, name: &str, vm: &'a VM<'_>) -> AttributeResolution<'a> {
    match class_dunder(id, name, vm) {
        Some(member) if has_get(member, vm) => AttributeResolution::Getter(member),
        Some(member) => AttributeResolution::Value(member),
        None => AttributeResolution::Missing,
    }
}

/// Select a mutation descriptor for every write entry point, without executing its hook.
pub(crate) fn data_member(class: HeapId, name: &str, vm: &VM<'_>) -> Option<Value> {
    if class_descriptor_free(class, vm) {
        None
    } else {
        class_dunder(class, name, vm)
            .filter(|member| is_data(member, vm))
            .map(|member| member.clone_with_heap(vm.heap))
    }
}

/// The sandbox class whose namespace supplies an instance's descriptor hooks.
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
        d.kind() == Type::Property
    } else {
        // Presence, not callability, determines precedence: a broken setter is still a data descriptor.
        descriptor_class(value, vm)
            .is_some_and(|class| class_defines(class, "__set__", vm) || class_defines(class, "__delete__", vm))
    }
}

/// Builtin wrappers always have a getter; user hooks are looked up on their type.
pub(crate) fn has_get(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_)))
        || descriptor_class(value, vm).is_some_and(|class| class_defines(class, "__get__", vm))
}

/// A plain function binds an instance; arbitrary descriptors may install a Python frame.
pub(crate) fn get(value: &Value, instance: Option<HeapId>, owner: HeapId, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if let Value::Ref(id) = value
        && matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_))
    {
        builtin_descriptor::get(*id, instance, owner, vm)
    } else if let Some(class) = descriptor_class(value, vm)
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
        vm.call_function(getter, args)
    } else {
        Ok(CallResult::Value(match instance {
            Some(instance) if is_method_value(value, vm) => bind_method(value, instance, vm),
            _ => value.clone_with_heap(vm.heap),
        }))
    }
}

/// Arguments passed by frozen attribute wrappers; validate them like any callable boundary.
#[derive(FromArgs)]
#[from_args(name = "attribute lookup", style = unpack)]
struct LookupArgs {
    #[from_args(pos_only)]
    object: Value,
    #[from_args(pos_only)]
    name: Value,
}

/// Resolve an attribute for a frozen continuation, rejecting malformed helper arguments.
pub(crate) fn get_attribute(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let LookupArgs { object, name } = LookupArgs::from_args(args, vm)?;
    defer_drop!(object, vm);
    defer_drop!(name, vm);
    let Some(name) = name.as_either_str(vm.heap) else {
        return Err(ExcType::type_error("attribute name must be a string"));
    };
    let name = name.resolve_interned(vm.interns);
    object.py_getattr(&name, vm)
}

/// Preserve arguments while a getter runs, including across host calls and snapshots.
pub(crate) fn call_attr(object: Value, attr: &EitherStr, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let mut guard = DropGuard::new((object, args), vm);
    let (_, vm) = guard.as_parts_mut();
    let name = attr.as_value(vm.heap);
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

/// Keep call arguments alive in Python containers while a getter suspends.
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

/// Bind the member selected by a frozen continuation after checking its receiver and owner.
pub(crate) fn bind_descriptor(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let BindingArgs {
        member,
        instance,
        owner,
    } = BindingArgs::from_args(args, vm)?;
    defer_drop!(member, vm);
    defer_drop!(instance, vm);
    defer_drop!(owner, vm);
    let (Value::Ref(instance), Value::Ref(owner)) = (instance, owner) else {
        return Err(ExcType::type_error(
            "descriptor binding requires a member, instance and class",
        ));
    };
    if matches!(vm.heap.get(*instance), HeapData::Instance(_)) && matches!(vm.heap.get(*owner), HeapData::Class(_)) {
        get(member, Some(*instance), *owner, vm)
    } else {
        Err(ExcType::type_error("descriptor binding requires an instance and class"))
    }
}

/// Explicit frozen-helper arguments, with no references supplied through user globals.
#[derive(FromArgs)]
#[from_args(name = "descriptor binding", style = unpack)]
struct BindingArgs {
    #[from_args(pos_only)]
    member: Value,
    #[from_args(pos_only)]
    instance: Value,
    #[from_args(pos_only)]
    owner: Value,
}

/// Call a class-only special method, binding native functions directly and suspending getters.
pub(crate) fn call_member(member: &Value, instance: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if has_get(member, vm) {
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
    } else if is_method_value(member, vm) {
        vm.heap.inc_ref(instance);
        vm.call_function(member, args.prepend(Value::Ref(instance)))
    } else {
        vm.call_function(member, args)
    }
}

/// Execute a mutation hook as an ordinary frame and ignore its result.
pub(crate) fn discard_call(function: Value, args: Vec<Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let args = allocate_tuple(args.into(), vm.heap);
    vm.call_frozen_exact(FrozenFunction::DescriptorDiscard, [function, args])
}

/// Descriptor writes use a continuation because Python ignores a setter's return value.
pub(crate) fn set_attr(object: &Value, name: &EitherStr, value: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    defer_drop!(value, vm);
    if let Some(class) = descriptor_class(object, vm) {
        let key = name.as_value(vm.heap);
        defer_drop!(key, vm);
        if let Some(error) = dataclasses::frozen_assignment_error(class, key, vm) {
            return Err(error);
        }
    }
    if let Some(class) = descriptor_class(object, vm)
        && let Some(member) = data_member(class, name.as_str(vm.interns), vm)
    {
        defer_drop!(member, vm);
        return mutate(member, object, Some(value.clone_with_heap(vm.heap)), vm);
    }
    object.py_set_attr(name, value.clone_with_heap(vm.heap), vm)?;
    Ok(CallResult::Value(Value::None))
}

/// Resolve a data descriptor's mutation hook on its type and discard the hook's result.
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
        builtin_descriptor::mutate(*id, object, value, vm)
    } else {
        let name = if setting { "__set__" } else { "__delete__" };
        let (Value::Ref(instance), Some(class)) = (descriptor, descriptor_class(descriptor, vm)) else {
            return Err(ExcType::type_error("mutation requires a data descriptor"));
        };
        let Some(function) = class_member(class, name, vm) else {
            return Err(ExcType::attribute_error_descriptor_hook(name));
        };
        let (value, vm) = guard.into_parts();
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
    let NameHookArgs { descriptor, missing } = NameHookArgs::from_args(args, vm)?;
    defer_drop!(descriptor, vm);
    defer_drop!(missing, vm);
    if let Value::Ref(id) = descriptor
        && let HeapData::BuiltinDescriptor(d) = vm.heap.get(*id)
        && d.kind() == Type::Property
    {
        let function = Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Descriptor(
            DescriptorMethod::SetName,
        )));
        Ok(CallResult::Value(bind_method(&function, *id, vm)))
    } else if let Value::Ref(instance) = descriptor
        && let Some(owner) = descriptor_class(descriptor, vm)
        && let Some(function) = class_member(owner, "__set_name__", vm)
    {
        defer_drop!(function, vm);
        get(function, Some(*instance), owner, vm)
    } else {
        Ok(CallResult::Value(missing.clone_with_heap(vm.heap)))
    }
}

/// The next namespace descriptor and the caller's unique missing sentinel.
#[derive(FromArgs)]
#[from_args(name = "descriptor name hook", style = unpack)]
struct NameHookArgs {
    #[from_args(pos_only)]
    descriptor: Value,
    #[from_args(pos_only)]
    missing: Value,
}

/// Include user instances whose type may gain a hook during an earlier notification.
fn may_have_name_hook(value: &Value, vm: &VM<'_>) -> bool {
    descriptor_class(value, vm).is_some()
        || matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(d) if d.kind() == Type::Property))
}
