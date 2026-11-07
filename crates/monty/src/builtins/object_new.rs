//! Implementation of `object.__new__`, the instance allocator.

use monty_types::ExcType;

use crate::{
    args::{ArgValues, KwargsValues},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData},
    intern::StaticStrings,
    types::{BuiltinType, Dict, Instance, Type, allocate_tuple, class::ClassBase},
    value::Value,
};

/// `object.__new__(cls, *args, **kwargs)` — allocates a new empty instance of `cls`.
pub fn builtin_object_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let has_kwargs = !kwargs.is_empty();
    kwargs.drop_with(vm.heap);
    let Some(cls_val) = pos.next() else {
        return Err(ExcType::type_error("object.__new__(): not enough arguments"));
    };
    defer_drop!(cls_val, vm);
    let extra_args: Vec<Value> = pos.collect();

    match cls_val {
        Value::Builtin(Builtins::Type(BuiltinType::Object)) => {
            extra_args.drop_with(vm.heap);
            Err(ExcType::type_error("cannot create 'object' instances"))
        }
        Value::Builtin(Builtins::ExcType(exc_type)) => {
            extra_args.drop_with(vm.heap);
            let name: &'static str = (*exc_type).into();
            let bare = name.rsplit_once('.').map_or(name, |(_, b)| b);
            Err(ExcType::type_error(format!(
                "object.__new__({bare}) is not safe, use {bare}.__new__()"
            )))
        }
        Value::Builtin(Builtins::Type(t)) => {
            extra_args.drop_with(vm.heap);
            let name = Type::from(*t).dunder_name(vm.heap, vm.interns);
            Err(ExcType::type_error(format!(
                "object.__new__({name}) is not safe, use {name}.__new__()"
            )))
        }
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => {
            extra_args.drop_with(vm.heap);
            Err(ExcType::type_error(
                "object.__new__(type) is not safe, use type.__new__()",
            ))
        }
        Value::Ref(class_id) => {
            match vm.heap.get(*class_id) {
                HeapData::NamedTupleClass(nt) => {
                    let name = nt.name(vm.interns).to_owned();
                    extra_args.drop_with(vm.heap);
                    return Err(ExcType::type_error(format!(
                        "object.__new__({name}) is not safe, use tuple.__new__()"
                    )));
                }
                HeapData::HostClassType(ty) => {
                    let name = ty.name(vm.interns).to_owned();
                    extra_args.drop_with(vm.heap);
                    return Err(ExcType::type_error(format!(
                        "object.__new__({name}) is not safe, use {name}.__new__()"
                    )));
                }
                _ => {}
            }
            let HeapData::Class(cls) = vm.heap.get(*class_id) else {
                extra_args.drop_with(vm.heap);
                let type_name = cls_val.py_type_name(vm);
                return Err(ExcType::type_error(format!(
                    "object.__new__(X): X is not a type object ({type_name})"
                )));
            };

            if cls.is_exception_class() {
                let name = cls.name().as_str(vm.interns).to_owned();
                extra_args.drop_with(vm.heap);
                return Err(ExcType::type_error(format!(
                    "object.__new__({name}) is not safe, use {name}.__new__()"
                )));
            }

            let has_excess = !extra_args.is_empty() || has_kwargs;
            if has_excess {
                let overrides_new = cls.overrides_new(vm.heap, vm.interns);
                let overrides_init = cls.overrides_init(vm.heap, vm.interns);
                if overrides_new {
                    extra_args.drop_with(vm.heap);
                    return Err(ExcType::type_error(
                        "object.__new__() takes exactly one argument (the type to instantiate)",
                    ));
                } else if !overrides_init {
                    let name = cls.name().as_str(vm.interns).to_owned();
                    extra_args.drop_with(vm.heap);
                    return Err(ExcType::type_error(format!("{name}() takes no arguments")));
                }
            }

            extra_args.drop_with(vm.heap);
            let attrs = Dict::new();
            vm.heap.inc_ref(*class_id);
            let inst = Instance::new(*class_id, attrs);
            let inst_id = vm.heap.allocate(HeapData::Instance(Box::new(inst)));
            Ok(Value::Ref(inst_id))
        }
        _ => {
            extra_args.drop_with(vm.heap);
            let type_name = cls_val.py_type_name(vm);
            Err(ExcType::type_error(format!(
                "object.__new__(X): X is not a type object ({type_name})"
            )))
        }
    }
}

/// `ExcType.__new__(cls, *args, **kwargs)` — allocates a new exception instance of `cls`.
///
/// Always bound to an `owner_exc` (`Value::Builtin(Builtins::ExcType(_))`) prepended as the
/// first argument so subtype validation matches CPython's `tp_new_wrapper`.
pub fn builtin_exception_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    kwargs.drop_with(vm.heap);
    let Some(owner_val) = pos.next() else {
        return Err(ExcType::type_error("BaseException.__new__(): not enough arguments"));
    };
    defer_drop!(owner_val, vm);
    let Value::Builtin(Builtins::ExcType(owner_exc)) = *owner_val else {
        pos.drop_with(vm.heap);
        return Err(ExcType::type_error("BaseException.__new__(): invalid owner"));
    };
    let owner_str: &'static str = owner_exc.into();
    let owner_name = owner_str.rsplit_once('.').map_or(owner_str, |(_, b)| b);

    let Some(cls_val) = pos.next() else {
        return Err(ExcType::type_error(format!(
            "{owner_name}.__new__(): not enough arguments"
        )));
    };
    defer_drop!(cls_val, vm);
    let extra_args: Vec<Value> = pos.collect();

    match cls_val {
        Value::Builtin(Builtins::ExcType(exc_type)) => {
            if !exc_type.is_subclass_of(owner_exc) {
                extra_args.drop_with(vm.heap);
                let name: &'static str = (*exc_type).into();
                let bare = name.rsplit_once('.').map_or(name, |(_, b)| b);
                return Err(ExcType::type_error(format!(
                    "{owner_name}.__new__({bare}): {bare} is not a subtype of {owner_name}"
                )));
            }
            let arg_values = ArgValues::from_parts(extra_args, KwargsValues::Empty);
            exc_type.call(vm, arg_values)
        }
        Value::Ref(class_id) => match vm.heap.get(*class_id) {
            HeapData::Class(cls) => {
                let is_sub = cls
                    .mro()
                    .iter()
                    .any(|b| matches!(b, ClassBase::BuiltinExc(e) if e.is_subclass_of(owner_exc)));
                if !is_sub {
                    let target_name = cls.name().as_str(vm.interns).to_owned();
                    extra_args.drop_with(vm.heap);
                    return Err(ExcType::type_error(format!(
                        "{owner_name}.__new__({target_name}): {target_name} is not a subtype of {owner_name}"
                    )));
                }

                let args_tuple = allocate_tuple(extra_args.into(), vm.heap);
                let attrs = Dict::from_pairs(
                    vec![(
                        Value::InternString(vm.interns.intern_static(StaticStrings::Args)),
                        args_tuple,
                    )],
                    vm,
                )?;

                vm.heap.inc_ref(*class_id);
                let inst = Instance::new(*class_id, attrs);
                let inst_id = vm.heap.allocate(HeapData::Instance(Box::new(inst)));
                Ok(Value::Ref(inst_id))
            }
            HeapData::NamedTupleClass(nt) => {
                let target_name = nt.name(vm.interns).to_owned();
                extra_args.drop_with(vm.heap);
                Err(ExcType::type_error(format!(
                    "{owner_name}.__new__({target_name}): {target_name} is not a subtype of {owner_name}"
                )))
            }
            HeapData::HostClassType(ty) => {
                let target_name = ty.name(vm.interns).to_owned();
                extra_args.drop_with(vm.heap);
                Err(ExcType::type_error(format!(
                    "{owner_name}.__new__({target_name}): {target_name} is not a subtype of {owner_name}"
                )))
            }
            _ => {
                extra_args.drop_with(vm.heap);
                let type_name = cls_val.py_type_name(vm);
                Err(ExcType::type_error(format!(
                    "{owner_name}.__new__(X): X is not a type object ({type_name})"
                )))
            }
        },
        Value::Builtin(Builtins::Type(t)) => {
            let target_name = Type::from(*t).dunder_name(vm.heap, vm.interns).into_owned();
            extra_args.drop_with(vm.heap);
            Err(ExcType::type_error(format!(
                "{owner_name}.__new__({target_name}): {target_name} is not a subtype of {owner_name}"
            )))
        }
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => {
            extra_args.drop_with(vm.heap);
            Err(ExcType::type_error(format!(
                "{owner_name}.__new__(type): type is not a subtype of {owner_name}"
            )))
        }
        _ => {
            extra_args.drop_with(vm.heap);
            let type_name = cls_val.py_type_name(vm);
            Err(ExcType::type_error(format!(
                "{owner_name}.__new__(X): X is not a type object ({type_name})"
            )))
        }
    }
}
