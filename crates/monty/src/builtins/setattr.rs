//! Implementation of the setattr() builtin function.

use monty_macros::bind_positional;
use monty_types::ExcType;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{RunResult, SimpleException},
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
pub fn builtin_setattr(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let positional = bind_positional!(args, vm.heap, "setattr(object, name, value, /)", iter)?;
    defer_drop!(positional, vm);

    let [object, name, value] = positional.as_slice() else {
        unreachable!("arity checked by signature")
    };

    let Some(name) = name.as_either_str(vm.heap).map(|s| s.resolve_interned(vm.interns)) else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("attribute name must be string, not '{}'", name.py_type_name(vm)),
        )
        .into());
    };

    // note: py_set_attr takes ownership of the inc-ref'd value and drops it on error
    object.py_set_attr(&name, value.clone_with_heap(vm), vm)?;

    Ok(Value::None)
}
