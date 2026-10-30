//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//! - `Union`, the type behind `int | None` (see `types/union.rs`)
//!
//! - `get_type_hints()`, evaluating the stringized annotations Monty stores
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking. `Optional[X]` is the one
//! marker that can be subscripted, producing `X | None`.

use monty_types::{OsFunctionCall, TypeHintsTarget};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions, eval_exec},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{HeapData, HeapId},
    intern::{FunctionId, StaticStrings},
    modules::ModuleFunctions,
    types::{BuiltinType, ClassAnnotations, Dict, List, Module, PyTrait, allocate_tuple},
    value::{Marker, Value},
};

/// Creates the `typing` module and allocates it on the heap.
///
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Typing, vm.interns);

    // typing.TYPE_CHECKING - always False
    module.set_attr(StaticStrings::TypeChecking, Value::Bool(false), vm);

    // `typing.Union` is the type of `int | None` (one object with
    // `types.UnionType` since 3.14), so it is a real type rather than a marker.
    module.set_attr(
        StaticStrings::UnionType,
        Value::Builtin(Builtins::Type(BuiltinType::Union)),
        vm,
    );

    // Export all typing markers as module attributes
    for ss in MARKER_ATTRS {
        module.set_attr(*ss, Value::Marker(Marker(*ss)), vm);
    }

    module.set_attr(
        StaticStrings::GetTypeHints,
        Value::ModuleFunction(ModuleFunctions::Typing(TypingFunctions::GetTypeHints)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Typing marker attributes exported by this module.
///
/// Each marker wraps its corresponding `StaticStrings` variant as both the
/// attribute name and the marker value.
const MARKER_ATTRS: &[StaticStrings] = &[
    StaticStrings::Any,
    StaticStrings::Optional,
    StaticStrings::ListType,
    StaticStrings::DictType,
    StaticStrings::TupleType,
    StaticStrings::SetType,
    StaticStrings::FrozenSet,
    StaticStrings::Callable,
    StaticStrings::Type,
    StaticStrings::Sequence,
    StaticStrings::Mapping,
    StaticStrings::Iterable,
    StaticStrings::IteratorType,
    StaticStrings::Generator,
    StaticStrings::ClassVar,
    StaticStrings::FinalType,
    StaticStrings::Literal,
    StaticStrings::TypeVar,
    StaticStrings::Generic,
    StaticStrings::Protocol,
    StaticStrings::Annotated,
    StaticStrings::SelfType,
    StaticStrings::Never,
    StaticStrings::NoReturn,
];

/// `typing` module functions, each a Python-visible callable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum TypingFunctions {
    GetTypeHints,
}

/// Dispatches a `typing` module function call.
pub(super) fn call(vm: &mut VM<'_>, function: TypingFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TypingFunctions::GetTypeHints => get_type_hints(vm, args),
    }
}

/// Evaluates stringized annotations, mirroring CPython 3.14's `typing._eval_type`.
/// Each `(name, hint, globals, locals, fwd_globals, fwd_locals, fwd_globals_first)` entry
/// evaluates a `str` hint in `globals` / `locals`; a string it yields, at any depth of a
/// `GenericAlias` / `Union`, is a forward reference evaluated in the `fwd_*` namespaces.
/// A string seen before in the chain stays a `str`, so `a = 'a'` cannot loop.
pub(crate) const GET_TYPE_HINTS_SOURCE: &str = r"
def __monty_typing_get_type_hints(entries, eval_hint, isinstance_, len_, str_, tuple_, generic_alias, union, none_type):
    hints = {}
    for name, hint, globalns, localns, fwd_globalns, fwd_localns, fwd_globals_first in entries:
        if isinstance_(hint, str_):
            hint = eval_hint(hint, globalns, localns, False, False)
        # Each frame is [hint, resolved args or None, forward refs seen, next arg index].
        stack = [[hint, None, (), 0]]
        while stack:
            frame = stack[-1]
            hint = frame[0]
            if frame[1] is None:
                seen = frame[2]
                while isinstance_(hint, str_) and hint not in seen:
                    seen = seen + (hint,)
                    hint = eval_hint(hint, fwd_globalns, fwd_localns, fwd_globals_first, True)
                if isinstance_(hint, generic_alias) or isinstance_(hint, union):
                    frame[0] = hint
                    frame[1] = []
                    frame[2] = seen
                else:
                    stack.pop()
                    if stack:
                        stack[-1][1].append(hint)
                    continue
            args = hint.__args__
            index = frame[3]
            if index < len_(args):
                frame[3] = index + 1
                stack.append([args[index], None, frame[2], 0])
                continue
            resolved = tuple_(frame[1])
            if resolved != args:
                if isinstance_(hint, generic_alias):
                    hint = hint.__origin__[resolved]
                else:
                    hint = resolved[0]
                    for arg in resolved[1:]:
                        hint = hint | arg
            stack.pop()
            if stack:
                stack[-1][1].append(hint)
        if hint is None:
            hint = none_type
        hints[name] = hint
    return hints
";

/// Arguments of `typing.get_type_hints(obj, globalns=None, localns=None, include_extras=False, *, format=None)`.
#[derive(FromArgs)]
#[from_args(name = "get_type_hints", style = def)]
struct GetTypeHintsArgs {
    obj: Value,
    #[from_args(default = Value::None)]
    globalns: Value,
    #[from_args(default = Value::None)]
    localns: Value,
    /// Accepted and ignored: Monty has no `Annotated` metadata to strip.
    #[from_args(default = Value::Bool(false))]
    include_extras: Value,
    #[from_args(kw_only, default = Value::None)]
    format: Value,
}

/// `typing.get_type_hints()`: collects `obj`'s stringized annotations and the
/// namespaces to evaluate them in, then hands them to the frozen evaluator,
/// which can push frames and suspend for host name lookups. A host function or
/// class suspends with an OS call instead: only the host can evaluate its hints.
fn get_type_hints(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let GetTypeHintsArgs {
        obj,
        globalns,
        localns,
        include_extras,
        format,
    } = GetTypeHintsArgs::from_args(args, vm)?;
    defer_drop!(obj, vm);
    defer_drop!(globalns, vm);
    defer_drop!(localns, vm);
    defer_drop!(include_extras, vm);
    defer_drop!(format, vm);

    if !matches!(format, Value::None) {
        return Err(SimpleException::new_msg(
            ExcType::NotImplementedError,
            "get_type_hints() format= is not supported",
        )
        .into());
    }
    let globalns = namespace_dict(globalns, "globalns", vm)?;
    let localns = namespace_dict(localns, "localns", vm)?;
    match hint_source(obj, vm) {
        HintSource::Function(func_id, globals) => {
            evaluate_entries(function_entries(func_id, globals, globalns, localns, vm), vm)
        }
        HintSource::Class(class_id) => evaluate_entries(class_entries(class_id, globalns, localns, vm)?, vm),
        HintSource::Host(target) => Ok(CallResult::OsCall(OsFunctionCall::GetTypeHints(target))),
        HintSource::Empty => evaluate_entries(Vec::new(), vm),
        HintSource::NotAnnotatable => Err(ExcType::no_annotations(obj, vm)),
    }
}

/// Hands evaluator `entries` to the frozen evaluator; with none, `{}` without a call.
fn evaluate_entries(entries: Vec<Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if entries.is_empty() {
        Ok(CallResult::Value(Value::Ref(
            vm.heap.allocate(HeapData::Dict(Dict::new())),
        )))
    } else {
        let entries = Value::Ref(vm.heap.allocate(HeapData::List(List::new(entries))));
        vm.call_frozen_exact(
            FrozenFunction::TypingGetTypeHints,
            [
                entries,
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::EvalHint)),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Len)),
                Value::Builtin(Builtins::Type(BuiltinType::Str)),
                Value::Builtin(Builtins::Type(BuiltinType::Tuple)),
                Value::Builtin(Builtins::Type(BuiltinType::GenericAlias)),
                Value::Builtin(Builtins::Type(BuiltinType::Union)),
                Value::Builtin(Builtins::Type(BuiltinType::NoneType)),
            ],
        )
    }
}

/// Where an object's annotations come from.
enum HintSource {
    /// A `def`'s compiled annotations, and the `exec()` globals dict it was defined under.
    Function(FunctionId, Option<HeapId>),
    /// A sandbox class's `__annotations__`.
    Class(HeapId),
    /// An `ExtFunction` or `HostClassType`, whose hints the host evaluates on request.
    Host(TypeHintsTarget),
    /// A module or callable with no annotations Monty records: `{}`.
    Empty,
    /// Neither annotated nor callable: `TypeError`, as in CPython.
    NotAnnotatable,
}

/// Classifies `obj`, unwrapping bound methods and `lru_cache` wrappers (`__wrapped__`).
///
/// A loop rather than recursion, so a deep chain of wrappers cannot exhaust the Rust stack.
fn hint_source(obj: &Value, vm: &VM<'_>) -> HintSource {
    let mut current = obj;
    loop {
        let id = match current {
            Value::DefFunction(func_id) => return HintSource::Function(*func_id, None),
            Value::Ref(id) => *id,
            value if value.is_callable(vm) => return HintSource::Empty,
            _ => return HintSource::NotAnnotatable,
        };
        current = match vm.heap.get(id) {
            HeapData::FunctionDefaults(function) => return HintSource::Function(function.func_id, function.globals),
            HeapData::Closure(closure) => return HintSource::Function(closure.func_id, closure.globals),
            HeapData::Class(_) => return HintSource::Class(id),
            HeapData::ExtFunction(function) => {
                return HintSource::Host(TypeHintsTarget::Function(function.as_str().to_owned()));
            }
            HeapData::HostClassType(class) => return HintSource::Host(TypeHintsTarget::Class(class.type_id())),
            HeapData::BoundMethod(method) => &method.func,
            HeapData::LruCache(cache) => match cache.wrapped() {
                Some(func) => func,
                None => return HintSource::Empty,
            },
            HeapData::Module(_) => return HintSource::Empty,
            _ if current.is_callable(vm) => return HintSource::Empty,
            _ => return HintSource::NotAnnotatable,
        };
    }
}

/// Entries for a `def`. An annotation evaluates in the function's own globals,
/// as its `__annotate__` would; a forward reference (a string it yields) in
/// `globalns` (default: those globals), then `localns` (default: `globalns`).
fn function_entries(
    func_id: FunctionId,
    globals: Option<HeapId>,
    globalns: Option<HeapId>,
    localns: Option<HeapId>,
    vm: &VM<'_>,
) -> Vec<Value> {
    let fwd_globals = globalns.or(globals);
    vm.interns
        .get_function(func_id)
        .annotations
        .iter()
        .map(|&(name, hint)| {
            hint_entry(
                Value::InternString(name),
                Value::InternString(hint),
                [globals, None, fwd_globals, localns],
                false,
                vm,
            )
        })
        .collect()
}

/// Entries for a class's own `__annotations__`. An annotation evaluates with a copy
/// of the class namespace (CPython's `dict(vars(cls))`) over the module globals, as
/// its `__annotate__` would; without explicit namespaces a forward reference looks
/// in the module globals first, as CPython's `get_type_hints` swaps them.
fn class_entries(
    class_id: HeapId,
    globalns: Option<HeapId>,
    localns: Option<HeapId>,
    vm: &mut VM<'_>,
) -> RunResult<Vec<Value>> {
    if class_attr_is_true(class_id, "__no_type_check__", vm)? {
        return Ok(Vec::new());
    }
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("hint_source classified a class");
    };
    let annotations = match class.annotations(vm.heap, vm.interns) {
        ClassAnnotations::Missing => return Ok(Vec::new()),
        ClassAnnotations::Dict(annotations) => annotations,
        // CPython's `annotationlib` rejects `None` here too.
        ClassAnnotations::NotADict => return Err(not_a_dict_error(class_id, vm)),
    };
    let pairs: Vec<(Value, Value)> = annotations
        .iter()
        .map(|(name, hint)| (name.clone_with_heap(vm.heap), hint.clone_with_heap(vm.heap)))
        .collect();
    defer_drop!(pairs, vm);
    // Owned by the guard until every entry holds its own reference.
    let namespace = class_namespace_copy(class_id, vm)?;
    defer_drop!(namespace, vm);

    let namespace_id = namespace.ref_id();
    let (fwd_globals, fwd_locals) = match (globalns, localns) {
        (None, None) => (None, namespace_id),
        (Some(_), None) => (globalns, namespace_id),
        (_, Some(_)) => (globalns, localns),
    };
    let fwd_globals_first = globalns.is_none() && localns.is_none();
    let scopes = [None, namespace_id, fwd_globals, fwd_locals];
    Ok(pairs
        .iter()
        .map(|(name, hint)| {
            hint_entry(
                name.clone_with_heap(vm.heap),
                hint.clone_with_heap(vm.heap),
                scopes,
                fwd_globals_first,
                vm,
            )
        })
        .collect())
}

/// Whether the class namespace binds `name` to a truthy value; `typing.no_type_check`
/// classes set `__no_type_check__`, which makes `get_type_hints` return `{}`.
fn class_attr_is_true(class_id: HeapId, name: &str, vm: &mut VM<'_>) -> RunResult<bool> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("hint_source classified a class");
    };
    let Some(value) = class.namespace().get_by_str(name, vm.heap, vm.interns) else {
        return Ok(false);
    };
    let value = value.clone_with_heap(vm.heap);
    defer_drop!(value, vm);
    value.py_bool(vm)
}

/// The `ValueError` for a class whose `__annotations__` is bound to a non-dict.
fn not_a_dict_error(class_id: HeapId, vm: &mut VM<'_>) -> RunError {
    vm.heap.inc_ref(class_id);
    let class = Value::Ref(class_id);
    defer_drop!(class, vm);
    match class.py_repr(vm) {
        Ok(repr) => {
            defer_drop!(repr, vm);
            let repr = repr.to_str(vm).map(str::to_owned).unwrap_or_default();
            ExcType::value_error(format!("{repr}.__annotations__ is neither a dict nor None"))
        }
        Err(err) => err,
    }
}

/// A fresh heap dict holding the class namespace's entries.
fn class_namespace_copy(class_id: HeapId, vm: &mut VM<'_>) -> RunResult<Value> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("hint_source classified a class");
    };
    let pairs = class
        .namespace()
        .iter()
        .map(|(name, value)| (name.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)))
        .collect();
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// One evaluator entry: `(name, hint, globals, locals, fwd_globals, fwd_locals, fwd_globals_first)`,
/// each namespace a new reference to a dict or `None`.
fn hint_entry(name: Value, hint: Value, scopes: [Option<HeapId>; 4], fwd_globals_first: bool, vm: &VM<'_>) -> Value {
    let mut items = SmallVec::new();
    items.push(name);
    items.push(hint);
    for scope in scopes {
        items.push(scope.map_or(Value::None, |id| {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        }));
    }
    items.push(Value::Bool(fwd_globals_first));
    allocate_tuple(items, vm.heap)
}

/// Validates a `globalns` / `localns` argument: `None`, or a dict whose id is returned (borrowed).
fn namespace_dict(value: &Value, param: &str, vm: &VM<'_>) -> RunResult<Option<HeapId>> {
    match value {
        Value::None => Ok(None),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Ok(Some(*id)),
        _ => Err(ExcType::type_error(format!(
            "get_type_hints() {param} must be a dict, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// Arguments of the private annotation-evaluation primitive.
#[derive(FromArgs)]
#[from_args(name = "eval_hint")]
struct EvalHintArgs {
    #[from_args(pos_only)]
    source: Value,
    #[from_args(pos_only)]
    globals: Value,
    #[from_args(pos_only)]
    locals: Value,
    #[from_args(pos_only)]
    globals_first: Value,
    #[from_args(pos_only)]
    forward_ref: Value,
}

/// Frozen helper: evaluates one annotation string; the namespaces are dicts or
/// `None`, built by [`get_type_hints`] and never seen by user code.
pub(crate) fn call_eval_hint(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let EvalHintArgs {
        source,
        globals,
        locals,
        globals_first,
        forward_ref,
    } = EvalHintArgs::from_args(args, vm)?;
    defer_drop!(source, vm);
    defer_drop!(globals, vm);
    defer_drop!(locals, vm);
    let globals_first = matches!(globals_first, Value::Bool(true));
    let forward_ref = matches!(forward_ref, Value::Bool(true));
    eval_exec::eval_hint(
        source,
        globals.ref_id(),
        locals.ref_id(),
        globals_first,
        forward_ref,
        vm,
    )
}
