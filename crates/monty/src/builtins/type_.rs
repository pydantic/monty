//! Implementation of the type() builtin function.

use std::mem;

use super::object::builtin_object_init_subclass;
use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    types::{Class, Dict, List, OwnedTypeRef, PyTrait, Type, TypeRef, type_ref::c3_merge},
    value::Value,
};

/// Implementation of the type() builtin function.
///
/// The 1-arg form returns the type of an object; the 3-arg form
/// `type(name, bases, dict)` dynamically creates a new class, mirroring
/// CPython. Any other positional count is a `TypeError`.
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
/// For an instance of a class the type *is* the class object, so `type(x) is
/// Foo` holds via reference identity; a host class instance returns the
/// `HostClassType` entry it owns. Borrowing rather than consuming so callers
/// that only need the type of a value they still own (e.g.
/// `functools.lru_cache(typed=True)` keying) don't have to clone it first.
pub(crate) fn type_of_ref(vm: &mut VM<'_>, value: &Value) -> Value {
    value.py_class(vm).to_value(vm.heap)
}

/// The 3-arg `type(name, bases, dict)` form: dynamically creates a class,
/// and also what a `class` statement calls once its body has run.
///
/// Follows CPython's validation order (name, then bases, then dict) and
/// message wording, except that non-string namespace keys raise a `TypeError`
/// where CPython merely warns. The namespace dict is *copied* into the class,
/// minus the `__classcell__` a body referencing `__class__` passes, which is filled
/// with the new class instead. A `__doc__ = None` entry is synthesized when
/// the dict omits it, and `__hash__ = None` when it defines `__eq__` without
/// `__hash__`, as CPython's `type_new` does. Keyword arguments go to the
/// parent's `__init_subclass__`.
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
    let bases = class_bases(bases, vm)?;
    let mro = linearize(&bases, vm)?;

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
    if let Some((bad_key, _)) = source.iter().find(|(k, _)| !k.is_str(vm.heap)) {
        let name_str = class_name.as_str(vm.interns);
        let key_type = bad_key.py_type_heap(vm.heap).name(vm.heap, vm.interns);
        return Err(ExcType::type_error(format!(
            "non-string key ({key_type}) in the namespace of class '{name_str}'"
        )));
    }
    let class_cell = match source.get_by_str("__classcell__", vm.heap, vm.interns) {
        Some(Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::Cell(_)) => Some(*id),
        Some(bad) => {
            let bad_type = bad.py_type_name_heap(vm.heap, vm.interns);
            return Err(ExcType::type_error(format!(
                "__classcell__ must be a nonlocal cell, not <class '{bad_type}'>"
            )));
        }
        None => None,
    };

    // Copy the namespace (CPython semantics: the class owns an independent
    // dict). `clone_with_heap` takes `&Heap`, so the pairs can be cloned
    // while `source` still borrows the heap immutably.
    let mut pairs: Vec<(Value, Value)> = source
        .iter()
        .filter(|(k, _)| {
            k.as_either_str(vm.heap)
                .is_none_or(|k| k.as_str(vm.interns) != "__classcell__")
        })
        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
        .collect();
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

    let owned_bases = bases
        .iter()
        .map(|base| match *base {
            TypeRef::Native(ty) => OwnedTypeRef::Native(ty),
            TypeRef::Class(id) => {
                vm.heap.inc_ref(id);
                OwnedTypeRef::Class(id)
            }
        })
        .collect();
    let class = Class::with_bases(class_name, namespace_dict, owned_bases, mro.into());
    let class_value = Value::Ref(vm.heap.allocate(HeapData::Class(Box::new(class))));
    let mut class_guard = DropGuard::new(class_value, vm);
    let (class_value, vm) = class_guard.as_parts_mut();
    let class_id = class_value.ref_id().expect("classes are heap values");

    if let Some(cell_id) = class_cell {
        let HeapReadOutput::Cell(cell) = vm.heap.read(cell_id) else {
            unreachable!("__classcell__ was checked to be a cell")
        };
        let new_value = class_value.clone_with_heap(vm.heap);
        let previous = mem::replace(&mut cell.get_mut(vm.heap).0, new_value);
        previous.drop_with(vm);
    }

    let kwargs = mem::replace(kwargs, KwargsValues::Empty);
    run_init_subclass(class_id, kwargs, vm)?;
    Ok(class_guard.into_inner())
}

/// Validates the `bases` tuple of a new class, returning the bases it names.
///
/// Monty classes derive only from other classes and `object`: the native
/// types CPython lets a class subclass are rejected (see `limitations/classes.md`).
fn class_bases(bases: &Value, vm: &VM<'_>) -> RunResult<Vec<TypeRef>> {
    let tuple = match bases {
        Value::Ref(id) if let HeapData::Tuple(tuple) = vm.heap.get(*id) => tuple,
        _ => {
            let got = bases.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            return Err(ExcType::type_error_bad_arg_pos("type.__new__", 2, "tuple", got));
        }
    };
    let mut out: Vec<TypeRef> = Vec::with_capacity(tuple.as_slice().len());
    for (index, base) in tuple.as_slice().iter().enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        let Some(base) = TypeRef::from_value(base, vm.heap) else {
            return Err(ExcType::type_error(
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the \
                 metaclasses of all its bases",
            ));
        };
        let name = base.dunder_name(vm.heap, vm.interns);
        match base {
            TypeRef::Native(Type::Object) => {}
            TypeRef::Native(ty) if !ty.def().basetype => {
                return Err(ExcType::type_error(format!(
                    "type '{}' is not an acceptable base type",
                    ty.name()
                )));
            }
            TypeRef::Class(id) if matches!(vm.heap.get(id), HeapData::Class(_)) => {}
            _ => {
                return Err(ExcType::type_error(format!(
                    "Monty does not support subclassing '{name}': classes can only inherit from classes \
                     defined in Python and object"
                )));
            }
        }
        if out.contains(&base) {
            return Err(ExcType::type_error(format!("duplicate base class {name}")));
        }
        out.push(base);
    }
    Ok(out)
}

/// The C3 linearisation of a new class's ancestors, after the class itself.
fn linearize(bases: &[TypeRef], vm: &VM<'_>) -> RunResult<Vec<TypeRef>> {
    if bases.is_empty() {
        return Ok(vec![TypeRef::Native(Type::Object)]);
    }
    let mut sequences: Vec<Vec<TypeRef>> = bases.iter().map(|base| base.mro(vm.heap).collect()).collect();
    sequences.push(bases.to_vec());
    c3_merge(sequences, &vm.heap.tracker)?.map_err(|pending| {
        let names: Vec<String> = pending
            .iter()
            .map(|ty| ty.dunder_name(vm.heap, vm.interns).into_owned())
            .collect();
        ExcType::type_error(format!(
            "Cannot create a consistent method resolution order (MRO) for bases {}",
            names.join(", ")
        ))
    })
}

/// Runs the first `__init_subclass__` among the new class's ancestors, as
/// CPython's `super(cls, cls).__init_subclass__(**kwargs)`: a user hook
/// receives the class first, `object`'s rejects any keyword argument.
fn run_init_subclass(class_id: HeapId, kwargs: KwargsValues, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("run_init_subclass takes a class")
    };
    let hook = class.ancestors().iter().find_map(|ancestor| match ancestor {
        TypeRef::Class(id) => match vm.heap.get(*id) {
            HeapData::Class(base) => base.namespace().get_by_str("__init_subclass__", vm.heap, vm.interns),
            _ => None,
        },
        TypeRef::Native(_) => None,
    });
    vm.heap.inc_ref(class_id);
    let args = ArgValues::from_parts(vec![Value::Ref(class_id)], kwargs);
    match hook.map(|hook| hook.clone_with_heap(vm.heap)) {
        Some(hook) => {
            defer_drop!(hook, vm);
            let result = vm.evaluate_function(hook, args)?;
            result.drop_with(vm);
            Ok(())
        }
        None => builtin_object_init_subclass(vm, args).map(|_| ()),
    }
}

/// Arguments of `type.mro(cls, /)`.
#[derive(FromArgs)]
#[from_args(name = "type.mro")]
struct TypeMroArgs {
    #[from_args(pos_only, default)]
    cls: Option<Value>,
    #[from_args(varargs)]
    extra: Vec<Value>,
}

/// `type.mro(cls)`: the class's method resolution order, as a fresh list.
pub(crate) fn builtin_type_mro(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    // CPython's `METH_NOARGS` wording comes before any arity check.
    let args = args.reject_kwargs("type.mro", vm.heap)?;
    let TypeMroArgs { cls, extra } = TypeMroArgs::from_args(args, vm)?;
    let given = extra.len();
    extra.drop_with(vm);
    let Some(cls) = cls else {
        return Err(ExcType::type_error("unbound method type.mro() needs an argument"));
    };
    defer_drop!(cls, vm);
    let Some(ty) = TypeRef::from_value(cls, vm.heap) else {
        let got = cls.py_type_name(vm);
        return Err(ExcType::type_error(format!(
            "descriptor 'mro' for 'type' objects doesn't apply to a '{got}' object"
        )));
    };
    if given > 0 {
        let name = ty.dunder_name(vm.heap, vm.interns);
        return Err(ExcType::type_error(format!(
            "{name}.mro() takes no arguments ({given} given)"
        )));
    }
    let items: Vec<Value> = ty.mro(vm.heap).map(|ty| ty.to_value(vm.heap)).collect();
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
}
