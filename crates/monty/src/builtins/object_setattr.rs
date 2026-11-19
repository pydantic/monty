//! Implementation of `object.__setattr__`, the hook-bypassing attribute write.

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, HeapReadOutput},
    value::{EitherStr, Value},
};

/// `object.__setattr__(obj, name, value)` — writes an instance attribute
/// without consulting the class.
///
/// The escape hatch a class that hooks attribute writes needs: its own
/// `__setattr__` has to store the value somehow, and going through `obj.x = v`
/// would call itself. Only instances of user-defined classes are accepted, as
/// CPython rejects anything whose `__setattr__` is not `object`'s.
///
/// ```python
/// object.__setattr__(point, 'x', 1)
/// ```
pub fn builtin_object_setattr(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    // CPython reaches this as a slot wrapper bound to `obj`, so its messages
    // name the wrapper and count `obj` as the receiver rather than an argument.
    let args = args.reject_kwargs("wrapper __setattr__", vm.heap)?;
    let given = args.count();
    let positional = monty_macros::bind_positional!(args, vm.heap, "object.__setattr__(object, name, value, /)", iter)
        .map_err(|_| match given {
            0 => ExcType::type_error("descriptor '__setattr__' of 'object' object needs an argument"),
            n => ExcType::type_error_arg_count("__setattr__", 2, n - 1),
        })?;
    defer_drop!(positional, vm);

    let [object, name, value] = positional.as_slice() else {
        unreachable!("arity checked by signature")
    };

    let Some(name) = name.as_either_str(vm.heap) else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("attribute name must be string, not '{}'", name.py_type_name(vm)),
        )
        .into());
    };

    // Only an instance has a `__dict__` to write into. Everything else gets the
    // error CPython's own `object.__setattr__` produces for a type whose
    // instances have no attribute storage.
    let Value::Ref(heap_id) = object else {
        return Err(no_dict_error(object, &name, vm));
    };
    let HeapReadOutput::Instance(instance) = vm.heap.read(*heap_id) else {
        return Err(no_dict_error(object, &name, vm));
    };

    let name = name.as_value(vm.heap);
    // The write path itself, bypassing whatever the class would have done.
    let replaced = instance.set_attr_unchecked(name, value.clone_with_heap(vm), vm)?;
    replaced.drop_with(vm);
    Ok(Value::None)
}

/// `object.__setattr__` applied to a value with no instance `__dict__`.
fn no_dict_error(object: &Value, name: &EitherStr, vm: &VM<'_>) -> RunError {
    let type_name = object.py_type_name(vm);
    ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns))
}
