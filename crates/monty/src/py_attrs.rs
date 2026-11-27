//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData},
    intern::{StaticStrings, StringId},
    py_value::PyValue,
    types::{BoundMethod, PyTrait, Type, TypeRef, str::allocate_string},
    value::{EitherStr, Value},
};

/// The receiver is the bound class object, including when called through an instance.
pub(crate) type ClassMethodCall = for<'h> fn(TypeRef, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;

/// Python attribute definitions and the native view passed to their methods.
pub(crate) trait PyAttrs<'h> {
    type Receiver;
    const TABLE: AttrTable;
}

/// Enumeration and generated lookup for one native namespace.
#[derive(Clone, Copy)]
pub(crate) struct AttrTable {
    pub entries: &'static [(StaticStrings, AttrDef)],
    pub lookup: fn(StaticStrings) -> Option<AttrDef>,
}

#[derive(Clone, Copy)]
pub(crate) enum AttrDef {
    Method(MethodCall),
    ClassMethod(ClassMethodCall),
    Value(fn(&mut VM<'_>) -> Value),
}

/// A registered native method identified by its defining class and interned Python name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct NativeMethod {
    pub owner: Type,
    pub name: StringId,
    #[serde(default)]
    class_method: bool,
}

impl NativeMethod {
    pub(crate) fn kind(self) -> Type {
        if self.class_method {
            Type::ClassMethodDescriptor
        } else {
            Type::MethodDescriptor
        }
    }

    pub(crate) fn call(self, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
        let attr = EitherStr::Interned(self.name);
        if self.class_method {
            let (receiver, args) = take_receiver(args).map_err(|args| {
                args.drop_with(vm);
                ExcType::type_error(format!(
                    "descriptor '{}' of '{}' object needs an argument",
                    attr.as_str(vm.interns),
                    self.owner.dunder_name()
                ))
            })?;
            defer_drop!(receiver, vm);
            let class = match self.descriptor_class(receiver, vm) {
                Ok(class) => class,
                Err(error) => {
                    args.drop_with(vm);
                    return Err(error);
                }
            };
            let Some((_, AttrDef::ClassMethod(call))) = self.owner.lookup_attr(&attr, vm.interns) else {
                args.drop_with(vm);
                return Err(self.owner.attribute_error(&attr, vm));
            };
            return call(class, args, vm);
        }
        self.owner.call_class_method(&attr, args, vm)
    }

    fn validate_class(self, class: TypeRef, vm: &VM<'_>) -> RunResult<TypeRef> {
        if class.is_subclass_of(self.owner.into(), vm.heap)
            || (self.owner == Type::NamedTuple
                && matches!(class, TypeRef::Class(id) if matches!(vm.heap.get(id), HeapData::NamedTupleClass(_))))
        {
            Ok(class)
        } else {
            Err(ExcType::type_error(format!(
                "descriptor '{}' requires a subtype of '{}' but received '{}'",
                vm.interns.get_str(self.name),
                self.owner.dunder_name(),
                class.name(vm.heap, vm.interns)
            )))
        }
    }

    fn descriptor_class(self, value: &Value, vm: &VM<'_>) -> RunResult<TypeRef> {
        let class = TypeRef::from_value(value, vm.heap).ok_or_else(|| {
            ExcType::type_error(format!(
                "descriptor '{}' for type '{}' needs a type, not a '{}' as arg 2",
                vm.interns.get_str(self.name),
                self.owner.dunder_name(),
                value.py_type_name(vm)
            ))
        })?;
        self.validate_class(class, vm)
    }

    pub(crate) fn bind(self, instance: Option<&Value>, owner: Option<&Value>, vm: &VM<'_>) -> RunResult<Value> {
        let receiver = match (instance, owner) {
            (None, None) => return Err(ExcType::type_error("__get__(None, None) is invalid")),
            (_, Some(owner)) if self.class_method => self.descriptor_class(owner, vm)?.to_value(vm.heap),
            (Some(instance), None) if self.class_method => {
                self.validate_class(instance.py_type(vm), vm)?.to_value(vm.heap)
            }
            (None, Some(_)) => return Ok(Value::NativeMethod(self)),
            (Some(instance), _) => {
                check_receiver(self.owner, &EitherStr::Interned(self.name), instance, vm)?;
                instance.clone_with_heap(vm)
            }
        };
        Ok(BoundMethod::allocate(receiver, Value::NativeMethod(self), vm.heap))
    }

    pub(crate) fn bound_attr(self, receiver: &Value, name: &str, vm: &VM<'_>) -> Option<Value> {
        match name {
            "__self__" => Some(receiver.clone_with_heap(vm)),
            "__name__" => Some(Value::InternString(self.name)),
            "__qualname__" => {
                let ty = TypeRef::from_value(receiver, vm.heap).unwrap_or_else(|| receiver.py_type(vm));
                let name = ty.name(vm.heap, vm.interns);
                Some(self.qualname(name.rsplit_once('.').map_or(&*name, |(_, bare)| bare), vm))
            }
            _ => None,
        }
    }

    fn qualname(self, owner: &str, vm: &VM<'_>) -> Value {
        allocate_string(format!("{owner}.{}", vm.interns.get_str(self.name)), vm.heap)
    }

    pub(crate) fn attr(self, name: &str, vm: &VM<'_>) -> Option<Value> {
        Some(match name {
            "__name__" => Value::InternString(self.name),
            "__qualname__" => self.qualname(self.owner.dunder_name(), vm),
            "__objclass__" => TypeRef::Native(self.owner).to_value(vm.heap),
            _ => return None,
        })
    }
}

#[monty_macros::pyattrs]
impl PyAttrs<'_> for NativeMethod {
    type Receiver = NativeMethod;

    #[pymethod(DunderGet)]
    fn __get__(method: NativeMethod, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
        let pair = args.get_one_two_args("__get__", vm.heap)?;
        defer_drop!(pair, vm);
        let (instance, owner) = pair;
        method.bind(
            (!matches!(instance, Value::None)).then_some(instance),
            owner.as_ref().filter(|v| !matches!(v, Value::None)),
            vm,
        )
    }
}

/// A class read returns its descriptor or constant. An instance read binds the receiver.
pub(crate) fn read_member(ty: Type, attr: &EitherStr, instance: Option<&Value>, vm: &mut VM<'_>) -> Option<Value> {
    let key = attr.static_string(vm.interns)?;
    let (owner, definition) = ty.lookup_static_attr(key)?;
    if matches!(definition, AttrDef::Value(_)) && instance.is_some() {
        return None;
    }
    let member = definition.namespace_value(owner, key, vm);
    Some(match definition {
        AttrDef::ClassMethod(_) => BoundMethod::allocate(
            instance
                .map_or(TypeRef::Native(ty), |value| value.py_class(vm))
                .to_value(vm.heap),
            member,
            vm.heap,
        ),
        AttrDef::Method(_) => match instance {
            Some(receiver) => BoundMethod::allocate(receiver.clone_with_heap(vm.heap), member, vm.heap),
            None => member,
        },
        AttrDef::Value(_) => member,
    })
}

impl AttrDef {
    pub(crate) fn namespace_value(self, owner: Type, key: StaticStrings, vm: &mut VM<'_>) -> Value {
        match self {
            Self::Value(get) => get(vm),
            Self::Method(_) | Self::ClassMethod(_) => Value::NativeMethod(NativeMethod {
                owner,
                name: vm.interns.intern_static(key),
                class_method: matches!(self, Self::ClassMethod(_)),
            }),
        }
    }
}

/// Calls an instance method through a type, taking the receiver from the first positional argument.
///
/// Validates the receiver against the defining type and adapts supported native subtypes.
pub(crate) fn call_unbound(
    owner: Type,
    attr: &EitherStr,
    call: MethodCall,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let (receiver, args) = match take_receiver(args) {
        Ok(parts) => parts,
        Err(args) => {
            args.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "unbound method {}.{}() needs an argument",
                owner.dunder_name(),
                attr.as_str(vm.interns),
            )));
        }
    };
    let mut pending = DropGuard::new((receiver, args), vm);
    let ((receiver, _), vm) = pending.as_parts_mut();
    check_receiver(owner, attr, receiver, vm)?;
    let ((receiver, args), vm) = pending.into_parts();
    defer_drop!(receiver, vm);
    call_method(owner, &mut receiver.py_value(vm), attr, call, args, vm)
}

fn check_receiver(owner: Type, attr: &EitherStr, receiver: &Value, vm: &VM<'_>) -> RunResult<()> {
    let receiver_type = receiver.py_type(vm);
    if !receiver_type.is_subclass_of(owner.into(), vm.heap) {
        return Err(ExcType::type_error(format!(
            "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
            attr.as_str(vm.interns),
            owner.name(),
            receiver.py_type_name(vm),
        )));
    }
    Ok(())
}

/// Calls a native descriptor, using its base's adapter when the receiver has another layout.
pub(crate) fn call_method<'h>(
    owner: Type,
    receiver: &mut PyValue<'h>,
    attr: &EitherStr,
    call: MethodCall,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    if let Some(adapt) = owner.def().subtype_call
        && receiver.py_type(vm) != owner
    {
        adapt(receiver, attr, call, args, vm)
    } else {
        call(receiver, args, vm)
    }
}

/// Adapts a base descriptor to a native subtype with a different payload.
pub(crate) type SubtypeCall =
    for<'h> fn(&mut PyValue<'h>, &EitherStr, MethodCall, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;

/// A table adapter could not project the receiver to its declared native view.
pub(crate) fn invalid_receiver(name: StaticStrings, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    args.drop_with(vm);
    let name: &str = name.into();
    Err(ExcType::type_error(format!("unsupported native receiver for '{name}'")))
}

/// Splits a positional receiver from the call without consuming keyword-only calls.
pub(crate) fn take_receiver(args: ArgValues) -> Result<(Value, ArgValues), ArgValues> {
    match args {
        ArgValues::One(receiver) => Ok((receiver, ArgValues::Empty)),
        ArgValues::Two(receiver, arg) => Ok((receiver, ArgValues::One(arg))),
        ArgValues::ArgsKargs { mut args, kwargs } if !args.is_empty() => {
            let receiver = args.remove(0);
            Ok((receiver, ArgValues::from_parts(args, kwargs)))
        }
        args => Err(args),
    }
}
