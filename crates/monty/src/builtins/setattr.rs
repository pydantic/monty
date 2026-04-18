//! Implementation of the setattr() builtin function.

use crate::{
    ExcType,
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{RunResult, SimpleException},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

/// Implementation of the setattr() builtin function.
///
/// Sets the named attribute on the given object to the specified value
/// This is the counterpart to getattr(). Returns None on success
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

    let Some(name) = name.as_either_str(vm.heap) else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("attribute name must be string, not '{}'", name.py_type(vm)),
        )
        .into());
    };

    // note: py_set_attr takes ownership of the inc-ref'd value and drops it on error
    object.py_set_attr(&name, value.clone_with_heap(vm), vm)?;

    Ok(Value::None)
}
