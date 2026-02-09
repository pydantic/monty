//! Implementation of the isinstance() and issubclass() builtin functions.

use super::Builtins;
use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    resource::ResourceTracker,
    types::{PyTrait, Type},
    value::Value,
};

/// Implementation of the isinstance() builtin function.
///
/// Checks if an object is an instance of a class or a tuple of classes.
/// For user-defined class instances, checks the instance's class MRO
/// to support inheritance (isinstance(dog, Animal) == True when Dog(Animal)).
pub fn builtin_isinstance(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (obj, classinfo) = args.get_two_args("isinstance", heap)?;
    defer_drop!(obj, heap);
    defer_drop!(classinfo, heap);

    // For user-defined class instances, extract the class_id and MRO for matching
    let instance_class_id = if let Value::Ref(heap_id) = &obj {
        if let HeapData::Instance(inst) = heap.get(*heap_id) {
            Some(inst.class_id())
        } else {
            None
        }
    } else {
        None
    };

    let obj_type = obj.py_type(heap);

    match isinstance_check(obj_type, instance_class_id, classinfo, heap)? {
        Some(result) => Ok(Value::Bool(result)),
        None => Err(ExcType::isinstance_arg2_error()),
    }
}

/// Implementation of the issubclass() builtin function.
///
/// Checks if a class is a subclass of another class or tuple of classes.
/// Uses MRO for user-defined classes.
pub fn builtin_issubclass(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (cls_val, classinfo) = args.get_two_args("issubclass", heap)?;
    defer_drop!(cls_val, heap);
    defer_drop!(classinfo, heap);

    match issubclass_check(cls_val, classinfo, heap)? {
        Some(result) => Ok(Value::Bool(result)),
        None => Err(ExcType::type_error(
            "issubclass() arg 2 must be a class, a tuple of classes, or a union".to_string(),
        )),
    }
}

/// Recursively checks if obj_type matches classinfo for isinstance().
///
/// Returns `Ok(true)` if the type matches, `Ok(false)` if it doesn't,
/// or `Err(())` if classinfo is invalid (not a type or tuple of types).
///
/// Supports:
/// - Single types: `isinstance(x, int)`
/// - Exception types: `isinstance(err, ValueError)` or `isinstance(err, LookupError)`
/// - User-defined classes with MRO: `isinstance(dog, Animal)` when Dog inherits from Animal
/// - Nested tuples: `isinstance(x, (int, (str, bytes)))`
/// - `object` type: all instances are instances of `object`
fn isinstance_check(
    obj_type: Type,
    instance_class_id: Option<HeapId>,
    classinfo: &Value,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<Option<bool>> {
    match classinfo {
        // Single builtin type: isinstance(x, int)
        Value::Builtin(Builtins::Type(t)) => {
            // Special case: isinstance(instance, object) is always True for user class instances
            if *t == Type::Object && instance_class_id.is_some() {
                return Ok(Some(true));
            }
            if let Some(inst_cls_id) = instance_class_id {
                let builtin_id = heap.builtin_class_id(*t)?;
                if let HeapData::ClassObject(inst_cls) = heap.get(inst_cls_id) {
                    return Ok(Some(inst_cls.is_subclass_of(inst_cls_id, builtin_id)));
                }
                return Ok(Some(false));
            }
            Ok(Some(obj_type.is_instance_of(*t)))
        }

        // Exception type: isinstance(err, ValueError) or isinstance(err, LookupError)
        Value::Builtin(Builtins::ExcType(handler_type)) => {
            // Check exception hierarchy using is_subclass_of
            Ok(Some(matches!(
                obj_type,
                Type::Exception(exc_type) if exc_type.is_subclass_of(*handler_type)
            )))
        }

        // Ref could be a ClassObject (user class) or a Tuple
        Value::Ref(id) => {
            match heap.get(*id) {
                // User-defined class: isinstance(obj, MyClass)
                HeapData::ClassObject(_) => {
                    // Check if the instance's class is this class or a subclass (via MRO)
                    if let Some(inst_cls_id) = instance_class_id {
                        if inst_cls_id == *id {
                            return Ok(Some(true));
                        }
                        // Check if inst_cls_id's MRO contains *id
                        if let HeapData::ClassObject(inst_cls) = heap.get(inst_cls_id) {
                            Ok(Some(inst_cls.is_subclass_of(inst_cls_id, *id)))
                        } else {
                            Ok(Some(false))
                        }
                    } else {
                        Ok(Some(false))
                    }
                }
                // Tuple of types (possibly nested): isinstance(x, (int, (str, bytes)))
                HeapData::Tuple(tuple) => {
                    let items: Vec<Value> = tuple.as_vec().iter().map(Value::copy_for_extend).collect();
                    tuple_any(items, heap, |value, heap| {
                        isinstance_check(obj_type, instance_class_id, value, heap)
                    })
                }
                _ => Ok(None), // Not a class or tuple - invalid
            }
        }
        _ => Ok(None), // Invalid classinfo
    }
}

/// Checks if cls is a subclass of classinfo for issubclass().
///
/// Supports user-defined classes with MRO, builtin types, and tuples.
fn issubclass_check(cls: &Value, classinfo: &Value, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<bool>> {
    // Get the class HeapId (cls must be a class)
    let cls_id = match cls {
        Value::Ref(id) => {
            if matches!(heap.get(*id), HeapData::ClassObject(_)) {
                Some(*id)
            } else {
                return Ok(None);
            }
        }
        Value::Builtin(Builtins::Type(t)) => {
            // Builtin type: issubclass(int, object) etc
            return issubclass_builtin_check(*t, classinfo, heap);
        }
        _ => return Ok(None),
    };
    let Some(cls_id) = cls_id else {
        return Ok(None);
    };

    match classinfo {
        // issubclass(MyClass, object)
        Value::Builtin(Builtins::Type(info_t)) => {
            let builtin_id = heap.builtin_class_id(*info_t)?;
            if let HeapData::ClassObject(cls_obj) = heap.get(cls_id) {
                Ok(Some(cls_obj.is_subclass_of(cls_id, builtin_id)))
            } else {
                Ok(Some(false))
            }
        }

        Value::Ref(info_id) => {
            match heap.get(*info_id) {
                HeapData::ClassObject(_) => {
                    // Check MRO
                    if let HeapData::ClassObject(cls_obj) = heap.get(cls_id) {
                        Ok(Some(cls_obj.is_subclass_of(cls_id, *info_id)))
                    } else {
                        Ok(Some(false))
                    }
                }
                HeapData::Tuple(tuple) => {
                    let items: Vec<Value> = tuple.as_vec().iter().map(Value::copy_for_extend).collect();
                    tuple_any(items, heap, |value, heap| issubclass_check(cls, value, heap))
                }
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

/// Checks issubclass for builtin types (e.g., issubclass(bool, int)).
fn issubclass_builtin_check(
    t: Type,
    classinfo: &Value,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<Option<bool>> {
    match classinfo {
        Value::Builtin(Builtins::Type(info_t)) => Ok(Some(t.is_instance_of(*info_t))),
        Value::Ref(id) => {
            match heap.get(*id) {
                HeapData::Tuple(tuple) => {
                    let items: Vec<Value> = tuple.as_vec().iter().map(Value::copy_for_extend).collect();
                    tuple_any(items, heap, |value, heap| issubclass_builtin_check(t, value, heap))
                }
                // Builtin types are never subclasses of user-defined classes
                HeapData::ClassObject(_) => Ok(Some(false)),
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

/// Processes tuple classinfo entries, ensuring temporary values are refcount-safe.
///
/// Copies values with `copy_for_extend()`, increments refs before use, then
/// drops them with the heap regardless of early returns.
fn tuple_any<T: ResourceTracker, F>(items: Vec<Value>, heap: &mut Heap<T>, mut check: F) -> RunResult<Option<bool>>
where
    F: FnMut(&Value, &mut Heap<T>) -> RunResult<Option<bool>>,
{
    let mut iter = items.into_iter();
    while let Some(item) = iter.next() {
        if let Value::Ref(id) = &item {
            heap.inc_ref(*id);
        }
        let result = match check(&item, heap) {
            Ok(value) => value,
            Err(err) => {
                item.drop_with_heap(heap);
                drop_copied_values(iter, heap);
                return Err(err);
            }
        };
        item.drop_with_heap(heap);
        match result {
            Some(true) => {
                drop_copied_values(iter, heap);
                return Ok(Some(true));
            }
            None => {
                drop_copied_values(iter, heap);
                return Ok(None);
            }
            Some(false) => {}
        }
    }
    Ok(Some(false))
}

/// Drops copied tuple values safely by balancing temporary refcounts.
fn drop_copied_values<T: ResourceTracker, I: Iterator<Item = Value>>(iter: I, heap: &mut Heap<T>) {
    for value in iter {
        if let Value::Ref(id) = &value {
            heap.inc_ref(*id);
        }
        value.drop_with_heap(heap);
    }
}
