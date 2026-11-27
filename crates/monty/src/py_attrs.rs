//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext},
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
}

impl NativeMethod {
    pub(crate) fn bind(self, receiver: &Value, vm: &VM<'_>) -> RunResult<Value> {
        check_receiver(self.owner, &EitherStr::Interned(self.name), receiver, vm)?;
        Ok(BoundMethod::allocate(
            receiver.clone_with_heap(vm),
            Value::NativeMethod(self),
            vm.heap,
        ))
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
        match pair {
            (Value::None, None | Some(Value::None)) => Err(ExcType::type_error("__get__(None, None) is invalid")),
            (Value::None, _) => Ok(Value::NativeMethod(method)),
            (instance, _) => method.bind(instance, vm),
        }
    }
}

/// A class read returns its descriptor or constant. An instance read binds the receiver.
pub(crate) fn read_member(ty: Type, attr: &EitherStr, instance: Option<&Value>, vm: &mut VM<'_>) -> Option<Value> {
    let key = attr.static_string(vm.interns)?;
    let (owner, definition) = ty.lookup_static_attr(key)?;
    let receiver = match definition {
        AttrDef::Value(get) => return instance.is_none().then(|| get(vm)),
        AttrDef::ClassMethod(_) => Some(TypeRef::Native(ty).to_value(vm.heap)),
        AttrDef::Method(_) => instance.map(|value| value.clone_with_heap(vm)),
    };
    let method = Value::NativeMethod(NativeMethod {
        owner,
        name: vm.interns.intern_static(key),
    });
    Some(match receiver {
        Some(receiver) => BoundMethod::allocate(receiver, method, vm.heap),
        None => method,
    })
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
