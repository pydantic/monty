//! `BaseException.__new__` and `BaseException.__init__`.

use crate::{
    args::ArgValues,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, ExceptionArgs, PyException, RunResult},
    heap::{DropWithContext, HeapData, HeapReadOutput},
    types::{Type, TypeRef},
    value::{EitherStr, Value},
};

/// Resolves `BaseException`'s class attributes (`__new__`, `__init__`).
pub(crate) fn base_exception_class_attr(attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
    let function = match attr.as_str(vm.interns) {
        "__new__" => BuiltinsFunctions::BaseExceptionNew,
        "__init__" => BuiltinsFunctions::BaseExceptionInit,
        _ => return None,
    };
    Some(Value::Builtin(Builtins::Function(function)))
}

/// `BaseException.__new__(cls, /, *args, **kwargs)`
pub(crate) fn builtin_base_exception_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (pos_iter, kwargs) = args.into_parts();
    kwargs.drop_with(vm);

    let mut pos_vec: Vec<Value> = pos_iter.collect();
    if pos_vec.is_empty() {
        return Err(ExcType::type_error_base_exception_new_not_enough_args());
    }
    let cls_val = pos_vec.remove(0);
    defer_drop!(cls_val, vm);

    let type_ref = match TypeRef::from_value(cls_val, vm.heap) {
        Some(ty) => ty,
        None => {
            let actual = cls_val.py_type_name(vm).into_owned();
            pos_vec.drop_with(vm);
            return Err(ExcType::type_error_base_exception_new_not_type(&actual));
        }
    };

    let is_subclass = type_ref.mro(vm.heap).any(|base| matches!(base, TypeRef::Native(Type::BaseException)));

    if !is_subclass {
        let name = type_ref.name(vm.heap, vm.interns).into_owned();
        pos_vec.drop_with(vm);
        return Err(ExcType::type_error_base_exception_new_not_subtype(&name));
    }

    let exc_type = type_ref
        .mro(vm.heap)
        .find_map(|t| match t {
            TypeRef::Native(ty) => ty.exc_type(),
            _ => None,
        })
        .unwrap_or(ExcType::BaseException);

    let exc_args = ExceptionArgs::from_values(pos_vec, vm);
    let exc = PyException::from_parts(type_ref.to_owned(vm.heap), exc_type, exc_args);
    let heap_id = vm.heap.allocate(HeapData::Exception(Box::new(exc)));
    Ok(Value::Ref(heap_id))
}

/// `BaseException.__init__(self, /, *args, **kwargs)`
pub(crate) fn builtin_base_exception_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (pos_iter, kwargs) = args.into_parts();
    let mut pos_vec: Vec<Value> = pos_iter.collect();
    if pos_vec.is_empty() {
        kwargs.drop_with(vm);
        return Err(ExcType::type_error_descriptor_needs_arg("__init__", "BaseException"));
    }
    let instance = pos_vec.remove(0);
    defer_drop!(instance, vm);

    let heap_id = match *instance {
        Value::Ref(id) if matches!(vm.heap.get(id), HeapData::Exception(_)) => id,
        _ => {
            pos_vec.drop_with(vm);
            kwargs.drop_with(vm);
            let actual = instance.py_type_name(vm).into_owned();
            return Err(ExcType::type_error_descriptor_requires("__init__", "BaseException", &actual));
        }
    };

    if !kwargs.is_empty() {
        pos_vec.drop_with(vm);
        kwargs.drop_with(vm);
        let name = instance.py_type_name(vm).into_owned();
        return Err(ExcType::type_error_no_kwargs(&name));
    }

    let new_args = ExceptionArgs::from_values(pos_vec, vm);
    let old_args = match vm.heap.read(heap_id) {
        HeapReadOutput::Exception(exc) => {
            let exc_mut = exc.get_mut(vm.heap);
            std::mem::replace(&mut exc_mut.args, new_args)
        }
        _ => unreachable!(),
    };
    let mut old_args = old_args;
    old_args.drop_with(vm);

    Ok(Value::None)
}
