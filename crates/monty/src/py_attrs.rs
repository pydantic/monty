//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext},
    intern::StaticStrings,
    py_value::PyValue,
    types::{PyTrait, Type, TypeRef},
    value::{EitherStr, Value},
};

/// The receiver is the bound class object, including when called through an instance.
pub(crate) type ClassMethodCall = for<'h> fn(TypeRef, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type Getter = for<'h> fn(&mut PyValue<'h>, &mut VM<'h>) -> RunResult<Value>;
pub(crate) type Setter = for<'h> fn(&mut PyValue<'h>, Value, &mut VM<'h>) -> RunResult<()>;

/// Adapts both infallible and fallible native accessors to the table signature.
pub(crate) trait IntoAttrResult<T> {
    fn into_attr_result(self) -> RunResult<T>;
}

impl<T> IntoAttrResult<T> for T {
    fn into_attr_result(self) -> RunResult<T> {
        Ok(self)
    }
}

impl<T> IntoAttrResult<T> for RunResult<T> {
    fn into_attr_result(self) -> Self {
        self
    }
}

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
    GetSet {
        get: Getter,
        set: Option<Setter>,
        member: bool,
    },
}

/// Writes a native descriptor, consuming the value even when it is read-only.
pub(crate) fn set_attribute<'h>(
    owner: Type,
    attr: &EitherStr,
    member: bool,
    setter: Option<Setter>,
    receiver: &mut PyValue<'h>,
    value: Value,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    if let Some(set) = setter {
        set(receiver, value, vm)
    } else {
        value.drop_with(vm);
        if member {
            Err(ExcType::attribute_error_readonly())
        } else {
            Err(ExcType::attribute_error_not_writable(
                attr.as_str(vm.interns),
                owner.name(),
            ))
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
    let receiver_type = receiver.py_type(vm);
    if !receiver_type.is_subclass_of(owner.into(), vm.heap) {
        return Err(ExcType::type_error(format!(
            "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
            attr.as_str(vm.interns),
            owner.name(),
            receiver.py_type_name(vm),
        )));
    }
    let ((receiver, args), vm) = pending.into_parts();
    defer_drop!(receiver, vm);
    call_method(owner, &mut receiver.py_value(vm), attr, call, args, vm)
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
    Err(invalid_receiver_error(name))
}

pub(crate) fn invalid_receiver_error(name: StaticStrings) -> RunError {
    let name: &str = name.into();
    ExcType::type_error(format!("unsupported native receiver for '{name}'"))
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
