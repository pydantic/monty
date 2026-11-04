//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//! - `Union`, the type behind `int | None` (see `types/union.rs`)
//! - `ForwardRef`, shared with `annotationlib` (see `types/forward_ref.rs`)
//! - `get_type_hints()`, evaluating annotations from their `__annotate__`
//!   function (PEP 649) in any `annotationlib.Format`;
//!   `annotationlib.get_annotations()` shares its machinery through [`collect_hints`]
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking. `Optional[X]` is the one
//! marker that can be subscripted, producing `X | None`.

use monty_types::{OsFunctionCall, TypeHintsTarget};
use smallvec::SmallVec;

use crate::{
    annotations::{self, empty_dict},
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions, eval_exec},
    bytecode::{CallResult, MissingNames, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{HeapData, HeapId},
    intern::{FunctionId, StaticStrings},
    modules::ModuleFunctions,
    types::{
        BuiltinType, LazyHeapSet, Module, PyTrait, allocate_string, allocate_tuple, forward_ref::format_code,
        generic_alias::repr_type_arg,
    },
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
    // `typing.ForwardRef is annotationlib.ForwardRef` in CPython.
    module.set_attr(
        StaticStrings::ForwardRef,
        Value::Builtin(Builtins::Type(BuiltinType::ForwardRef)),
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

/// Evaluates annotations as CPython 3.14's `annotationlib.get_annotations` and
/// `typing.get_type_hints` do, from an `__annotate__` function or a stored dict.
///
/// `annotate` and `stored` are the object's `__annotate__` function and bound
/// `__annotations__` dict (`None` when absent), `classdict` a copy of a class's
/// namespace, and `scopes` `(globals, locals, globals_first)`: where a forward
/// reference evaluates. `format_` is the `annotationlib.Format` code, `task` 0
/// for `get_annotations`, 1 for it with `eval_str=True` and 2 for
/// `get_type_hints`. `call_annotate` runs the function with a format and a
/// fake-globals mode (0 none, 1 stringify unresolved names, 2 every name, 3
/// raise `NameError` without asking the host);
/// `cache`, for a class, stores a plain evaluation as CPython's
/// `type.__annotations__` would. `eval_hint`'s last three arguments are the
/// name mode, the owner and the class flag.
pub(crate) const GET_ANNOTATIONS_SOURCE: &str = r"
def __monty_get_annotations(owner, annotate, stored, classdict, scopes, format_, task, include_extras, is_class, call_annotate, cache, eval_hint, alias_with, forward_ref, type_repr_, isinstance_, len_, str_, tuple_, dict_, repr_, generic_alias, literal_, union, none_type, exception, not_implemented_error, syntax_error, value_error):
    VALUE = 1
    FAKE_GLOBALS = 2
    FORWARDREF = 3
    STRING = 4
    fwd_globalns, fwd_localns, fwd_globals_first = scopes
    # `annotationlib.get_annotations`: a stored dict is reported as is (STRING
    # prefers the function, so a cached evaluation never hides the source);
    # otherwise `__annotate__` runs, first asked for the format itself (a
    # hand-written one may support it), then under fake globals
    # (`call_annotate_function`).
    if format_ == STRING:
        if annotate is None:
            annotations = stored
        else:
            try:
                annotations = call_annotate(annotate, classdict, STRING, 0, owner, is_class)
            except not_implemented_error:
                try:
                    annotations = call_annotate(annotate, classdict, FAKE_GLOBALS, 2, owner, is_class)
                except not_implemented_error:
                    annotations = call_annotate(annotate, classdict, VALUE, 0, owner, is_class)
            if not isinstance_(annotations, dict_):
                raise value_error(repr_(owner) + '.__annotate__ returned a non-dict')
        result = {}
        for name, value in annotations.items():
            if isinstance_(value, forward_ref):
                value = value.__forward_arg__
            elif value is ...:
                value = '...'
            elif not isinstance_(value, str_):
                value = type_repr_(value)
            result[name] = value
        return result
    if stored is not None:
        annotations = stored
    elif format_ == VALUE:
        annotations = call_annotate(annotate, classdict, VALUE, 0, owner, is_class)
        if cache is not None:
            cache(owner, annotations)
    else:
        # FORWARDREF: the plain evaluation `__annotations__` makes first, which a
        # class caches; it raises `NameError` where it would ask the host for a
        # name, so this format never does. Then `call_annotate_function`: the
        # function's own support, else fake globals stringifying unresolved
        # names, and if that run raises, every name is stringified and each
        # reference then evaluated on its own.
        # A non-dict from the plain evaluation is the `TypeError` `__annotations__`
        # raises, which `get_annotations` swallows like any other failure.
        plain = True
        try:
            annotations = call_annotate(annotate, classdict, VALUE, 3, owner, is_class)
        except exception:
            plain = False
        if plain and not isinstance_(annotations, dict_):
            plain = False
        if plain:
            if cache is not None:
                cache(owner, annotations)
        else:
            try:
                annotations = call_annotate(annotate, classdict, FORWARDREF, 0, owner, is_class)
            except not_implemented_error:
                try:
                    annotations = call_annotate(annotate, classdict, FAKE_GLOBALS, 1, owner, is_class)
                except exception:
                    annotations = call_annotate(annotate, classdict, FAKE_GLOBALS, 2, owner, is_class)
                    if not isinstance_(annotations, dict_):
                        raise value_error(repr_(owner) + '.__annotate__ returned a non-dict')
                    evaluated = {}
                    for name, value in annotations.items():
                        if isinstance_(value, forward_ref):
                            value = value.evaluate(format=FORWARDREF)
                        evaluated[name] = value
                    annotations = evaluated
            if not isinstance_(annotations, dict_):
                raise value_error(repr_(owner) + '.__annotate__ returned a non-dict')
    # `get_annotations(eval_str=True)` evaluates string values with plain `eval()`.
    if task == 1:
        result = {}
        for name, value in annotations.items():
            if isinstance_(value, str_):
                value = eval_hint(value, fwd_globalns, fwd_localns, False, False, 0, None, False)
            result[name] = value
        return result
    if task != 2:
        result = {}
        for name, value in annotations.items():
            result[name] = value
        return result
    # Pass 2, `typing._eval_type`: resolve forward references at any depth of a
    # generic alias or union (`alias_with` also strips `Annotated` unless
    # `include_extras`). An explicit stack, since frozen code cannot recurse; each frame is
    # [value, resolved args or None, texts seen, next index, parent reference].
    result = {}
    for name, value in annotations.items():
        if isinstance_(value, str_):
            value = forward_ref(value, is_argument=not is_class, is_class=is_class)
        stack = [[value, None, (), 0, None]]
        while stack:
            frame = stack[-1]
            value = frame[0]
            if frame[1] is None:
                seen = frame[2]
                parent = frame[4]
                # `evaluate_forward_ref`: follow the chain of references, stopping
                # at one seen before (`a = 'a'`) or, in FORWARDREF, one that stays unresolved.
                while isinstance_(value, forward_ref):
                    text = value.__forward_arg__
                    if text in seen:
                        break
                    if format_ == FORWARDREF:
                        try:
                            evaluated = eval_hint(text, fwd_globalns, fwd_localns, fwd_globals_first, True, 1, owner, value.__forward_is_class__)
                        except syntax_error:
                            raise
                        except exception:
                            break
                        if isinstance_(evaluated, forward_ref):
                            if not text.isidentifier():
                                value = evaluated
                            break
                    else:
                        evaluated = eval_hint(text, fwd_globalns, fwd_localns, fwd_globals_first, True, 0, None, False)
                    seen = seen + (text,)
                    parent = value
                    if isinstance_(evaluated, str_):
                        evaluated = forward_ref(evaluated, module=value.__forward_module__, owner=owner, is_argument=value.__forward_is_argument__, is_class=value.__forward_is_class__)
                    value = evaluated
                # A `Literal` holds values, not types: its strings stay strings.
                if (isinstance_(value, generic_alias) and value.__origin__ is not literal_) or isinstance_(value, union):
                    frame[0] = value
                    frame[1] = []
                    frame[2] = seen
                    frame[4] = parent
                else:
                    stack.pop()
                    if stack:
                        stack[-1][1].append(value)
                    continue
            args = value.__args__
            index = frame[3]
            if index < len_(args):
                frame[3] = index + 1
                arg = args[index]
                if isinstance_(arg, str_):
                    parent = frame[4]
                    if parent is None or isinstance_(value, union):
                        arg = forward_ref(arg)
                    else:
                        arg = forward_ref(arg, module=parent.__forward_module__, owner=parent.__owner__)
                stack.append([arg, None, frame[2], 0, None])
                continue
            resolved = tuple_(frame[1])
            if isinstance_(value, generic_alias):
                value = alias_with(value, resolved, include_extras)
            elif resolved != args:
                value = resolved[0]
                for member in resolved[1:]:
                    value = value | member
            stack.pop()
            if stack:
                stack[-1][1].append(value)
        if value is None:
            value = none_type
        result[name] = value
    return result
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
    #[from_args(default = Value::Bool(false))]
    include_extras: Value,
    #[from_args(kw_only, default = Value::None)]
    format: Value,
}

/// `typing.get_type_hints()`: see [`collect_hints`].
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

    // CPython reads `__no_type_check__` before it looks at anything else.
    if let HintSource::Class(class_id) = hint_source(obj, vm)
        && class_attr_is_true(class_id, "__no_type_check__", vm)?
    {
        return Ok(CallResult::Value(empty_dict(vm)));
    }
    let format = HintFormat::parse(format, true, vm)?;
    let globalns = namespace_dict(globalns, "get_type_hints", "globalns", vm)?;
    let localns = namespace_dict(localns, "get_type_hints", "localns", vm)?;
    let include_extras = include_extras.py_bool(vm)?;
    collect_hints(
        obj,
        &HintsRequest {
            format,
            globalns,
            localns,
            include_extras,
            task: HintsTask::TypeHints,
        },
        vm,
    )
}

/// How an annotation is reported: the `annotationlib.Format` members Monty implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintFormat {
    /// `VALUE`: the evaluated object.
    Value,
    /// `FORWARDREF`: the evaluated object, with each unresolved name a `ForwardRef`.
    ForwardRef,
    /// `STRING`: the source text.
    String,
}

impl HintFormat {
    /// The member `value` names, or CPython's `ValueError`; `None` is `VALUE` only
    /// where `none_is_value` (`get_type_hints`, not `get_annotations`).
    pub(crate) fn parse(value: &Value, none_is_value: bool, vm: &mut VM<'_>) -> RunResult<Self> {
        match format_code(value) {
            _ if none_is_value && matches!(value, Value::None) => Ok(Self::Value),
            Some(1) => Ok(Self::Value),
            Some(2) => Err(ExcType::value_error(
                "The VALUE_WITH_FAKE_GLOBALS format is for internal use only",
            )),
            Some(3) => Ok(Self::ForwardRef),
            Some(4) => Ok(Self::String),
            _ => {
                let repr = value.py_repr(vm)?;
                defer_drop!(repr, vm);
                Err(ExcType::value_error(format!(
                    "Unsupported format {}",
                    repr.as_str(vm).unwrap_or_default()
                )))
            }
        }
    }

    /// The member's integer value, as the frozen evaluator takes it.
    fn code(self) -> i64 {
        match self {
            Self::Value => 1,
            Self::ForwardRef => 3,
            Self::String => 4,
        }
    }
}

/// What the caller wants done with the annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintsTask {
    /// `annotationlib.get_annotations()`: each annotation evaluated where it was
    /// written, forward references left as strings unless `eval_str`.
    Annotations {
        /// Evaluate string values with `eval()` in the given or default namespaces.
        eval_str: bool,
    },
    /// `typing.get_type_hints()`: forward references resolved at any depth and
    /// `None` reported as `NoneType`.
    TypeHints,
}

impl HintsTask {
    /// The task code the frozen evaluator takes.
    fn code(self) -> i64 {
        match self {
            Self::Annotations { eval_str: false } => 0,
            Self::Annotations { eval_str: true } => 1,
            Self::TypeHints => 2,
        }
    }

    /// The Python function's name, for error messages.
    fn name(self) -> &'static str {
        match self {
            Self::Annotations { .. } => "get_annotations",
            Self::TypeHints => "get_type_hints",
        }
    }
}

/// One `get_type_hints()` / `get_annotations()` call's options. The namespaces are
/// borrowed dict ids, validated by [`namespace_dict`].
pub(crate) struct HintsRequest {
    pub format: HintFormat,
    pub globalns: Option<HeapId>,
    pub localns: Option<HeapId>,
    /// `get_type_hints(include_extras=True)`: keep `Annotated` metadata.
    pub include_extras: bool,
    pub task: HintsTask,
}

/// Finds `obj`'s `__annotate__` function or stored `__annotations__` and the
/// namespaces its forward references evaluate in, then hands them to the frozen
/// evaluator, which can push frames and suspend for host name lookups. A host
/// function or class suspends with an OS call instead: only the host can
/// evaluate its hints, in `VALUE` form.
pub(crate) fn collect_hints(obj: &Value, request: &HintsRequest, vm: &mut VM<'_>) -> RunResult<CallResult> {
    match hint_source(obj, vm) {
        HintSource::Function(func_id, globals) => {
            let annotate = annotations::function_annotate(func_id, globals, vm);
            defer_drop!(annotate, vm);
            if matches!(annotate, Value::None) {
                return Ok(CallResult::Value(empty_dict(vm)));
            }
            // A forward reference evaluates in `globalns` (default: the function's
            // globals), then `localns` (default: `globalns`).
            let scopes = ForwardRefScopes {
                globals: request.globalns.or(globals),
                locals: request.localns,
                globals_first: false,
            };
            let source = AnnotationSource {
                annotate: annotate.clone_with_heap(vm),
                stored: Value::None,
                classdict: Value::None,
                cache: false,
            };
            resolve_hints(obj, source, scopes, false, request, vm)
        }
        HintSource::Class(class_id) => {
            let stored = annotations::stored_class_annotations(class_id, vm);
            defer_drop!(stored, vm);
            // CPython's `annotationlib` rejects a non-dict binding such as `None`.
            if let Some(stored) = stored
                && !matches!(stored, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)))
            {
                return Err(not_a_dict_error(class_id, vm));
            }
            let annotate = annotations::class_annotate(class_id, vm);
            defer_drop!(annotate, vm);
            if stored.is_none() && annotate.is_none() {
                return Ok(CallResult::Value(empty_dict(vm)));
            }
            // The annotations evaluate with a copy of the class namespace (CPython's
            // `dict(vars(cls))`) over the module globals.
            let classdict = annotations::class_namespace_copy(class_id, vm)?;
            defer_drop!(classdict, vm);
            let classdict_id = classdict.ref_id();
            let (globals, locals, globals_first) = match request.task {
                // Without explicit namespaces `get_type_hints` searches the module
                // globals before the class namespace, as CPython swaps them.
                HintsTask::TypeHints => match (request.globalns, request.localns) {
                    (None, None) => (None, classdict_id, true),
                    (Some(_), None) => (request.globalns, classdict_id, false),
                    (_, Some(_)) => (request.globalns, request.localns, false),
                },
                HintsTask::Annotations { .. } => (request.globalns, request.localns.or(classdict_id), false),
            };
            let scopes = ForwardRefScopes {
                globals,
                locals,
                globals_first,
            };
            let source = AnnotationSource {
                annotate: annotate.as_ref().map_or(Value::None, |value| value.clone_with_heap(vm)),
                stored: stored.as_ref().map_or(Value::None, |value| value.clone_with_heap(vm)),
                classdict: classdict.clone_with_heap(vm),
                cache: true,
            };
            resolve_hints(obj, source, scopes, true, request, vm)
        }
        HintSource::Host(target) => match request.format {
            HintFormat::String => Err(ExcType::not_implemented(format!(
                "{}() format=Format.STRING is not supported for host functions and classes",
                request.task.name()
            ))
            .into()),
            // The host evaluates its own hints; `FORWARDREF` is `VALUE` there.
            HintFormat::Value | HintFormat::ForwardRef => Ok(CallResult::OsCall(OsFunctionCall::GetTypeHints(target))),
        },
        HintSource::Empty => Ok(CallResult::Value(empty_dict(vm))),
        HintSource::NotAnnotatable => Err(ExcType::no_annotations(obj, vm)),
    }
}

/// The namespaces a forward reference evaluates in: each a borrowed dict id or
/// `None` for the module slots (globals) or no locals.
struct ForwardRefScopes {
    globals: Option<HeapId>,
    locals: Option<HeapId>,
    /// Bound module globals shadow `locals`.
    globals_first: bool,
}

impl ForwardRefScopes {
    /// The `scopes` tuple the frozen evaluator takes, each dict a new reference.
    fn into_tuple(self, vm: &VM<'_>) -> Value {
        let mut items = SmallVec::new();
        for scope in [self.globals, self.locals] {
            items.push(scope.map_or(Value::None, |id| {
                vm.heap.inc_ref(id);
                Value::Ref(id)
            }));
        }
        items.push(Value::Bool(self.globals_first));
        allocate_tuple(items, vm.heap)
    }
}

/// What the frozen evaluator reads an object's annotations from, each an OWNED
/// value or `None`: its `__annotate__` function, its stored `__annotations__`
/// dict, and for a class the namespace copy its annotations evaluate under.
struct AnnotationSource {
    annotate: Value,
    stored: Value,
    classdict: Value,
    /// Cache a plain evaluation on the owner, as `type.__annotations__` does.
    cache: bool,
}

/// Runs the frozen evaluator for `owner`, consuming `source`.
fn resolve_hints(
    owner: &Value,
    source: AnnotationSource,
    scopes: ForwardRefScopes,
    is_class: bool,
    request: &HintsRequest,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let cache = if source.cache {
        Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::CacheClassAnnotations))
    } else {
        Value::None
    };
    let scopes = scopes.into_tuple(vm);
    vm.call_frozen_exact(
        FrozenFunction::GetAnnotations,
        [
            owner.clone_with_heap(vm),
            source.annotate,
            source.stored,
            source.classdict,
            scopes,
            Value::Int(request.format.code()),
            Value::Int(request.task.code()),
            Value::Bool(request.include_extras),
            Value::Bool(is_class),
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::CallAnnotate)),
            cache,
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::EvalHint)),
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::AliasWith)),
            Value::Builtin(Builtins::Type(BuiltinType::ForwardRef)),
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::TypeRepr)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Len)),
            Value::Builtin(Builtins::Type(BuiltinType::Str)),
            Value::Builtin(Builtins::Type(BuiltinType::Tuple)),
            Value::Builtin(Builtins::Type(BuiltinType::Dict)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Repr)),
            Value::Builtin(Builtins::Type(BuiltinType::GenericAlias)),
            Value::Marker(Marker(StaticStrings::Literal)),
            Value::Builtin(Builtins::Type(BuiltinType::Union)),
            Value::Builtin(Builtins::Type(BuiltinType::NoneType)),
            Value::Builtin(Builtins::ExcType(ExcType::Exception)),
            Value::Builtin(Builtins::ExcType(ExcType::NotImplementedError)),
            Value::Builtin(Builtins::ExcType(ExcType::SyntaxError)),
            Value::Builtin(Builtins::ExcType(ExcType::ValueError)),
        ],
    )
}

/// Frozen helper: CPython's `annotationlib.type_repr`, a type by its name and
/// anything else by its repr, for `STRING` format.
pub(crate) fn call_type_repr(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let value = args.get_one_arg("type_repr", vm.heap)?;
    defer_drop!(value, vm);
    let mut text = String::new();
    repr_type_arg(value, &mut text, vm, &mut LazyHeapSet::default())?;
    Ok(allocate_string(text, vm.heap))
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

/// The globals `ForwardRef.evaluate()` defaults to for `owner`: a function's own
/// globals (`None` for the module slots), the module slots for a class, and an
/// empty dict for anything else, as CPython evaluates against `{}`.
pub(crate) fn owner_globals(owner: &Value, vm: &VM<'_>) -> Value {
    match hint_source(owner, vm) {
        HintSource::Function(_, Some(dict)) => {
            vm.heap.inc_ref(dict);
            Value::Ref(dict)
        }
        HintSource::Function(_, None) | HintSource::Class(_) => Value::None,
        _ => empty_dict(vm),
    }
}

/// The locals `ForwardRef.evaluate()` defaults to for `owner`: a copy of a class's
/// namespace, nothing otherwise.
pub(crate) fn owner_locals(owner: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    match hint_source(owner, vm) {
        HintSource::Class(class_id) => annotations::class_namespace_copy(class_id, vm),
        _ => Ok(Value::None),
    }
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
            let repr = repr.as_str(vm).map(str::to_owned).unwrap_or_default();
            ExcType::value_error(format!("{repr}.__annotations__ is neither a dict nor None"))
        }
        Err(err) => err,
    }
}

/// Validates a namespace argument of `function`: `None`, or a dict whose id is returned (borrowed).
pub(crate) fn namespace_dict(value: &Value, function: &str, param: &str, vm: &VM<'_>) -> RunResult<Option<HeapId>> {
    match value {
        Value::None => Ok(None),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Ok(Some(*id)),
        _ => Err(ExcType::type_error(format!(
            "{function}() {param} must be a dict, not {}",
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
    /// 0 raises `NameError`, 1 stringifies unresolved names, 2 stringifies every name.
    #[from_args(pos_only)]
    missing: Value,
    #[from_args(pos_only)]
    owner: Value,
    #[from_args(pos_only)]
    is_class: Value,
}

/// Frozen helper: evaluates one annotation string; the namespaces are dicts or
/// `None`, built by [`collect_hints`] and never seen by user code.
pub(crate) fn call_eval_hint(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let EvalHintArgs {
        source,
        globals,
        locals,
        globals_first,
        forward_ref,
        missing,
        owner,
        is_class,
    } = EvalHintArgs::from_args(args, vm)?;
    defer_drop!(source, vm);
    defer_drop!(globals, vm);
    defer_drop!(locals, vm);
    defer_drop!(owner, vm);
    let globals_first = matches!(globals_first, Value::Bool(true));
    let forward_ref = matches!(forward_ref, Value::Bool(true));
    let missing = match missing {
        Value::Int(0) => MissingNames::Raise,
        Value::Int(every_name) => MissingNames::ForwardRef {
            owner: owner.clone_with_heap(vm),
            is_class: matches!(is_class, Value::Bool(true)),
            every_name: every_name == 2,
        },
        _ => unreachable!("frozen code passes the name mode as an int"),
    };
    eval_exec::eval_hint(
        source,
        globals.ref_id(),
        locals.ref_id(),
        globals_first,
        forward_ref,
        missing,
        vm,
    )
}
