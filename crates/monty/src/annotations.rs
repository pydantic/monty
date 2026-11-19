//! Runtime side of PEP 649 annotations.
//!
//! The compiler turns a `def`'s or class body's annotations into an
//! `__annotate__(format)` function (see `Parser::synthesize_annotate`). This
//! module exposes it: `f.__annotate__` / `f.__annotations__`, a class's lazily
//! computed and cached `__annotations__`, and the private helpers frozen code
//! uses to call an annotate function in each `annotationlib.Format`.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, MissingNames, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    heap_data::FunctionDefaults,
    intern::{FunctionId, StaticStrings},
    modules::ModuleFunctions,
    types::{Dict, dict},
    value::Value,
};

/// `annotationlib.Format.VALUE`, the format `__annotations__` evaluates in.
const VALUE_FORMAT: i64 = 1;

/// `__monty_class_annotations(cls, annotate, classdict, call_annotate, cache)`:
/// evaluates a class's `__annotate__` in `VALUE` format under its namespace and
/// caches the dict, so a later access returns the same object as in CPython.
pub(crate) const CLASS_ANNOTATIONS_SOURCE: &str = r"
def __monty_class_annotations(cls, annotate, classdict, call_annotate, cache):
    result = call_annotate(annotate, classdict, 1, 0, cls, True)
    cache(cls, result)
    return result
";

/// `f.__annotate__` and `f.__annotations__` of a `def`, the only attributes
/// Monty's functions have (see `limitations/language.md`); `None` for any other
/// name. `globals` is the exec dict the `def` was made under, if any.
pub(crate) fn function_getattr(
    func_id: FunctionId,
    globals: Option<HeapId>,
    attr: &str,
    vm: &mut VM<'_>,
) -> Option<RunResult<CallResult>> {
    match attr {
        "__annotate__" => Some(Ok(CallResult::Value(function_annotate(func_id, globals, vm)))),
        "__annotations__" => Some(function_annotations(func_id, globals, vm)),
        _ => None,
    }
}

/// `f.__annotate__(format)` and `f.__annotations__(...)`: the attribute, called.
/// `None` for any other name, which has no attribute to call.
pub(crate) fn function_call_attr(
    func_id: FunctionId,
    globals: Option<HeapId>,
    attr: &str,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Option<RunResult<CallResult>> {
    // The guard releases `args` when there is no attribute or computing it fails.
    let mut args_guard = DropGuard::new(args, vm);
    let (_, vm) = args_guard.as_parts_mut();
    let callee = match function_getattr(func_id, globals, attr, vm) {
        Some(Ok(callee)) => callee.require_synchronous(vm),
        Some(Err(err)) => Err(err),
        None => return None,
    };
    let callee = match callee {
        Ok(callee) => callee,
        Err(err) => return Some(Err(err)),
    };
    let (args, vm) = args_guard.into_parts();
    defer_drop!(callee, vm);
    Some(vm.call_function(callee, args))
}

/// `f.__annotations__`: runs `__annotate__(VALUE)` on every access, since a
/// Monty function has no `__dict__` to cache the result in (see `limitations/typing.md`).
fn function_annotations(func_id: FunctionId, globals: Option<HeapId>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let annotate = function_annotate(func_id, globals, vm);
    defer_drop!(annotate, vm);
    if matches!(annotate, Value::None) {
        Ok(CallResult::Value(empty_dict(vm)))
    } else {
        vm.call_annotate(annotate, VALUE_FORMAT, None, MissingNames::Raise)
    }
}

/// The function value of a `def`'s `__annotate__`, sharing the `def`'s exec
/// globals so it resolves names where the `def` does; `None` when unannotated.
pub(crate) fn function_annotate(func_id: FunctionId, globals: Option<HeapId>, vm: &mut VM<'_>) -> Value {
    let Some(annotate_id) = vm.interns.get_function(func_id).annotate else {
        return Value::None;
    };
    match globals {
        None => Value::DefFunction(annotate_id),
        Some(dict) => {
            vm.heap.inc_ref(dict);
            let function = FunctionDefaults {
                func_id: annotate_id,
                defaults: Vec::new(),
                globals: Some(dict),
            };
            Value::Ref(vm.heap.allocate(HeapData::FunctionDefaults(function)))
        }
    }
}

/// `C.__annotations__`: the dict the class body or a caller bound, else the
/// result of its `__annotate__`, evaluated on first access and cached under
/// `__annotations_cache__` as CPython does; `{}` for an unannotated class.
pub(crate) fn class_annotations(class_id: HeapId, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if let Some(stored) = stored_class_annotations(class_id, vm) {
        return Ok(CallResult::Value(stored));
    }
    if let Some(annotate) = class_annotate(class_id, vm) {
        defer_drop!(annotate, vm);
        let classdict = class_namespace_copy(class_id, vm)?;
        defer_drop!(classdict, vm);
        vm.heap.inc_ref(class_id);
        vm.call_frozen_exact(
            FrozenFunction::ClassAnnotations,
            [
                Value::Ref(class_id),
                annotate.clone_with_heap(vm),
                classdict.clone_with_heap(vm),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::CallAnnotate)),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::CacheClassAnnotations)),
            ],
        )
    } else {
        // The guard releases the dict if caching it fails.
        let mut annotations_guard = DropGuard::new(empty_dict(vm), vm);
        let (annotations, vm) = annotations_guard.as_parts_mut();
        set_class_attr(
            class_id,
            StaticStrings::DunderAnnotationsCache,
            annotations.clone_with_heap(vm),
            vm,
        )?;
        let (annotations, _) = annotations_guard.into_parts();
        Ok(CallResult::Value(annotations))
    }
}

/// What a class's `__annotations__` or `__annotations_cache__` binds, owned:
/// the PEP 563 dict a class body stored, a dict a caller assigned, or a cached
/// evaluation. `None` when the annotations are still to be evaluated.
pub(crate) fn stored_class_annotations(class_id: HeapId, vm: &VM<'_>) -> Option<Value> {
    [StaticStrings::DunderAnnotations, StaticStrings::DunderAnnotationsCache]
        .into_iter()
        .find_map(|name| class_member(class_id, name, vm))
}

/// A class's `__annotate__`, owned: stored as `__annotate_func__` by a compiled
/// class body and as `__annotate__` by a namespace handed to `type()`, the two
/// keys CPython's `get_annotate_from_class_namespace` reads.
pub(crate) fn class_annotate(class_id: HeapId, vm: &VM<'_>) -> Option<Value> {
    [StaticStrings::DunderAnnotateFunc, StaticStrings::DunderAnnotate]
        .into_iter()
        .find_map(|name| class_member(class_id, name, vm))
        .filter(|annotate| !matches!(annotate, Value::None))
}

/// `C.__annotations__ = value`: replaces a dict the namespace binds as
/// `__annotations__`, else is stored as the cached dict; either way the class's
/// `__annotate__` is set aside, as CPython's `type.__annotations__` setter does.
pub(crate) fn set_class_annotations(class_id: HeapId, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let key = if has_class_member(class_id, StaticStrings::DunderAnnotations, vm) {
        StaticStrings::DunderAnnotations
    } else {
        StaticStrings::DunderAnnotationsCache
    };
    set_class_attr(class_id, key, value, vm)?;
    set_class_attr(class_id, StaticStrings::DunderAnnotateFunc, Value::None, vm)
}

/// A fresh heap dict holding the class namespace's entries, CPython's
/// `dict(vars(cls))`: the locals a class's annotations evaluate against.
/// The pair buffer and the dict are preflighted like a dict copy.
pub(crate) fn class_namespace_copy(class_id: HeapId, vm: &mut VM<'_>) -> RunResult<Value> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("class_namespace_copy takes a class");
    };
    dict::check_pairs_into_fresh_dict(class.namespace().len(), vm)?;
    let pairs = class
        .namespace()
        .iter()
        .map(|(name, value)| (name.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)))
        .collect();
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// Frozen helper: calls `annotate(format)` under `classdict` for `owner`, with
/// unresolved names handled per `fake` (see [`missing_names`]).
pub(crate) fn call_annotate_helper(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let CallAnnotateArgs {
        annotate,
        classdict,
        format,
        fake,
        owner,
        is_class,
    } = CallAnnotateArgs::from_args(args, vm)?;
    defer_drop!(annotate, vm);
    defer_drop!(classdict, vm);
    defer_drop!(owner, vm);
    let Value::Int(format) = format else {
        unreachable!("frozen code passes the format as an int")
    };
    let missing = missing_names(&fake, owner, matches!(is_class, Value::Bool(true)), vm);
    vm.call_annotate(annotate, format, classdict.ref_id(), missing)
}

/// What an unresolved name loads as while an `__annotate__` function runs, from
/// the fake-globals mode frozen code passes: `0` a `NameError` (the function
/// runs as written), `1` a `ForwardRef` stringifier for unresolved names, `2`
/// one for every name — CPython's `_StringifierDict` for `FORWARDREF` and
/// `STRING` — and `3` a `NameError` without asking the host.
/// `owner` is borrowed, cloned only when a stringifier needs it.
fn missing_names(fake: &Value, owner: &Value, is_class: bool, vm: &VM<'_>) -> MissingNames {
    match fake {
        Value::Int(0) => MissingNames::Raise,
        Value::Int(every_name @ (1 | 2)) => MissingNames::ForwardRef {
            owner: owner.clone_with_heap(vm.heap),
            is_class,
            every_name: *every_name == 2,
        },
        Value::Int(3) => MissingNames::NameError,
        _ => unreachable!("frozen code passes the fake-globals mode as an int"),
    }
}

/// Arguments of the private `call_annotate` helper.
#[derive(FromArgs)]
#[from_args(name = "call_annotate")]
struct CallAnnotateArgs {
    #[from_args(pos_only)]
    annotate: Value,
    #[from_args(pos_only)]
    classdict: Value,
    #[from_args(pos_only)]
    format: Value,
    #[from_args(pos_only)]
    fake: Value,
    #[from_args(pos_only)]
    owner: Value,
    #[from_args(pos_only)]
    is_class: Value,
}

/// Frozen helper: caches a class's evaluated annotations under
/// `__annotations_cache__`, rejecting a non-dict as CPython's `type.__annotations__` does.
pub(crate) fn cache_class_annotations_helper(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let (cls, annotations) = args.get_two_args("cache", vm.heap)?;
    defer_drop!(cls, vm);
    defer_drop!(annotations, vm);
    let Value::Ref(class_id) = cls else {
        unreachable!("frozen code passes the class")
    };
    if !matches!(annotations, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_))) {
        let type_name = annotations.py_type_name(vm);
        return Err(ExcType::type_error(format!(
            "__annotate__ returned non-dict of type '{type_name}'"
        )));
    }
    set_class_attr(
        *class_id,
        StaticStrings::DunderAnnotationsCache,
        annotations.clone_with_heap(vm),
        vm,
    )?;
    Ok(Value::None)
}

/// An owned clone of the class namespace member `name`, if bound.
fn class_member(class_id: HeapId, name: StaticStrings, vm: &VM<'_>) -> Option<Value> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("class_member takes a class");
    };
    class
        .namespace()
        .get_by_str(name.into(), vm.heap, vm.interns)
        .map(|value| value.clone_with_heap(vm.heap))
}

/// Whether the class namespace binds `name`.
fn has_class_member(class_id: HeapId, name: StaticStrings, vm: &VM<'_>) -> bool {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("has_class_member takes a class");
    };
    class.namespace().get_by_str(name.into(), vm.heap, vm.interns).is_some()
}

/// Binds `name` to `value` in the class namespace, taking ownership of `value`.
fn set_class_attr(class_id: HeapId, name: StaticStrings, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapReadOutput::Class(class) = vm.heap.read(class_id) else {
        unreachable!("set_class_attr takes a class");
    };
    let name = Value::InternString(vm.interns.intern_static(name));
    let replaced = class.set_attr(name, value, vm)?;
    replaced.drop_with(vm);
    Ok(())
}

/// A fresh, empty heap dict.
pub(crate) fn empty_dict(vm: &VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::Dict(Dict::new())))
}
