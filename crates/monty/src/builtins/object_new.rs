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
    types::{Dict, Instance, Type, allocate_tuple},
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
        Value::Builtin(Builtins::ExcType(exc_type)) => {
            let arg_values = ArgValues::from_parts(extra_args, KwargsValues::Empty);
            exc_type.call(vm, arg_values)
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
            let HeapData::Class(cls) = vm.heap.get(*class_id) else {
                extra_args.drop_with(vm.heap);
                let type_name = cls_val.py_type_name(vm);
                return Err(ExcType::type_error(format!(
                    "object.__new__(X): X is not a type object ({type_name})"
                )));
            };

            let base_exc = cls.base_exception();
            if base_exc.is_none() && (!extra_args.is_empty() || has_kwargs) {
                extra_args.drop_with(vm.heap);
                return Err(ExcType::type_error(
                    "object.__new__() takes exactly one argument (the type to instantiate)",
                ));
            }

            let attrs = if base_exc.is_some() {
                let args_tuple = allocate_tuple(extra_args.into(), vm.heap);
                Dict::from_pairs(
                    vec![(
                        Value::InternString(vm.interns.intern_static(StaticStrings::Args)),
                        args_tuple,
                    )],
                    vm,
                )?
            } else {
                extra_args.drop_with(vm.heap);
                Dict::new()
            };

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
