//! Implementation of the getattr() builtin function.

use crate::{
    ExcType,
    args::ArgValues,
    defer_drop,
    exception_private::{RunResult, SimpleException},
    heap::{Heap, HeapData},
    intern::Interns,
    resource::ResourceTracker,
    types::AttrCallResult,
    value::Value,
};

/// Implementation of the getattr() builtin function.
///
/// Returns the value of the named attribute of an object
/// If the attribute doesn't exist and a default is provided, returns the default
/// If no default is provided and the attribute doesn't exist, raises AttributeError
///
/// Note: name must be a string. Per Python docs, "Since private name mangling happens
/// at compilation time, one must manually mangle a private attribute's (attributes with
/// two leading underscores) name in order to retrieve it with getattr()."
///
/// Examples:
/// ```python
/// getattr(obj, 'x')             # Get obj.x
/// getattr(obj, 'y', None)       # Get obj.y or None if not found
/// getattr(module, 'function')   # Get module.function
/// ```
pub fn builtin_getattr(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let positional = args.into_pos_only("getattr", heap)?;
    defer_drop!(positional, heap);

    let (object, name, default) = match positional.as_slice() {
        too_few @ ([] | [_]) => return Err(ExcType::type_error_at_least("getattr", 2, too_few.len())),
        [object, name] => (object, name, None),
        [object, name, default] => (object, name, Some(default)),
        too_many => return Err(ExcType::type_error_at_most("getattr", 3, too_many.len())),
    };

    let name_id = match name {
        Value::InternString(id) => *id,
        Value::Ref(v) if matches!(heap.get(*v), HeapData::Str(_)) => {
            // TODO: support arbitrary strings as attribute names, not just interned ones.
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                "getattr(): attribute name must be interned string",
            )
            .into());
        }
        _ => {
            return Err(
                SimpleException::new_msg(ExcType::TypeError, "getattr(): attribute name must be string").into(),
            );
        }
    };

    match object.py_getattr(name_id, heap, interns) {
        Ok(AttrCallResult::Value(value)) => Ok(value),
        Ok(_) => {
            // getattr() only retrieves attribute values — OS calls, external calls,
            // method calls, and awaits are not supported here
            //
            // TODO: might need to support this case?
            Err(SimpleException::new_msg(ExcType::TypeError, "getattr(): attribute is not a simple value").into())
        }
        Err(e) => {
            if let Some(d) = default {
                Ok(d.clone_with_heap(heap))
            } else {
                Err(e)
            }
        }
    }
}
