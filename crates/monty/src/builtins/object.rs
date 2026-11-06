//! The members `object` provides to every class: `__new__`, `__init__`,
//! `__init_subclass__` and `__setattr__`.
//!
//! They are reached as `object.__init__`, through a class that defines no
//! override (`Foo.__init__`), and through `super()`.

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropWithContext, HeapData, HeapId},
    types::{Dict, Instance, PyTrait, Type, TypeRef, instance::class_defines},
    value::{EitherStr, Value},
};

/// `object`'s class attributes, for the native type table.
pub(crate) fn object_class_attr(attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
    let function = match attr.as_str(vm.interns) {
        // Exists so a class that hooks attribute writes has a way to perform
        // one (see `limitations/classes.md`).
        "__setattr__" => BuiltinsFunctions::ObjectSetattr,
        "__new__" => BuiltinsFunctions::ObjectNew,
        "__init__" => BuiltinsFunctions::ObjectInit,
        "__init_subclass__" => BuiltinsFunctions::ObjectInitSubclass,
        _ => return None,
    };
    Some(Value::Builtin(Builtins::Function(function)))
}

/// Arguments of `object.__new__(cls, /, *args, **kwargs)`.
#[derive(FromArgs)]
#[from_args(name = "object.__new__")]
struct ObjectNewArgs {
    #[from_args(pos_only, default)]
    cls: Option<Value>,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwargs: KwargsValues,
}

/// `object.__new__(cls, *args, **kwargs)`: a new, empty instance of the class `cls`.
///
/// Extra arguments are an error unless the class overrides `__init__` and not
/// `__new__`, CPython's `object_new` rule. Native classes have their own
/// `__new__`, and `object()` itself cannot be built (see `limitations/classes.md`).
pub(crate) fn builtin_object_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ObjectNewArgs { cls, args, kwargs } = ObjectNewArgs::from_args(args, vm)?;
    let excess = !args.is_empty() || !kwargs.is_empty();
    args.drop_with(vm);
    kwargs.drop_with(vm);
    let Some(cls) = cls else {
        return Err(ExcType::type_error("object.__new__(): not enough arguments"));
    };
    defer_drop!(cls, vm);
    let class_id = match TypeRef::from_value(cls, vm.heap) {
        Some(TypeRef::Class(id)) if matches!(vm.heap.get(id), HeapData::Class(_)) => id,
        Some(TypeRef::Native(Type::Object)) => {
            return Err(ExcType::type_error("cannot create 'object' instances"));
        }
        Some(ty) => {
            // CPython names the nearest native base, whose own `__new__` builds the layout.
            let name = ty.dunder_name(vm.heap, vm.interns);
            let base = match ty {
                // A host class stands in for the host's real class, whose own `__new__` it names.
                TypeRef::Class(id) if matches!(vm.heap.get(id), HeapData::HostClassType(_)) => name.clone(),
                _ => ty
                    .mro(vm.heap)
                    .find_map(TypeRef::native)
                    .map_or("object", Type::dunder_name)
                    .into(),
            };
            return Err(ExcType::type_error(format!(
                "object.__new__({name}) is not safe, use {base}.__new__()"
            )));
        }
        None => {
            return Err(ExcType::type_error(format!(
                "object.__new__(X): X is not a type object ({})",
                cls.py_type_name(vm)
            )));
        }
    };
    if excess {
        if class_defines(class_id, "__new__", vm) {
            return Err(ExcType::type_error(
                "object.__new__() takes exactly one argument (the type to instantiate)",
            ));
        }
        if !class_defines(class_id, "__init__", vm) {
            return Err(takes_no_arguments(class_id, vm));
        }
    }
    vm.heap.inc_ref(class_id);
    let instance = Instance::new(class_id, Dict::new());
    Ok(Value::Ref(vm.heap.allocate(HeapData::Instance(Box::new(instance)))))
}

/// Arguments of `object.__init__(self, /, *args, **kwargs)`.
#[derive(FromArgs)]
#[from_args(name = "object.__init__")]
struct ObjectInitArgs {
    #[from_args(pos_only, default)]
    instance: Option<Value>,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwargs: KwargsValues,
}

/// `object.__init__(self, *args, **kwargs)`: does nothing, but rejects extra
/// arguments unless the class overrides `__new__` and not `__init__`, CPython's
/// `object_init` rule.
pub(crate) fn builtin_object_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ObjectInitArgs { instance, args, kwargs } = ObjectInitArgs::from_args(args, vm)?;
    let excess = !args.is_empty() || !kwargs.is_empty();
    args.drop_with(vm);
    kwargs.drop_with(vm);
    let Some(instance) = instance else {
        return Err(ExcType::type_error(
            "descriptor '__init__' of 'object' object needs an argument",
        ));
    };
    defer_drop!(instance, vm);
    if !excess {
        return Ok(Value::None);
    }
    match instance.py_type(vm) {
        TypeRef::Class(class_id) if matches!(vm.heap.get(class_id), HeapData::Class(_)) => {
            if class_defines(class_id, "__init__", vm) {
                Err(init_takes_one_argument("object"))
            } else if class_defines(class_id, "__new__", vm) {
                Ok(Value::None)
            } else {
                let name = TypeRef::Class(class_id).name(vm.heap, vm.interns);
                Err(init_takes_one_argument(&name))
            }
        }
        // Immutable native types construct in `__new__` and leave `object.__init__`
        // in place, so CPython lets it ignore their arguments.
        TypeRef::Native(ty) if !native_overrides_init(ty) => Ok(Value::None),
        _ => Err(init_takes_one_argument("object")),
    }
}

/// Whether a native type defines its own `__init__` (CPython's `tp_init`), as
/// mutable containers and exceptions do; the rest build in `__new__`.
fn native_overrides_init(ty: Type) -> bool {
    ty.exc_type().is_some()
        || matches!(
            ty,
            Type::List
                | Type::Dict
                | Type::DefaultDict
                | Type::Counter
                | Type::Set
                | Type::Deque
                | Type::Super
                | Type::Property
                | Type::Path
                | Type::Random
                | Type::Module
        )
}

/// Arguments of `object.__init_subclass__(cls, /)`, which accepts no keywords.
#[derive(FromArgs)]
#[from_args(name = "object.__init_subclass__")]
struct ObjectInitSubclassArgs {
    #[from_args(pos_only, default)]
    cls: Option<Value>,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwargs: KwargsValues,
}

/// `object.__init_subclass__(cls)`: the hook run after a class statement,
/// which does nothing but reject arguments the statement passed.
pub(crate) fn builtin_object_init_subclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ObjectInitSubclassArgs { cls, args, kwargs } = ObjectInitSubclassArgs::from_args(args, vm)?;
    let given = args.len();
    let has_kwargs = !kwargs.is_empty();
    args.drop_with(vm);
    kwargs.drop_with(vm);
    let name = match cls {
        Some(cls) => {
            defer_drop!(cls, vm);
            match TypeRef::from_value(cls, vm.heap) {
                Some(ty) => ty.dunder_name(vm.heap, vm.interns).into_owned(),
                None => "object".to_owned(),
            }
        }
        None => "object".to_owned(),
    };
    if given > 0 {
        Err(ExcType::type_error(format!(
            "{name}.__init_subclass__() takes no arguments ({given} given)"
        )))
    } else if has_kwargs {
        Err(ExcType::type_error_no_kwargs(&format!("{name}.__init_subclass__")))
    } else {
        Ok(Value::None)
    }
}

/// CPython's `object.__init__` wording for an initializer given extra arguments.
fn init_takes_one_argument(type_name: &str) -> RunError {
    ExcType::type_error(format!(
        "{type_name}.__init__() takes exactly one argument (the instance to initialize)"
    ))
}

/// `Foo() takes no arguments`, for a class with neither `__init__` nor `__new__`.
pub(crate) fn takes_no_arguments(class_id: HeapId, vm: &VM<'_>) -> RunError {
    let name = TypeRef::Class(class_id).name(vm.heap, vm.interns);
    ExcType::type_error(format!("{name}() takes no arguments"))
}
