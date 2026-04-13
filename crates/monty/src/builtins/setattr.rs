//! Implementation of the setattr() builtin function.

use crate::{
    ExcType,
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{RunResult, SimpleException},
    resource::ResourceTracker,
    value::Value,
};

/// Implementation of the setattr() builtin function.
///
/// Sets the named attribute on the given object to the specified value
/// This is the counterpart to getattr(). Returns None on success
///
/// Note: Currently only dataclass objects support attribute setting
/// Other object types will raise AttributeError
///
/// Examples:
/// ```python
/// setattr(obj, 'x', 42)      # Set obj.x = 42
/// setattr(obj, 'name', 'foo') # Set obj.name = 'foo'
/// ```
pub fn builtin_setattr(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let positional = args.into_pos_only("setattr", vm.heap)?;
    defer_drop!(positional, vm);

    let (object, name, value) = match positional.as_slice() {
        [object, name, value] => (object, name, value),
        other => return Err(ExcType::type_error_arg_count("setattr", 3, other.len())),
    };

    let Value::InternString(name_id) = name else {
        return Err(SimpleException::new_msg(ExcType::TypeError, "setattr(): attribute name must be string").into());
    };

    // note: py_set_attr takes ownership of value and drops it on error
    object.py_set_attr(*name_id, value.clone_with_heap(vm), vm)?;

    Ok(Value::None)
}
