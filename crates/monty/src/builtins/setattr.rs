//! Implementation of the setattr() builtin function.

use monty_macros::py_signature;
use monty_types::ExcType;

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{RunResult, SimpleException},
    types::descriptor,
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
pub fn builtin_setattr(vm: &mut VM<'_>, params: &SetattrArgs) -> RunResult<CallResult> {
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

    // note: py_set_attr takes ownership of the inc-ref'd value and drops it on error
    descriptor::set_attr(&params.object, &name, params.value.clone_with_heap(vm), vm)
}
