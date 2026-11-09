//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext},
    intern::StaticStrings,
    py_class::PyClass,
    py_value::PyValue,
    types::{PyTrait, Type, date, namedtuple},
    value::{EitherStr, Value},
};

/// The receiver is the bound class object, including when called through an instance.
pub(crate) type ClassMethodCall = for<'h> fn(PyClass<'h>, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
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
    Method {
        call: MethodCall,
        /// Inherited native descriptors validate against their defining base.
        owner: Option<Type>,
    },
    ClassMethod(ClassMethodCall),
    Value(fn(&mut VM<'_>) -> Value),
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
                owner.dunder_name(vm.heap, vm.interns),
                attr.as_str(vm.interns),
            )));
        }
    };
    let mut pending = DropGuard::new((receiver, args), vm);
    let ((receiver, _), vm) = pending.as_parts_mut();
    let receiver_type = receiver.py_type(vm);
    if !receiver_type.is_instance_of(owner) {
        return Err(ExcType::type_error(format!(
            "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
            attr.as_str(vm.interns),
            owner.name(vm.heap, vm.interns),
            receiver.py_type_name(vm),
        )));
    }
    let ((receiver, args), vm) = pending.into_parts();
    defer_drop!(receiver, vm);
    if receiver_type == owner {
        call(&mut receiver.py_value(vm), args, vm)
    } else {
        call_unbound_subtype(owner, attr, call, receiver, args, vm)
    }
}

/// Native subtypes with different layouts provide their own descriptor adapters.
fn call_unbound_subtype(
    owner: Type,
    attr: &EitherStr,
    call: MethodCall,
    this: &Value,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    match (owner, this.py_value(vm)) {
        (Type::Dict, PyValue::Dict(dict)) => call(&mut PyValue::Dict(dict), args, vm),
        (Type::Tuple, PyValue::NamedTuple(tuple)) => namedtuple::call_tuple_descriptor(&tuple, attr, args, vm),
        (Type::Date, PyValue::DateTime(mut datetime)) => {
            date::call_datetime_descriptor(&mut datetime, attr, call, args, vm)
        }
        _ => {
            args.drop_with(vm);
            Err(ExcType::type_error(format!(
                "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                attr.as_str(vm.interns),
                owner.name(vm.heap, vm.interns),
                this.py_type_name(vm),
            )))
        }
    }
}

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
