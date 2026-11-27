//! Namespace and attribute enumeration for `dir()`.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, HeapData},
    intern::StaticStrings,
    sorting::sort_values,
    types::{Dict, List, PyTrait, Type, TypeRef, instance::instance_call_dunder_sync, iter::collect_owned_iterable},
    value::{VALUE_SIZE, Value},
};

#[derive(FromArgs)]
#[from_args(name = "dir", style = unpack)]
struct DirArgs {
    #[from_args(pos_only, default)]
    object: Option<Value>,
}

/// Returns a sorted list of the caller's local names or the object's attribute names.
pub(crate) fn builtin_dir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DirArgs { object } = DirArgs::from_args(args, vm)?;
    if let Some(object) = object {
        defer_drop!(object, vm);
        if let Some(result) = call_dir_hook(object, vm)? {
            sorted_names(collect_owned_iterable(result, vm)?, false, vm)
        } else {
            let mut pending = DropGuard::new(Vec::new(), vm);
            let (names, vm) = pending.as_parts_mut();
            attribute_names(object, names, vm)?;
            let (names, vm) = pending.into_parts();
            sorted_names(names, true, vm)
        }
    } else {
        let locals = vm.locals_dict()?;
        sorted_names(collect_owned_iterable(locals, vm)?, false, vm)
    }
}

/// Looks up instance hooks on the class and module hooks in the module namespace.
fn call_dir_hook(object: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let Value::Ref(id) = object else { return Ok(None) };
    match vm.heap.get(*id) {
        HeapData::Instance(_) => instance_call_dunder_sync(*id, "__dir__", None, vm),
        HeapData::Module(module) => {
            let hook = module
                .namespace_attrs(vm.heap)
                .get_by_str("__dir__", vm.heap, vm.interns)
                .map(|hook| hook.clone_with_heap(vm.heap));
            if let Some(hook) = hook {
                defer_drop!(hook, vm);
                vm.evaluate_function(hook, ArgValues::Empty).map(Some)
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Enumerates stored names without resolving descriptors or lazy host attributes.
fn attribute_names(object: &Value, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    if let Value::Ref(id) = object {
        match vm.heap.get(*id) {
            HeapData::Module(module) => return dict_names(module.namespace_attrs(vm.heap), names, vm),
            HeapData::Instance(instance) => dict_names(instance.attrs(), names, vm)?,
            HeapData::HostClass(instance) => dict_names(instance.attrs(), names, vm)?,
            HeapData::NamedTuple(tuple) => {
                return if let Some(class) = tuple.class_id() {
                    type_names(TypeRef::Class(class), names, vm)
                } else {
                    for name in tuple.field_names() {
                        push_name(names, vm, || name.as_value(vm.heap))?;
                    }
                    type_names(Type::Tuple.into(), names, vm)
                };
            }
            _ => {}
        }
    }
    let ty = TypeRef::from_value(object, vm.heap).unwrap_or_else(|| object.py_type(vm));
    type_names(ty, names, vm)
}

/// Enumerates members defined on a class and its bases, not its metaclass.
fn type_names(ty: TypeRef, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    for ty in ty.mro(vm.heap) {
        match ty {
            TypeRef::Native(ty) => native_type_names(ty, names, vm)?,
            TypeRef::Class(id) => match vm.heap.get(id) {
                HeapData::Class(class) => dict_names(class.namespace(), names, vm)?,
                HeapData::HostClassType(class) => dict_names(class.attrs(), names, vm)?,
                HeapData::NamedTupleClass(class) => {
                    native_type_names(Type::NamedTuple, names, vm)?;
                    for name in class.field_names() {
                        push_name(names, vm, || name.as_value(vm.heap))?;
                    }
                    for name in [
                        StaticStrings::UnderFields,
                        StaticStrings::UnderFieldDefaults,
                        StaticStrings::MatchArgsDunder,
                        StaticStrings::DunderDoc,
                        StaticStrings::DunderModule,
                    ] {
                        static_name(name, names, vm)?;
                    }
                }
                _ => unreachable!("TypeRef::Class must refer to a class"),
            },
        }
    }
    Ok(())
}

fn native_type_names(ty: Type, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    if let Some(table) = ty.def().attrs {
        for &(name, _) in table.entries {
            static_name(name, names, vm)?;
        }
    }
    if ty.has_class_getitem() {
        static_name(StaticStrings::ClassGetitem, names, vm)?;
    }
    Ok(())
}

fn dict_names(dict: &Dict, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    for (key, _) in dict {
        push_name(names, vm, || key.clone_with_heap(vm.heap))?;
    }
    Ok(())
}

fn static_name(name: StaticStrings, names: &mut Vec<Value>, vm: &VM<'_>) -> RunResult<()> {
    push_name(names, vm, || Value::InternString(vm.interns.intern_static(name)))
}

fn push_name(names: &mut Vec<Value>, vm: &VM<'_>, name: impl FnOnce() -> Value) -> RunResult<()> {
    vm.heap
        .tracker
        .check_growth(names.len(), names.capacity(), VALUE_SIZE)?;
    names.push(name());
    Ok(())
}

fn sorted_names(names: Vec<Value>, dedup: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut pending = DropGuard::new(names, vm);
    let (names, vm) = pending.as_parts_mut();
    sort_values(names, None, false, vm)?;
    if dedup {
        names.dedup_by(|later, earlier| {
            if let Some(later_name) = later.as_str(vm)
                && Some(later_name) == earlier.as_str(vm)
            {
                mem::replace(later, Value::None).drop_with(vm);
                true
            } else {
                false
            }
        });
    }
    let (names, vm) = pending.into_parts();
    Ok(Value::allocate(List::new(names), vm.heap))
}
