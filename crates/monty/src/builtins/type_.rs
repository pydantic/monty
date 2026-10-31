//! Implementation of the type() builtin function.

use std::mem;

use super::{Builtins, BuiltinsFunctions};
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, Heap, HeapData, HeapId},
    intern::StaticStrings,
    types::{BuiltinType, Class, Dict, PyTrait, Type, class::ClassBase},
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
fn type_of(vm: &mut VM<'_>, value: Value) -> Value {
    defer_drop!(value, vm);
    type_of_ref(vm, value)
}

/// The type of a borrowed value.
///
/// For an instance of a user-defined class the type *is* the class object
/// itself, so `type(x) is Foo` holds via reference identity; a host class
/// instance returns the `HostClassType` entry it owns (one per host class, so
/// identity holds there too); everything else returns the builtin `Type`
/// marker. Borrowing rather than consuming so callers that only need the type
/// of a value they still own (e.g. `functools.lru_cache(typed=True)` keying)
/// don't have to clone it first.
pub(crate) fn type_of_ref(vm: &mut VM<'_>, value: &Value) -> Value {
    if let Value::Ref(id) = value
        && let HeapData::Instance(inst) = vm.heap.get(*id)
    {
        let class_id = inst.class();
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else if let Value::Ref(id) = value
        && let HeapData::NamedTuple(nt) = vm.heap.get(*id)
        && let Some(class_id) = nt.class_id()
    {
        // A factory-made namedtuple's type is its class object, so
        // `type(p) is Point` holds by identity (self-describing internal named
        // tuples like `sys.version_info` have no class and fall through).
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else if let Value::Ref(id) = value
        && let HeapData::HostClass(hc) = vm.heap.get(*id)
    {
        let class_id = hc.class_id();
        vm.heap.inc_ref(class_id);
        Value::Ref(class_id)
    } else {
        let t = value.py_type(vm);
        if t == Type::Type {
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Type))
        } else {
            Value::Builtin(Builtins::Type(
                t.try_into().expect("instances return their class object"),
            ))
        }
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
    defer_drop_mut!(kwargs, vm);

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

    let mut bases = Vec::with_capacity(tuple.as_slice().len());
    let mut base_names = Vec::with_capacity(tuple.as_slice().len());
    let mut seen_object = false;
    let mut seen_layout_root: Option<ExcType> = None;
    for base in tuple.as_slice() {
        match base {
            Value::Builtin(Builtins::Type(BuiltinType::Object)) => {
                if seen_object {
                    return Err(ExcType::type_error("duplicate base class object"));
                }
                seen_object = true;
                bases.push(ClassBase::Object);
                base_names.push("object");
            }
            Value::Builtin(Builtins::Type(ty)) => {
                let type_name = Type::from(*ty).name(vm.heap, vm.interns);
                return Err(ExcType::type_error(format!(
                    "type '{type_name}' is not an acceptable base type"
                )));
            }
            Value::Builtin(Builtins::ExcType(exc_type)) => {
                let type_name: &'static str = (*exc_type).into();
                if bases.contains(&ClassBase::BuiltinExc(*exc_type)) {
                    return Err(ExcType::type_error(format!("duplicate base class {type_name}")));
                }
                if let Some(layout_root) = exception_layout_root(*exc_type) {
                    if let Some(existing) = seen_layout_root {
                        if existing != layout_root {
                            return Err(ExcType::type_error("multiple bases have instance lay-out conflict"));
                        }
                    } else {
                        seen_layout_root = Some(layout_root);
                    }
                }
                bases.push(ClassBase::BuiltinExc(*exc_type));
                base_names.push(type_name);
            }
            Value::Ref(id) => {
                if let HeapData::Class(cls) = vm.heap.get(*id) {
                    let cls_name = cls.name().as_str(vm.interns);
                    if bases.contains(&ClassBase::Class(*id)) {
                        return Err(ExcType::type_error(format!("duplicate base class {cls_name}")));
                    }
                    if let Some(parent_exc) = cls.base_exception()
                        && let Some(layout_root) = exception_layout_root(parent_exc)
                    {
                        if let Some(existing) = seen_layout_root {
                            if existing != layout_root {
                                return Err(ExcType::type_error("multiple bases have instance lay-out conflict"));
                            }
                        } else {
                            seen_layout_root = Some(layout_root);
                        }
                    }
                    bases.push(ClassBase::Class(*id));
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

    if let Some(idx) = bases.iter().position(|base| matches!(base, ClassBase::Object))
        && idx < bases.len() - 1
    {
        return Err(ExcType::type_error(format!(
            "Cannot create a consistent method resolution order (MRO) for bases {}",
            base_names[idx..].join(", ")
        )));
    }

    let c3_bases = if bases.is_empty() {
        vec![ClassBase::Object]
    } else {
        bases.clone()
    };
    let mut mro = compute_c3_mro(&c3_bases, &base_names, vm.heap)?;
    if !mro.contains(&ClassBase::Object) {
        mro.push(ClassBase::Object);
    }

    let Value::Ref(ns_id) = namespace else {
        let got = namespace.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 3, "dict", got));
    };
    let HeapData::Dict(source) = vm.heap.get(*ns_id) else {
        let got = namespace.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 3, "dict", got));
    };

    // Monty divergence: CPython only emits a `RuntimeWarning` for non-string
    // namespace keys; Monty has no warnings machinery, so silently accepting
    // them would hide the mistake — raise instead. Validated before cloning
    // any pairs so the error path has nothing to clean up.
    for (i, (k, _)) in source.iter().enumerate() {
        vm.heap.tracker.check_time_every(i)?;
        if !k.is_str(vm.heap) {
            let name_str = class_name.as_str(vm.interns);
            let key_type = k.py_type_heap(vm.heap).name(vm.heap, vm.interns);
            return Err(ExcType::type_error(format!(
                "non-string key ({key_type}) in the namespace of class '{name_str}'"
            )));
        }
    }

    let mut inherited_methods = ahash::AHashSet::new();
    let mut method_idx = 0;
    for base in &mro {
        if let ClassBase::Class(ancestor_id) = base
            && let HeapData::Class(ancestor_cls) = vm.heap.get(*ancestor_id)
        {
            for &fid in ancestor_cls.defined_methods() {
                vm.heap.tracker.check_time_every(method_idx)?;
                method_idx += 1;
                inherited_methods.insert(fid);
            }
        }
    }

    let mut defined_methods = Vec::new();
    let mut seen_methods = ahash::AHashSet::new();
    for (i, (_, val)) in source.iter().enumerate() {
        vm.heap.tracker.check_time_every(i)?;
        let fid = match val {
            Value::DefFunction(fid) => Some(*fid),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Closure(c) => Some(c.func_id),
                HeapData::FunctionDefaults(d) => Some(d.func_id),
                _ => None,
            },
            _ => None,
        };
        if let Some(fid) = fid
            && !inherited_methods.contains(&fid)
            && seen_methods.insert(fid)
        {
            defined_methods.push(fid);
        }
    }

    // Copy the namespace (CPython semantics: the class owns an independent
    // dict). Check the deadline once beforehand so that building the cloned
    // pairs cannot exit early and leak cloned heap references.
    vm.heap.tracker.check_time()?;
    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(source.len() + 2);
    pairs.extend(
        source
            .iter()
            .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap))),
    );
    if source.get_by_str("__doc__", vm.heap, vm.interns).is_none() {
        pairs.push((
            Value::InternString(vm.interns.intern_static(StaticStrings::DunderDoc)),
            Value::None,
        ));
    }
    if source.get_by_str("__eq__", vm.heap, vm.interns).is_some()
        && source.get_by_str("__hash__", vm.heap, vm.interns).is_none()
    {
        pairs.push((
            Value::InternString(vm.interns.intern_static(StaticStrings::DunderHash)),
            Value::None,
        ));
    }
    let namespace_dict = Dict::from_pairs(pairs, vm)?;

    let bases_ids: Vec<HeapId> = bases
        .iter()
        .filter_map(|b| match b {
            ClassBase::Class(id) => Some(*id),
            _ => None,
        })
        .collect();

    for &base_id in &bases_ids {
        vm.heap.inc_ref(base_id);
    }

    let class_id = vm.heap.allocate(HeapData::Class(Box::new(Class::new(
        class_name.clone(),
        namespace_dict,
        bases_ids,
        bases,
        mro.clone(),
        defined_methods,
    ))));

    // Look for __init_subclass__ in base classes' MRO
    let mut init_subclass = None;
    for base in &mro {
        if let ClassBase::Class(base_id) = base
            && let HeapData::Class(base_cls) = vm.heap.get(*base_id)
            && let Some(val) = base_cls
                .namespace()
                .get_by_str("__init_subclass__", vm.heap, vm.interns)
        {
            init_subclass = Some(val.clone_with_heap(vm.heap));
            break;
        }
    }

    let kwargs = mem::replace(kwargs, KwargsValues::Empty);
    if let Some(init_subclass_fn) = init_subclass {
        defer_drop!(init_subclass_fn, vm);
        vm.heap.inc_ref(class_id);
        let arg_values = ArgValues::from_parts(vec![Value::Ref(class_id)], kwargs);
        match vm.evaluate_function(init_subclass_fn, arg_values) {
            Ok(ret) => {
                ret.drop_with(vm.heap);
            }
            Err(e) => {
                Value::Ref(class_id).drop_with(vm.heap);
                return Err(e);
            }
        }
    } else if !kwargs.is_empty() {
        kwargs.drop_with(vm.heap);
        let name_str = class_name.as_str(vm.interns);
        Value::Ref(class_id).drop_with(vm.heap);
        return Err(ExcType::type_error_no_kwargs(&format!("{name_str}.__init_subclass__")));
    } else {
        kwargs.drop_with(vm.heap);
    }

    Ok(Value::Ref(class_id))
}

/// Computes Python's C3 method resolution order for the given base classes.
fn compute_c3_mro(bases: &[ClassBase], base_names: &[&str], heap: &Heap) -> RunResult<Vec<ClassBase>> {
    let mut sequences: Vec<Vec<ClassBase>> = Vec::with_capacity(bases.len() + 1);
    for &base in bases {
        match base {
            ClassBase::Class(base_id) => {
                if let HeapData::Class(base_cls) = heap.get(base_id) {
                    let mut seq = Vec::with_capacity(1 + base_cls.mro().len());
                    seq.push(ClassBase::Class(base_id));
                    seq.extend_from_slice(base_cls.mro());
                    sequences.push(seq);
                }
            }
            ClassBase::BuiltinExc(exc) => {
                let mut seq = Vec::new();
                let mut cur = Some(exc);
                while let Some(e) = cur {
                    seq.push(ClassBase::BuiltinExc(e));
                    cur = e.parent();
                }
                seq.push(ClassBase::Object);
                sequences.push(seq);
            }
            ClassBase::Object => {
                sequences.push(vec![ClassBase::Object]);
            }
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

/// Returns the layout root for built-in exceptions that have custom instance layouts in CPython.
///
/// Standard exceptions (such as `KeyError`, `AttributeError`, `ValueError`, `TypeError`,
/// `LookupError`, `ArithmeticError`, `Exception`, etc.) share the common `BaseException` layout
/// and do not conflict with each other or with specialized types unless two conflicting
/// specialized layouts are combined (e.g. `OSError` with `SyntaxError` or `SystemExit`).
fn exception_layout_root(exc: ExcType) -> Option<ExcType> {
    match exc {
        ExcType::SystemExit => Some(ExcType::SystemExit),
        ExcType::SyntaxError => Some(ExcType::SyntaxError),
        ExcType::UnicodeDecodeError | ExcType::UnicodeEncodeError => Some(ExcType::UnicodeDecodeError),
        ExcType::OSError
        | ExcType::FileNotFoundError
        | ExcType::FileExistsError
        | ExcType::IsADirectoryError
        | ExcType::NotADirectoryError
        | ExcType::PermissionError
        | ExcType::TimeoutError
        | ExcType::UnsupportedOperation => Some(ExcType::OSError),
        _ => None,
    }
}
