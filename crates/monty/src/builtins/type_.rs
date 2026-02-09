//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    types::{ClassObject, Dict, PyTrait, Type, compute_c3_mro},
    value::{EitherStr, Value},
};

/// Implementation of the type() builtin function.
///
/// Single argument form: `type(obj)` returns the type of the object.
/// Three argument form: `type(name, bases, dict)` creates a new class.
pub fn builtin_type(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    match args {
        // 1-argument form: type(obj)
        ArgValues::One(value) => {
            defer_drop!(value, heap);
            // For user-defined class instances, return the ClassObject reference
            if let Value::Ref(heap_id) = &value
                && let HeapData::Instance(inst) = heap.get(*heap_id)
            {
                let class_id = inst.class_id();
                heap.inc_ref(class_id);
                return Ok(Value::Ref(class_id));
            }
            // For user-defined classes, return the metaclass (not always builtin type).
            if let Value::Ref(heap_id) = &value
                && let HeapData::ClassObject(cls) = heap.get(*heap_id)
            {
                return Ok(cls.metaclass().clone_with_heap(heap));
            }
            Ok(Value::Builtin(Builtins::Type(value.py_type(heap))))
        }

        // 3-argument form: type(name, bases, dict)
        ArgValues::ArgsKargs {
            args: ref positional, ..
        } if positional.len() == 3 => type_three_arg(heap, args, interns),

        _ => {
            args.drop_with_heap(heap);
            Err(ExcType::type_error("type() takes 1 or 3 arguments"))
        }
    }
}

/// Implements the 3-argument `type(name, bases, dict)` form.
///
/// Creates a new class dynamically with the given name, bases, and namespace dict.
fn type_three_arg(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let positional = match args {
        ArgValues::ArgsKargs { args, kwargs } => {
            kwargs.drop_with_heap(heap);
            args
        }
        _ => unreachable!(),
    };

    let mut iter = positional.into_iter();
    let name_val = iter.next().unwrap();
    let bases_val = iter.next().unwrap();
    let dict_val = iter.next().unwrap();

    // Extract the class name (must be a string)
    let class_name = match &name_val {
        Value::InternString(id) => EitherStr::Interned(*id),
        Value::Ref(heap_id) => {
            let s = if let HeapData::Str(s) = heap.get(*heap_id) {
                s.as_str().to_string()
            } else {
                name_val.drop_with_heap(heap);
                bases_val.drop_with_heap(heap);
                dict_val.drop_with_heap(heap);
                return Err(ExcType::type_error("type() argument 1 must be str"));
            };
            EitherStr::Heap(s)
        }
        _ => {
            name_val.drop_with_heap(heap);
            bases_val.drop_with_heap(heap);
            dict_val.drop_with_heap(heap);
            return Err(ExcType::type_error("type() argument 1 must be str"));
        }
    };
    name_val.drop_with_heap(heap);

    // Extract bases (must be a tuple of classes)
    let bases: Vec<HeapId> = if let Value::Ref(heap_id) = &bases_val {
        // Extract HeapIds from tuple elements (each must be a ClassObject)
        let tuple_len = if let HeapData::Tuple(t) = heap.get(*heap_id) {
            t.as_vec().len()
        } else {
            bases_val.drop_with_heap(heap);
            dict_val.drop_with_heap(heap);
            return Err(ExcType::type_error("type() argument 2 must be tuple"));
        };
        let mut result = Vec::new();
        for i in 0..tuple_len {
            let val = match heap.get(*heap_id) {
                HeapData::Tuple(t) => &t.as_vec()[i],
                _ => unreachable!(),
            };
            match val {
                Value::Ref(id) => {
                    if matches!(heap.get(*id), HeapData::ClassObject(_)) {
                        result.push(*id);
                    } else {
                        bases_val.drop_with_heap(heap);
                        dict_val.drop_with_heap(heap);
                        return Err(ExcType::type_error("type() argument 2 must be tuple of classes"));
                    }
                }
                Value::Builtin(Builtins::Type(t)) => {
                    let class_id = heap.builtin_class_id(*t)?;
                    result.push(class_id);
                }
                _ => {
                    bases_val.drop_with_heap(heap);
                    dict_val.drop_with_heap(heap);
                    return Err(ExcType::type_error("type() argument 2 must be tuple of classes"));
                }
            }
        }
        result
    } else {
        bases_val.drop_with_heap(heap);
        dict_val.drop_with_heap(heap);
        return Err(ExcType::type_error("type() argument 2 must be tuple"));
    };
    bases_val.drop_with_heap(heap);

    // Extract dict (must be a dict).
    // Clone entries with proper refcount handling for the new namespace.
    let namespace = if let Value::Ref(heap_id) = &dict_val {
        let heap_id = *heap_id;
        let raw_pairs: Vec<(Value, Value)> = if let HeapData::Dict(d) = heap.get(heap_id) {
            d.iter()
                .map(|(k, v)| (k.clone_with_heap(heap), v.clone_with_heap(heap)))
                .collect()
        } else {
            dict_val.drop_with_heap(heap);
            return Err(ExcType::type_error("type() argument 3 must be dict"));
        };
        Dict::from_pairs(raw_pairs, heap, interns)?
    } else {
        dict_val.drop_with_heap(heap);
        return Err(ExcType::type_error("type() argument 3 must be dict"));
    };
    dict_val.drop_with_heap(heap);

    let class_uid = heap.next_class_uid();
    // Create the ClassObject
    let class_obj = ClassObject::new(
        class_name,
        class_uid,
        Value::Builtin(Builtins::Type(Type::Type)),
        namespace,
        bases.clone(),
        vec![],
    );
    let heap_id = heap.allocate(HeapData::ClassObject(class_obj))?;

    // Compute C3 MRO
    let mro = compute_c3_mro(heap_id, &bases, heap, interns)?;
    for &mro_id in &mro {
        heap.inc_ref(mro_id);
    }
    if let HeapData::ClassObject(cls) = heap.get_mut(heap_id) {
        cls.set_mro(mro);
    }

    if bases.is_empty() {
        let object_id = heap.builtin_class_id(Type::Object)?;
        heap.with_entry_mut(object_id, |_, data| {
            let HeapData::ClassObject(cls) = data else {
                return Err(ExcType::type_error("builtin object is not a class".to_string()));
            };
            cls.register_subclass(heap_id, class_uid);
            Ok(())
        })?;
    } else {
        for &base_id in &bases {
            heap.with_entry_mut(base_id, |_, data| {
                let HeapData::ClassObject(cls) = data else {
                    return Err(ExcType::type_error("base is not a class".to_string()));
                };
                cls.register_subclass(heap_id, class_uid);
                Ok(())
            })?;
        }
    }

    Ok(Value::Ref(heap_id))
}
