//! Attribute deletion, including suspendable descriptor deleters.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapReadOutput},
    modules::dataclasses,
    types::{
        attribute_name_value, descriptor,
        instance::{class_member, class_name},
    },
    value::Value,
};

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
        && let Some(member) = class_member(class, attr.as_str(vm.interns), vm)
    {
        defer_drop!(member, vm);
        if descriptor::is_data(member, vm) {
            return descriptor::mutate(member, object, None, vm);
        }
    }
    let key = attribute_name_value(&attr, vm);
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
        Err(ExcType::attribute_error_type(
            &class_name(*id, vm.heap, vm.interns),
            attr.as_str(vm.interns),
        ))
    } else {
        Err(ExcType::attribute_error(
            object.py_type_name(vm),
            attr.as_str(vm.interns),
        ))
    }
}
