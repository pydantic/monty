//! Implementation of the isinstance() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::{Heap, HeapData},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{PyTrait, Type},
    value::Value,
};

/// Implementation of the isinstance() builtin function.
///
/// Checks if an object is an instance of a class or a tuple of classes.
pub fn builtin_isinstance(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (obj, classinfo) = args.get_two_args("isinstance")?;
    let obj_type = obj.py_type(Some(heap));

    let Ok(result) = isinstance_check(obj_type, &classinfo, heap) else {
        obj.drop_with_heap(heap);
        classinfo.drop_with_heap(heap);
        return Err(ExcType::isinstance_arg2_error());
    };

    obj.drop_with_heap(heap);
    classinfo.drop_with_heap(heap);
    Ok(Value::Bool(result))
}

/// Recursively checks if obj_type matches classinfo for isinstance().
///
/// Returns `Ok(true)` if the type matches, `Ok(false)` if it doesn't,
/// or `Err(())` if classinfo is invalid (not a type or tuple of types).
///
/// Supports:
/// - Single types: `isinstance(x, int)`
/// - Exception types: `isinstance(err, ValueError)`
/// - Exception hierarchy: `isinstance(err, LookupError)` for KeyError/IndexError
/// - Nested tuples: `isinstance(x, (int, (str, bytes)))`
fn isinstance_check(obj_type: Type, classinfo: &Value, heap: &Heap<impl ResourceTracker>) -> Result<bool, ()> {
    match classinfo {
        // Single type: isinstance(x, int)
        Value::Builtin(Builtins::Type(t)) => Ok(obj_type.is_instance_of(*t)),

        // Exception type: isinstance(err, ValueError) or isinstance(err, LookupError)
        Value::Builtin(Builtins::ExcType(handler_type)) => {
            // Check exception hierarchy using is_subclass_of
            Ok(matches!(obj_type, Type::Exception(exc_type) if exc_type.is_subclass_of(*handler_type)))
        }

        // Tuple of types (possibly nested): isinstance(x, (int, (str, bytes)))
        Value::Ref(id) => {
            if let HeapData::Tuple(tuple) = heap.get(*id) {
                for v in tuple.as_vec() {
                    if isinstance_check(obj_type, v, heap)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            } else {
                Err(()) // Not a tuple - invalid
            }
        }
        _ => Err(()), // Invalid classinfo
    }
}
