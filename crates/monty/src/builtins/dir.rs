//! Name enumeration for dir().

use super::{Builtins, BuiltinsFunctions, sorted::builtin_sorted};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::RunResult,
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, HeapData},
    modules::ModuleFunctions,
    resource_checks::check_estimated_size,
    types::{Class, Dict, Instance, List, Module, PyTrait, Set, Type, TypeRef, instance::class_dunder},
    value::{VALUE_SIZE, Value},
};

/// Both descriptor binding and the hook itself can suspend.
pub(crate) const HOOK_SOURCE: &str = r"
def _dir_hook(sort, hook, bind, obj, owner):
    if bind is not None:
        hook = bind(hook, obj, owner)
    return sort(hook())
";

/// Read an instance's namespace before walking the class MRO: its getter can mutate the class.
pub(crate) const CLASS_SOURCE: &str = r"
def _dir_class(obj, cls, get, is_instance, dict_type, sort):
    names = {'__class__': None, '__dict__': None}
    if obj is not None:
        namespace = get(obj, '__dict__', None)
        if is_instance(namespace, dict_type):
            names.update(namespace)
    for base in cls.__mro__:
        names.update(base.__dict__)
    return sort(names)
";

pub(super) fn builtin_dir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let Some(obj) = args.get_zero_one_arg("dir", vm.heap)? else {
        let locals = vm.locals_dict()?;
        return builtin_sorted(vm, ArgValues::One(locals)).map(CallResult::Value);
    };
    defer_drop!(obj, vm);
    let sort = Value::Builtin(Builtins::Function(BuiltinsFunctions::Sorted));
    if let Some(instance) = obj.get_as::<Instance>(vm.heap)
        && let Some(hook) = class_dunder(instance.class(), "__dir__", vm)
    {
        let owner = TypeRef::Class(instance.class()).to_value(vm.heap);
        let bind = Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::BindDescriptor));
        return vm.call_frozen_exact(
            FrozenFunction::DirHook,
            [
                sort,
                hook.clone_with_heap(vm.heap),
                bind,
                obj.clone_with_heap(vm.heap),
                owner,
            ],
        );
    }
    let sandbox_class = if let Some(instance) = obj.get_as::<Instance>(vm.heap) {
        Some((
            obj.clone_with_heap(vm.heap),
            TypeRef::Class(instance.class()).to_value(vm.heap),
        ))
    } else if obj.get_as::<Class>(vm.heap).is_some() {
        Some((Value::None, obj.clone_with_heap(vm.heap)))
    } else {
        None
    };
    if let Some((instance, class)) = sandbox_class {
        return vm.call_frozen_exact(
            FrozenFunction::DirClass,
            [
                instance,
                class,
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Getattr)),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)),
                Value::Builtin(Builtins::Type(Type::Dict)),
                sort,
            ],
        );
    }
    let mut guard = DropGuard::new(Vec::new(), vm);
    let (names, vm) = guard.as_parts_mut();
    if let Some(module) = obj.get_as::<Module>(vm.heap) {
        let attrs = module.members(vm)?;
        if let Some(hook) = attrs.get_by_str("__dir__", vm.heap, vm.interns) {
            return vm.call_frozen_exact(
                FrozenFunction::DirHook,
                [
                    sort,
                    hook.clone_with_heap(vm.heap),
                    Value::None,
                    Value::None,
                    Value::None,
                ],
            );
        }
        add_keys(names, attrs, vm)?;
    } else {
        object_names(obj, names, vm)?;
    }
    let (names, vm) = guard.into_parts();
    let list = Value::allocate(List::new(names), vm.heap);
    let set = Set::init(vm, ArgValues::One(list))?;
    builtin_sorted(vm, ArgValues::One(set)).map(CallResult::Value)
}

/// Only read names. Resolving a property here would run its getter.
fn object_names(obj: &Value, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    if let Some(id) = obj.ref_id() {
        match vm.heap.get(id) {
            HeapData::HostClass(instance) => add_keys(names, instance.attrs(), vm)?,
            HeapData::NamedTuple(tuple) => {
                extend_names(
                    names,
                    tuple.field_names().len(),
                    tuple.field_names().iter().map(|name| name.as_value(vm.heap)),
                    vm,
                )?;
            }
            _ => {}
        }
    }
    let class = TypeRef::from_value(obj, vm.heap).unwrap_or_else(|| obj.py_type(vm));
    for (index, ty) in class.mro(vm.heap).enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        match ty {
            TypeRef::Native(ty) => add_native(names, ty, vm)?,
            TypeRef::Class(id) => match vm.heap.get(id) {
                HeapData::Class(class) => add_keys(names, class.namespace(vm.heap), vm)?,
                HeapData::NamedTupleClass(class) => {
                    extend_names(
                        names,
                        class.field_names().len(),
                        class.field_names().iter().map(|name| name.as_value(vm.heap)),
                        vm,
                    )?;
                    add_native(names, Type::NamedTuple, vm)?;
                }
                HeapData::HostClassType(class) => add_keys(names, class.attrs(), vm)?,
                _ => unreachable!("type reference names a class"),
            },
        }
    }
    Ok(())
}

fn add_native(names: &mut Vec<Value>, ty: Type, vm: &VM<'_>) -> RunResult<()> {
    if let Some(attrs) = ty.def().attrs {
        extend_names(
            names,
            attrs.entries.len(),
            attrs
                .entries
                .iter()
                .map(|(name, _)| Value::InternString(vm.interns.intern_static(*name))),
            vm,
        )?;
    }
    Ok(())
}

fn add_keys(names: &mut Vec<Value>, dict: &Dict, vm: &VM<'_>) -> RunResult<()> {
    extend_names(
        names,
        dict.len(),
        dict.iter().map(|(key, _)| key.clone_with_heap(vm.heap)),
        vm,
    )
}

fn extend_names(names: &mut Vec<Value>, len: usize, values: impl Iterator<Item = Value>, vm: &VM<'_>) -> RunResult<()> {
    check_estimated_size(
        names.len().saturating_add(len).saturating_mul(VALUE_SIZE),
        &vm.heap.tracker,
    )?;
    for (i, value) in values.enumerate() {
        names.push(value);
        vm.heap.tracker.check_time_every(i)?;
    }
    Ok(())
}
