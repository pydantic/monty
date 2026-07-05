//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData},
    intern::StaticStrings,
    resource::ResourceTracker,
    types::{Class, Dict, PyTrait},
    value::Value,
};

/// Implementation of the type() builtin function.
///
/// The 1-arg form returns the type of an object; the 3-arg form
/// `type(name, bases, dict)` dynamically creates a new class, mirroring
/// CPython (except that `bases` must be empty — Monty classes cannot
/// inherit). Any other positional count is a `TypeError`.
///
/// This hand-rolls `args.into_parts()` rather than using `#[derive(FromArgs)]`
/// because the "exactly 1 *or* 3 positionals, same name" overload isn't
/// expressible by any of the binder families — CPython special-cases `type`'s
/// argument parsing in `type_new`/`type_init` for the same reason.
pub fn builtin_type(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    match pos.len() {
        1 => {
            let value = pos.next().expect("length checked");
            if kwargs.is_empty() {
                Ok(type_of(vm, value))
            } else {
                value.drop_with_heap(vm);
                kwargs.drop_with_heap(vm);
                Err(ExcType::type_error_no_kwargs("type"))
            }
        }
        3 => {
            let name = pos.next().expect("length checked");
            let bases = pos.next().expect("length checked");
            let namespace = pos.next().expect("length checked");
            create_class(vm, name, bases, namespace, kwargs)
        }
        _ => {
            pos.drop_with_heap(vm);
            kwargs.drop_with_heap(vm);
            Err(ExcType::type_error("type() takes 1 or 3 arguments"))
        }
    }
}

/// The 1-arg `type(obj)` form.
///
/// For an instance of a user-defined class the type *is* the class object
/// itself, so `type(x) is Foo` holds via reference identity; for everything
/// else it returns the builtin `Type` marker.
fn type_of(vm: &mut VM<'_, impl ResourceTracker>, value: Value) -> Value {
    defer_drop!(value, vm);
    if let Value::Ref(id) = &value
        && let HeapData::Instance(inst) = vm.heap.get(*id)
    {
        let class_id = inst.class();
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else {
        Value::Builtin(Builtins::Type(value.py_type(vm)))
    }
}

/// The 3-arg `type(name, bases, dict)` form: dynamically creates a class.
///
/// Follows CPython's validation order (name, then bases, then dict, then
/// keyword rejection) and message wording (`type.__new__() argument N must
/// be ...`), except that non-string namespace keys raise a `TypeError`
/// where CPython merely warns. The namespace dict is *copied* into the
/// class — later mutation of the source dict must not affect the class —
/// and a `__doc__ = None` entry is synthesized when the dict omits it,
/// matching CPython's `type` descriptor default (compiled `class` bodies
/// get their `__doc__` from the parser instead).
fn create_class(
    vm: &mut VM<'_, impl ResourceTracker>,
    name: Value,
    bases: Value,
    namespace: Value,
    kwargs: KwargsValues,
) -> RunResult<Value> {
    defer_drop!(name, vm);
    defer_drop!(bases, vm);
    defer_drop!(namespace, vm);
    defer_drop!(kwargs, vm);

    let Some(class_name) = name.as_either_str(vm.heap) else {
        let got = name.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 1, "str", got));
    };

    match bases {
        Value::Ref(id) if let HeapData::Tuple(t) = vm.heap.get(*id) => {
            // Monty divergence: classes cannot inherit, so even `(object,)` is
            // rejected — the parse-time equivalent (`class Foo(Bar)`) is a
            // syntax error, and this is its runtime counterpart.
            if !t.as_slice().is_empty() {
                return Err(ExcType::type_error("type() bases are not supported"));
            }
        }
        _ => {
            let got = bases.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            return Err(ExcType::type_error_bad_arg_pos("type.__new__", 2, "tuple", got));
        }
    }

    let Value::Ref(ns_id) = namespace else {
        let got = namespace.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 3, "dict", got));
    };
    let HeapData::Dict(source) = vm.heap.get(*ns_id) else {
        let got = namespace.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 3, "dict", got));
    };

    if !kwargs.is_empty() {
        // CPython forwards extra keywords to `__init_subclass__`, which
        // `object` rejects with this message — synthesize the equivalent.
        let name_str = class_name.as_str(vm.interns);
        return Err(ExcType::type_error_no_kwargs(&format!("{name_str}.__init_subclass__")));
    }

    // Monty divergence: CPython only emits a `RuntimeWarning` for non-string
    // namespace keys; Monty has no warnings machinery, so silently accepting
    // them would hide the mistake — raise instead. Validated before cloning
    // any pairs so the error path has nothing to clean up.
    if let Some((bad_key, _)) = source.iter().find(|(k, _)| !k.is_str(vm.heap)) {
        let name_str = class_name.as_str(vm.interns);
        let key_type = bad_key.py_type_heap(vm.heap).name(vm.heap, vm.interns);
        return Err(ExcType::type_error(format!(
            "non-string key ({key_type}) in the namespace of class '{name_str}'"
        )));
    }

    // Copy the namespace (CPython semantics: the class owns an independent
    // dict). `clone_with_heap` takes `&Heap`, so the pairs can be cloned
    // while `source` still borrows the heap immutably.
    let mut pairs: Vec<(Value, Value)> = source
        .iter()
        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
        .collect();
    if source.get_by_str("__doc__", vm.heap, vm.interns).is_none() {
        pairs.push((Value::InternString(StaticStrings::DunderDoc.into()), Value::None));
    }
    let namespace_dict = Dict::from_pairs(pairs, vm)?;

    let class_id = vm
        .heap
        .allocate(HeapData::Class(Class::new(class_name, namespace_dict)))?;
    Ok(Value::Ref(class_id))
}
