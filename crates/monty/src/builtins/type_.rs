//! Implementation of the type() builtin function.

use super::Builtins;
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, Heap, HeapData, HeapId},
    intern::StaticStrings,
    types::{Class, Dict, PyTrait, Type},
    value::Value,
};

/// Implementation of the type() builtin function.
///
/// The 1-arg form returns the type of an object; the 3-arg form
/// `type(name, bases, dict)` dynamically creates a new class, mirroring
/// CPython (supporting user-defined classes and `object` as bases).
/// Any other positional count is a `TypeError`.
///
/// This hand-rolls `args.into_parts()` rather than using `#[derive(FromArgs)]`
/// because the "exactly 1 *or* 3 positionals, same name" overload isn't
/// expressible by any of the binder families — CPython special-cases `type`'s
/// argument parsing in `type_new`/`type_init` for the same reason.
pub fn builtin_type(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    match pos.len() {
        1 => {
            let value = pos.next().expect("length checked");
            if kwargs.is_empty() {
                Ok(type_of(vm, value))
            } else {
                value.drop_with(vm);
                kwargs.drop_with(vm);
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
            pos.drop_with(vm);
            kwargs.drop_with(vm);
            Err(ExcType::type_error("type() takes 1 or 3 arguments"))
        }
    }
}

/// The 1-arg `type(obj)` form.
///
/// For an instance of a user-defined class the type *is* the class object
/// itself, so `type(x) is Foo` holds via reference identity; a host class
/// instance returns the `HostClassType` entry it owns (one per host class, so
/// identity holds there too); everything else returns the builtin `Type`
/// marker.
fn type_of(vm: &mut VM<'_>, value: Value) -> Value {
    defer_drop!(value, vm);
    if let Value::Ref(id) = &value
        && let HeapData::Instance(inst) = vm.heap.get(*id)
    {
        let class_id = inst.class();
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else if let Value::Ref(id) = &value
        && let HeapData::NamedTuple(nt) = vm.heap.get(*id)
        && let Some(class_id) = nt.class_id()
    {
        // A factory-made namedtuple's type is its class object, so
        // `type(p) is Point` holds by identity (self-describing internal named
        // tuples like `sys.version_info` have no class and fall through).
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else if let Value::Ref(id) = &value
        && let HeapData::HostClass(hc) = vm.heap.get(*id)
    {
        let class_id = hc.class_id();
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
    vm: &mut VM<'_>,
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

    // Extract and validate base classes
    let Value::Ref(tuple_id) = &bases else {
        let got = bases.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error(format!(
            "type.__new__() argument 2 must be tuple, not {got}"
        )));
    };
    let HeapData::Tuple(tuple) = vm.heap.get(*tuple_id) else {
        let got = bases.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error(format!(
            "type.__new__() argument 2 must be tuple, not {got}"
        )));
    };

    let mut bases_ids = Vec::with_capacity(tuple.as_slice().len());
    let mut base_names = Vec::with_capacity(tuple.as_slice().len());
    let mut seen_object = false;
    for base in tuple.as_slice() {
        match base {
            Value::Builtin(Builtins::Type(Type::Object)) => {
                if seen_object {
                    return Err(ExcType::type_error("duplicate base class object"));
                }
                seen_object = true;
                base_names.push("object");
            }
            Value::Builtin(Builtins::Type(ty)) => {
                let type_name = ty.name(vm.heap, vm.interns);
                return Err(ExcType::type_error(format!(
                    "type '{type_name}' is not an acceptable base type"
                )));
            }
            Value::Builtin(Builtins::ExcType(exc_type)) => {
                let type_name: &'static str = (*exc_type).into();
                return Err(ExcType::type_error(format!(
                    "type '{type_name}' is not an acceptable base type"
                )));
            }
            Value::Ref(id) => {
                if let HeapData::Class(cls) = vm.heap.get(*id) {
                    let cls_name = cls.name().as_str(vm.interns);
                    if bases_ids.contains(id) {
                        return Err(ExcType::type_error(format!("duplicate base class {cls_name}")));
                    }
                    bases_ids.push(*id);
                    base_names.push(cls_name);
                } else {
                    let got = base.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
                    return Err(ExcType::type_error(format!(
                        "type.__new__(X): {got} is not a type object"
                    )));
                }
            }
            _ => {
                let got = base.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
                return Err(ExcType::type_error(format!(
                    "type.__new__(X): {got} is not a type object"
                )));
            }
        }
    }

    if let Some(idx) = tuple
        .as_slice()
        .iter()
        .position(|base| matches!(base, Value::Builtin(Builtins::Type(Type::Object))))
        && idx < tuple.as_slice().len() - 1
    {
        return Err(ExcType::type_error(format!(
            "Cannot create a consistent method resolution order (MRO) for bases {}",
            base_names[idx..].join(", ")
        )));
    }

    let mro = compute_c3_mro(&bases_ids, &base_names, vm.heap)?;

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
        pairs.push((
            Value::InternString(vm.interns.intern_static(StaticStrings::DunderDoc)),
            Value::None,
        ));
    }
    let namespace_dict = Dict::from_pairs(pairs, vm)?;

    for &base_id in &bases_ids {
        vm.heap.inc_ref(base_id);
    }

    let class_id = vm.heap.allocate(HeapData::Class(Box::new(Class::new(
        class_name,
        namespace_dict,
        bases_ids,
        mro,
    ))));
    Ok(Value::Ref(class_id))
}

/// Computes Python's C3 method resolution order for the given base classes.
fn compute_c3_mro(bases: &[HeapId], base_names: &[&str], heap: &Heap) -> RunResult<Vec<HeapId>> {
    let mut sequences: Vec<Vec<HeapId>> = Vec::with_capacity(bases.len() + 1);
    for &base_id in bases {
        if let HeapData::Class(base_cls) = heap.get(base_id) {
            let mut seq = Vec::with_capacity(1 + base_cls.mro().len());
            seq.push(base_id);
            seq.extend_from_slice(base_cls.mro());
            sequences.push(seq);
        }
    }
    sequences.push(bases.to_vec());

    let mut result = Vec::new();
    let mut step: usize = 0;
    loop {
        sequences.retain(|seq| !seq.is_empty());
        if sequences.is_empty() {
            break;
        }

        let mut candidate = None;
        'outer: for seq in &sequences {
            let head = seq[0];
            for other_seq in &sequences {
                for &tail_elem in &other_seq[1..] {
                    step = step.wrapping_add(1);
                    heap.tracker.check_time_every(step)?;
                    if tail_elem == head {
                        continue 'outer;
                    }
                }
            }
            candidate = Some(head);
            break;
        }

        let Some(selected) = candidate else {
            return Err(ExcType::type_error(format!(
                "Cannot create a consistent method resolution order (MRO) for bases {}",
                base_names.join(", ")
            )));
        };

        result.push(selected);
        for seq in &mut sequences {
            if seq.first() == Some(&selected) {
                seq.remove(0);
            }
        }
    }

    Ok(result)
}
