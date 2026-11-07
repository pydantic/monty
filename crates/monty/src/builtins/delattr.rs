//! Attribute deletion, including suspendable descriptor deleters.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{HeapData, HeapReadOutput},
    modules::dataclasses,
    types::{descriptor, instance::class_name},
    value::Value,
};

/// Delete instance storage or dispatch its type's data-descriptor hook.
pub(crate) fn builtin_delattr(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let args = args.into_pos_only("delattr", vm.heap)?;
    defer_drop!(args, vm);
    let [object, name] = args.as_slice() else {
        return Err(ExcType::type_error_arg_count("delattr", 2, args.len()));
    };
    let Some(attr) = name.as_either_str(vm.heap).map(|s| s.resolve_interned(vm.interns)) else {
        return Err(ExcType::type_error(format!(
            "attribute name must be string, not '{}'",
            name.py_type_name(vm)
        )));
    };
    if let Some(class) = descriptor::descriptor_class(object, vm)
        && let Some(error) = dataclasses::frozen_deletion_error(class, name, vm)
    {
        return Err(error);
    }
    if let Some(class) = descriptor::descriptor_class(object, vm)
        && let Some(member) = descriptor::data_member(class, attr.as_str(vm.interns), vm)
    {
        defer_drop!(member, vm);
        return descriptor::mutate(member, object, None, vm);
    }
    let key = attr.as_value(vm.heap);
    defer_drop!(key, vm);
    let removed = match object {
        Value::Ref(id) => match vm.heap.read(*id) {
            HeapReadOutput::Instance(instance) => instance.attrs_mut().pop(key, vm)?.map(|(key, value)| {
                key.drop_with(vm);
                value
            }),
            HeapReadOutput::Class(class) => class.pop_attr(key, vm)?,
            _ => None,
        },
        _ => None,
    };
    if let Some(value) = removed {
        value.drop_with(vm);
        Ok(CallResult::Value(Value::None))
    } else if let Value::Ref(id) = object
        && matches!(vm.heap.get(*id), HeapData::Class(_))
    {
        Err(deletion_error(ExcType::attribute_error_type(
            &class_name(*id, vm.heap, vm.interns),
            attr.as_str(vm.interns),
        )))
    } else {
        Err(deletion_error(ExcType::attribute_error(
            object.py_type_name(vm),
            attr.as_str(vm.interns),
        )))
    }
}

/// Attribute deletion displays carets, unlike attribute reads with the same message.
fn deletion_error(mut error: RunError) -> RunError {
    if let RunError::Exc(raised) = &mut error {
        raised.hide_caret = false;
    }
    error
}
