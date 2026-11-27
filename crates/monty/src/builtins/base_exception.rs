//! `BaseException.__new__` and `BaseException.__init__`.

use std::mem;

use monty_macros::pyattrs;

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, ExceptionArgs, PyException, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapObjectRead, HeapReadOutput},
    py_attrs::PyAttrs,
    types::{OwnedTypeRef, Type, TypeRef, allocate_string},
    value::Value,
};

/// Attributes inherited from BaseException by exception classes.
pub(crate) struct PyBaseExceptionImpl;

#[pyattrs]
impl<'h> PyAttrs<'h> for PyBaseExceptionImpl {
    type Receiver = HeapObjectRead<'h, PyException>;

    #[pyvalue(DunderNew)]
    fn __new__(_vm: &mut VM<'_>) -> Value {
        Value::Builtin(Builtins::Function(BuiltinsFunctions::BaseExceptionNew))
    }

    #[pyvalue(DunderInit)]
    fn __init__(_vm: &mut VM<'_>) -> Value {
        Value::Builtin(Builtins::Function(BuiltinsFunctions::BaseExceptionInit))
    }
}

/// Arguments of `BaseException.__new__(cls, /, *args, **kwargs)`.
#[derive(FromArgs)]
#[from_args(name = "BaseException.__new__")]
pub(crate) struct BaseExceptionNewArgs {
    #[from_args(pos_only, default)]
    pub(crate) cls: Option<Value>,
    #[from_args(varargs)]
    pub(crate) args: Vec<Value>,
    #[from_args(varkwargs)]
    pub(crate) kwargs: KwargsValues,
}

/// Arguments of `BaseException.__init__(self, /, *args, **kwargs)`.
#[derive(FromArgs)]
#[from_args(name = "BaseException.__init__")]
pub(crate) struct BaseExceptionInitArgs {
    #[from_args(pos_only, default)]
    pub(crate) instance: Option<Value>,
    #[from_args(varargs)]
    pub(crate) args: Vec<Value>,
    #[from_args(varkwargs)]
    pub(crate) kwargs: KwargsValues,
}

/// Initializes an exception instance with positional and keyword arguments.
///
/// Matches CPython's per-type `__init__` rules:
/// - Subclasses of `AttributeError` accept keyword arguments `name` and `obj`.
/// - Subclasses of `NameError` accept keyword argument `name`.
/// - Subclasses of `ImportError` accept keyword arguments `name` and `path`.
/// - Subclasses of `StopIteration` populate `value = args[0]` (or `None`).
/// - Subclasses of `SystemExit` populate `code = args[0]` for 1 arg, or the `args` tuple for 2+ args.
/// - Other exception types reject keyword arguments.
pub(crate) fn init_exception_instance(
    heap_id: HeapId,
    args: Vec<Value>,
    kwargs: KwargsValues,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let (exc_type, type_name) = match vm.heap.get(heap_id) {
        HeapData::Exception(e) => {
            let name = match e.class {
                OwnedTypeRef::Native(ty) => ty.name().to_owned(),
                OwnedTypeRef::Class(id) => match vm.heap.get(id) {
                    HeapData::Class(c) => c.name().as_str(vm.interns).to_owned(),
                    _ => "Exception".to_owned(),
                },
            };
            (e.exc_type, name)
        }
        _ => unreachable!(),
    };

    let is_attr_err = Type::from(exc_type).is_subclass_of(Type::AttributeError);
    let is_name_err = Type::from(exc_type).is_subclass_of(Type::NameError);
    let is_import_err = Type::from(exc_type).is_subclass_of(Type::ImportError);
    let is_stop_iter = Type::from(exc_type).is_subclass_of(Type::StopIteration);
    let is_sys_exit = Type::from(exc_type).is_subclass_of(Type::SystemExit);

    let mut iter = kwargs.into_iter();
    let mut name_val: Option<Value> = None;
    let mut obj_val: Option<Value> = None;
    let mut path_val: Option<Value> = None;

    while let Some((key, val)) = iter.next() {
        let Some(s) = key.as_either_str(vm.heap) else {
            key.drop_with(vm);
            val.drop_with(vm);
            iter.drop_with(vm);
            args.drop_with(vm);
            if let Some(n) = name_val {
                n.drop_with(vm);
            }
            if let Some(o) = obj_val {
                o.drop_with(vm);
            }
            if let Some(p) = path_val {
                p.drop_with(vm);
            }
            return Err(ExcType::type_error("keywords must be strings"));
        };
        let key_str = s.as_str(vm.interns).to_owned();
        key.drop_with(vm);

        if (is_attr_err || is_name_err || is_import_err) && key_str == "name" {
            if let Some(old) = name_val.replace(val) {
                old.drop_with(vm);
            }
        } else if is_attr_err && key_str == "obj" {
            if let Some(old) = obj_val.replace(val) {
                old.drop_with(vm);
            }
        } else if is_import_err && key_str == "path" {
            if let Some(old) = path_val.replace(val) {
                old.drop_with(vm);
            }
        } else {
            val.drop_with(vm);
            iter.drop_with(vm);
            args.drop_with(vm);
            if let Some(n) = name_val {
                n.drop_with(vm);
            }
            if let Some(o) = obj_val {
                o.drop_with(vm);
            }
            if let Some(p) = path_val {
                p.drop_with(vm);
            }
            if is_attr_err || is_name_err || is_import_err {
                return Err(ExcType::type_error_unexpected_keyword(&type_name, &key_str));
            }
            return Err(ExcType::type_error_no_kwargs(&type_name));
        }
    }

    let new_args = ExceptionArgs::from_values(args, vm);
    let mut attrs_to_set = Vec::new();
    if is_attr_err {
        attrs_to_set.push(("name", name_val.unwrap_or(Value::None)));
        attrs_to_set.push(("obj", obj_val.unwrap_or(Value::None)));
    } else if is_name_err {
        attrs_to_set.push(("name", name_val.unwrap_or(Value::None)));
    } else if is_import_err {
        attrs_to_set.push(("name", name_val.unwrap_or(Value::None)));
        attrs_to_set.push(("path", path_val.unwrap_or(Value::None)));
    }
    if is_stop_iter {
        let first = match &new_args {
            ExceptionArgs::OneValue(v) => Some(v.clone_with_heap(vm.heap)),
            ExceptionArgs::Tuple(id) => match vm.heap.get(*id) {
                HeapData::Tuple(t) => t.as_slice().first().map(|v| v.clone_with_heap(vm.heap)),
                _ => None,
            },
            _ => None,
        };
        attrs_to_set.push(("value", first.unwrap_or(Value::None)));
    } else if is_sys_exit {
        let code = match &new_args {
            ExceptionArgs::Empty => None,
            ExceptionArgs::OneValue(v) => Some(v.clone_with_heap(vm.heap)),
            ExceptionArgs::OneStr(s) => Some(allocate_string(s.as_str(), vm.heap)),
            ExceptionArgs::Tuple(id) => {
                vm.heap.inc_ref(*id);
                Some(Value::Ref(*id))
            }
        };
        if let Some(v) = code {
            attrs_to_set.push(("code", v));
        } else {
            let has_code = match vm.heap.read(heap_id) {
                HeapReadOutput::Exception(exc) => exc
                    .get(vm.heap)
                    .attrs
                    .as_ref()
                    .is_some_and(|a| a.get_by_str("code", vm.heap, vm.interns).is_some()),
                _ => false,
            };
            if !has_code {
                attrs_to_set.push(("code", Value::None));
            }
        }
    }

    while !attrs_to_set.is_empty() {
        let (attr_name, val) = attrs_to_set.remove(0);
        let key = allocate_string(attr_name, vm.heap);
        let mut dict = match vm.heap.read(heap_id) {
            HeapReadOutput::Exception(exc) => exc.get_mut(vm.heap).attrs.take().unwrap_or_default(),
            _ => unreachable!(),
        };
        let set_result = dict.set(key, val, vm);
        match vm.heap.read(heap_id) {
            HeapReadOutput::Exception(exc) => exc.get_mut(vm.heap).attrs = Some(dict),
            _ => unreachable!(),
        }
        match set_result {
            Ok(Some(old)) => old.drop_with(vm),
            Ok(None) => {}
            Err(err) => {
                for (_, v) in attrs_to_set {
                    v.drop_with(vm);
                }
                new_args.drop_with(vm);
                return Err(err);
            }
        }
    }

    let old_args = match vm.heap.read(heap_id) {
        HeapReadOutput::Exception(exc) => mem::replace(&mut exc.get_mut(vm.heap).args, new_args),
        _ => unreachable!(),
    };
    old_args.drop_with(vm);

    Ok(())
}

/// `BaseException.__new__(cls, /, *args, **kwargs)`
pub(crate) fn builtin_base_exception_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let BaseExceptionNewArgs { cls, args, kwargs } = BaseExceptionNewArgs::from_args(args, vm)?;
    kwargs.drop_with(vm);

    let Some(cls) = cls else {
        args.drop_with(vm);
        return Err(ExcType::type_error_base_exception_new_not_enough_args());
    };
    defer_drop!(cls, vm);

    let Some(type_ref) = TypeRef::from_value(cls, vm.heap) else {
        let actual = cls.py_type_name(vm).into_owned();
        args.drop_with(vm);
        return Err(ExcType::type_error_base_exception_new_not_type(&actual));
    };

    let is_subclass = type_ref
        .mro(vm.heap)
        .any(|base| matches!(base, TypeRef::Native(Type::BaseException)));

    if !is_subclass {
        let name = type_ref.name(vm.heap, vm.interns).into_owned();
        args.drop_with(vm);
        return Err(ExcType::type_error_base_exception_new_not_subtype(&name));
    }

    let exc_type = type_ref
        .mro(vm.heap)
        .find_map(|t| match t {
            TypeRef::Native(ty) => ty.exc_type(),
            TypeRef::Class(_) => None,
        })
        .unwrap_or(ExcType::BaseException);

    let exc_args = ExceptionArgs::from_values(args, vm);
    let exc = PyException::from_parts(type_ref.to_owned(vm.heap), exc_type, exc_args);
    let heap_id = vm.heap.allocate(HeapData::Exception(Box::new(exc)));
    Ok(Value::Ref(heap_id))
}

/// `BaseException.__init__(self, /, *args, **kwargs)`
pub(crate) fn builtin_base_exception_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let BaseExceptionInitArgs { instance, args, kwargs } = BaseExceptionInitArgs::from_args(args, vm)?;

    let Some(instance) = instance else {
        args.drop_with(vm);
        kwargs.drop_with(vm);
        return Err(ExcType::type_error_descriptor_needs_arg("__init__", "BaseException"));
    };
    defer_drop!(instance, vm);

    let heap_id = match *instance {
        Value::Ref(id) if matches!(vm.heap.get(id), HeapData::Exception(_)) => id,
        _ => {
            args.drop_with(vm);
            kwargs.drop_with(vm);
            let actual = instance.py_type_name(vm).into_owned();
            return Err(ExcType::type_error_descriptor_requires(
                "__init__",
                "BaseException",
                &actual,
            ));
        }
    };

    init_exception_instance(heap_id, args, kwargs, vm)?;
    Ok(Value::None)
}
