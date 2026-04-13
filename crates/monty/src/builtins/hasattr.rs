//! Implementation of the hasattr() builtin function.

use crate::{
    ExcType,
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{RunResult, SimpleException},
    resource::ResourceTracker,
    value::{EitherStr, Value},
};

/// Implementation of the hasattr() builtin function.
///
/// Returns True if the object has the named attribute, False otherwise.
/// This function always succeeds and never raises AttributeError.
///
/// Signature: `hasattr(object, name)`
///
/// Note: This is implemented by calling getattr(object, name) and returning
/// True if it succeeds, False if it raises an exception.
///
/// Examples:
/// ```python
/// hasattr(obj, 'x')             # Check if obj.x exists
/// hasattr(slice(1, 10), 'start') # True - slice has start attribute
/// hasattr(42, 'nonexistent')    # False - int has no such attribute
/// ```
pub fn builtin_hasattr(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let positional = args.into_pos_only("hasattr", vm.heap)?;
    defer_drop!(positional, vm);

    let (object, name) = match positional.as_slice() {
        [object, name] => (object, name),
        other => return Err(ExcType::type_error_arg_count("hasattr", 2, other.len())),
    };

    let Value::InternString(name_id) = name else {
        return Err(SimpleException::new_msg(ExcType::TypeError, "hasattr(): attribute name must be string").into());
    };

    // important: we must own the returned value if py_get_attr succeeds to drop it
    let has_attr = match object.py_getattr(&EitherStr::Interned(*name_id), vm) {
        Ok(CallResult::Value(value)) => {
            value.drop_with_heap(vm);
            true
        }
        Ok(_) => {
            // hasattr() only tests attribute values — OS calls, external calls,
            // method calls, and awaits are not supported here
            //
            // TODO: might need to support this case?
            return Err(
                SimpleException::new_msg(ExcType::TypeError, "getattr(): attribute is not a simple value").into(),
            );
        }
        Err(_) => false,
    };

    Ok(Value::Bool(has_attr))
}
