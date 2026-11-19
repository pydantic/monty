//! Implementation of the setattr() builtin function.

use monty_macros::py_signature;
use monty_types::ExcType;

use crate::{
    bytecode::VM,
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
#[py_signature("setattr(object, name, value, /)")]
pub fn builtin_setattr(vm: &mut VM<'_>, args: &SetattrArgs) -> RunResult<Value> {
    let SetattrArgs { object, name, value } = args;

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
