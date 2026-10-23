//! Implementation of the getattr() builtin function.

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, PendingLookupEffect, VM},
    defer_drop,
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    frozen::FrozenFunction,
    types::{
        attribute_name_value, descriptor,
        instance::{FastAttributeLookupResult, lookup_attribute_fast},
    },
    value::Value,
};

/// Implementation of the getattr() builtin function.
///
/// Returns the value of the named attribute of an object.
/// If the attribute doesn't exist and a default is provided, returns the default.
/// If no default is provided and the attribute doesn't exist, raises AttributeError.
///
/// Note: name must be a string. Per Python docs, "Since private name mangling happens
/// at compilation time, one must manually mangle a private attribute's (attributes with
/// two leading underscores) name in order to retrieve it with getattr()."
///
/// A lazy attribute on a host-backed object suspends to the host like
/// `obj.attr` does; the `default` rides along as a [`PendingLookupEffect`]
/// so an `Undefined` answer yields it instead of raising.
///
/// Examples:
/// ```python
/// getattr(obj, 'x')             # Get obj.x
/// getattr(obj, 'y', None)       # Get obj.y or None if not found
/// getattr(module, 'function')   # Get module.function
/// ```
pub fn builtin_getattr(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let positional = args.into_pos_only("getattr", vm.heap)?;
    defer_drop!(positional, vm);

    let (object, name, default) = match positional.as_slice() {
        too_few @ ([] | [_]) => return Err(ExcType::type_error_at_least("getattr", 2, too_few.len())),
        [object, name] => (object, name, None),
        [object, name, default] => (object, name, Some(default)),
        too_many => return Err(ExcType::type_error_at_most("getattr", 3, too_many.len())),
    };

    let Some(attr) = name.as_either_str(vm.heap).map(|s| s.resolve_interned(vm.interns)) else {
        let ty = name.py_type_name(vm);
        return Err(
            SimpleException::new_msg(ExcType::TypeError, format!("attribute name must be string, not '{ty}'")).into(),
        );
    };

    match lookup_attribute_fast(object, &attr, vm) {
        FastAttributeLookupResult::Found(value) => return Ok(CallResult::Value(value)),
        FastAttributeLookupResult::Missing => {
            return default
                .map(|default| CallResult::Value(default.clone_with_heap(vm.heap)))
                .ok_or_else(|| ExcType::attribute_error(object.py_type_name(vm), attr.as_str(vm.interns)));
        }
        FastAttributeLookupResult::Inconclusive => {}
    }

    if let Some(default) = default
        && descriptor::attribute_uses_descriptor(object, &attr, vm)
    {
        let name = attribute_name_value(&attr, vm);
        return vm.call_frozen_exact(
            FrozenFunction::DescriptorGetattr,
            [
                descriptor::attribute_helper(),
                object.clone_with_heap(vm.heap),
                name,
                default.clone_with_heap(vm.heap),
                Value::Builtin(Builtins::ExcType(ExcType::AttributeError)),
            ],
        );
    }
    match object.py_getattr(&attr, vm) {
        Ok(CallResult::Value(value)) => Ok(CallResult::Value(value)),
        Ok(CallResult::AttrLookup {
            name,
            class_name,
            object_id,
            type_object,
            effect: _,
        }) => Ok(CallResult::AttrLookup {
            name,
            class_name,
            object_id,
            type_object,
            effect: default.map(|d| PendingLookupEffect::Default(d.clone_with_heap(vm))),
        }),
        Ok(other) => Ok(other),
        Err(RunError::Exc(e))
            if let Some(d) = default
                && e.exc.exc_type() == ExcType::AttributeError =>
        {
            Ok(CallResult::Value(d.clone_with_heap(vm)))
        }
        Err(e) => Err(e),
    }
}
