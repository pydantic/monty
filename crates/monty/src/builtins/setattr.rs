//! Implementation of the setattr() builtin function.

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcTypeExt, RunResult, SimpleException},
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
pub fn builtin_setattr(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let positional = args.into_pos_only("setattr", vm.heap)?;
    defer_drop!(positional, vm);

    let (object, name, value) = match positional.as_slice() {
        [object, name, value] => (object, name, value),
        other => return Err(ExcType::type_error_arg_count("setattr", 3, other.len())),
    };

    let Some(name) = name.as_either_str(vm.heap).map(|s| s.resolve_interned(vm.interns)) else {
        return Err(SimpleException::new_msg(
            ExcType::TypeError,
            format!("attribute name must be string, not '{}'", name.py_type_name(vm)),
        )
        .into());
    };

    // note: py_set_attr takes ownership of the inc-ref'd value and drops it on error
    descriptor::set_attr(object, &name, value.clone_with_heap(vm), vm)
}
