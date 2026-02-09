//! Minimal implementation of the `weakref` module.
//!
//! Provides `weakref.ref(obj)` for creating weak references to instances that
//! expose a `__weakref__` slot (either explicitly via `__slots__` or implicitly
//! by having an instance `__dict__`).

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module, PyTrait, WeakRef},
    value::Value,
};

/// Weakref module functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum WeakrefFunctions {
    Ref,
}

/// Creates the `weakref` module and allocates it on the heap.
///
/// The module provides:
/// - `ref(obj)`: create a weak reference to `obj` if supported.
///
/// # Returns
/// A HeapId pointing to the newly allocated module.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Weakref);

    module.set_attr(
        StaticStrings::Ref,
        Value::ModuleFunction(ModuleFunctions::Weakref(WeakrefFunctions::Ref)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to a weakref module function.
///
/// Returns `AttrCallResult::Value` for values computed immediately.
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
    functions: WeakrefFunctions,
    args: ArgValues,
) -> RunResult<AttrCallResult> {
    match functions {
        WeakrefFunctions::Ref => weakref_ref(heap, interns, args),
    }
}

/// Implementation of `weakref.ref(obj)`.
///
/// Returns a `weakref.ReferenceType` object that does not keep `obj` alive.
///
/// # Errors
/// Returns `TypeError` if the object does not support weak references.
fn weakref_ref(heap: &mut Heap<impl ResourceTracker>, interns: &Interns, args: ArgValues) -> RunResult<AttrCallResult> {
    let target = args.get_one_arg("weakref.ref", heap)?;

    let target_id = match target {
        Value::Ref(id) => id,
        other => {
            let type_name = other.py_type(heap);
            other.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "cannot create weak reference to '{type_name}' object"
            )));
        }
    };

    let has_weakref = match heap.get(target_id) {
        HeapData::Instance(inst) => match heap.get(inst.class_id()) {
            HeapData::ClassObject(cls) => cls.instance_has_weakref(),
            _ => false,
        },
        _ => false,
    };

    if !has_weakref {
        let type_name = match heap.get(target_id) {
            HeapData::Instance(inst) => match heap.get(inst.class_id()) {
                HeapData::ClassObject(cls) => cls.name(interns).to_string(),
                _ => "instance".to_string(),
            },
            other => other.py_type(heap).to_string(),
        };
        Value::Ref(target_id).drop_with_heap(heap);
        return Err(ExcType::type_error(format!(
            "cannot create weak reference to '{type_name}' object"
        )));
    }

    let weakref_id = heap.allocate(HeapData::WeakRef(WeakRef::new(target_id)))?;

    let register_result = heap.with_entry_mut(target_id, |_, data| {
        let HeapData::Instance(inst) = data else {
            return Err(ExcType::type_error("weakref target is not an instance".to_string()));
        };
        inst.register_weakref(weakref_id);
        Ok(())
    });

    Value::Ref(target_id).drop_with_heap(heap);

    match register_result {
        Ok(()) => Ok(AttrCallResult::Value(Value::Ref(weakref_id))),
        Err(err) => {
            Value::Ref(weakref_id).drop_with_heap(heap);
            Err(err)
        }
    }
}
