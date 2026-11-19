//! Implementation of the hasattr() builtin function.

use monty_macros::py_signature;
use monty_types::ExcType;

use crate::{
    builtins::Builtins,
    bytecode::{CallResult, PendingLookupEffect, VM},
    exception_private::{RunError, RunResult, SimpleException},
    frozen::FrozenFunction,
    heap::DropWithContext,
    types::{
        descriptor,
        instance::{FastAttributeLookupResult, lookup_attribute_fast},
    },
    value::Value,
};

/// Implementation of the hasattr() builtin function.
///
/// Returns True if the object has the named attribute, False otherwise.
/// This function always succeeds and never raises AttributeError.
///
/// Signature: `hasattr(object, name)`
///
/// Note: This is implemented by calling getattr(object, name) and returning
/// True if it succeeds, False if it raises AttributeError. A lazy attribute on a
/// host-backed object suspends to the host like `obj.attr` does, with a
/// [`PendingLookupEffect::HasAttr`] turning the answer into the bool.
///
/// Examples:
/// ```python
/// hasattr(obj, 'x')             # Check if obj.x exists
/// hasattr(slice(1, 10), 'start') # True - slice has start attribute
/// hasattr(42, 'nonexistent')    # False - int has no such attribute
/// ```
#[py_signature("hasattr(object, name, /)")]
pub fn builtin_hasattr(vm: &mut VM<'_>, params: &HasattrArgs) -> RunResult<CallResult> {
    let Some(name) = params
        .name
        .as_either_str(vm.heap)
        .map(|s| s.resolve_interned(vm.interns))
    else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("attribute name must be string, not '{}'", params.name.py_type_name(vm)),
        )
        .into());
    };

    let object = &params.object;
    match lookup_attribute_fast(object, &name, vm) {
        FastAttributeLookupResult::Found(value) => {
            value.drop_with(vm);
            return Ok(CallResult::Value(Value::Bool(true)));
        }
        FastAttributeLookupResult::Missing => return Ok(CallResult::Value(Value::Bool(false))),
        FastAttributeLookupResult::HostLookup => {}
        FastAttributeLookupResult::Inconclusive => {
            let name = name.as_value(vm.heap);
            return vm.call_frozen_exact(
                FrozenFunction::DescriptorHasattr,
                [
                    descriptor::attribute_helper(),
                    object.clone_with_heap(vm.heap),
                    name,
                    Value::Builtin(Builtins::Type(ExcType::AttributeError.into())),
                ],
            );
        }
    }

    // Only native host lookups reach this path; Undefined is handled by the pending effect.
    let has_attr = match object.py_getattr(&name, vm) {
        Ok(CallResult::Value(value)) => {
            value.drop_with(vm);
            true
        }
        Ok(CallResult::AttrLookup {
            name,
            class_name,
            object_id,
            type_object,
            effect,
        }) => {
            effect.drop_with(vm);
            return Ok(CallResult::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect: Some(PendingLookupEffect::HasAttr),
            });
        }
        Ok(other) => {
            other.drop_with(vm);
            return Err(RunError::internal("hasattr() lookup escaped its continuation"));
        }
        Err(RunError::Exc(e)) if e.exc.exc_type() == ExcType::AttributeError => false,
        Err(e) => return Err(e),
    };

    Ok(CallResult::Value(Value::Bool(has_attr)))
}
