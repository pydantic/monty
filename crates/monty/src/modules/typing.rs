//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//! - `Union`, the type behind `int | None` (see `types/union.rs`)
//! - `ForwardRef`, shared with `annotationlib` (see `types/forward_ref.rs`)
//! - `get_type_hints()`, evaluating the stringized annotations Monty stores in
//!   any `annotationlib.Format`; `annotationlib.get_annotations()` shares its
//!   machinery through [`collect_hints`]
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking. `Optional[X]` is the one
//! marker that can be subscripted, producing `X | None`.

use monty_types::{OsFunctionCall, TypeHintsTarget};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions, eval_exec},
    bytecode::{CallResult, MissingNames, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, HeapData, HeapId},
    intern::{FunctionId, StaticStrings},
    modules::ModuleFunctions,
    stringize::string_literal_text,
    types::{
        BuiltinType, ClassAnnotations, Dict, List, Module, PyTrait, allocate_string, allocate_tuple,
        forward_ref::format_code,
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

/// Evaluates stringized annotations as CPython 3.14's `annotationlib.get_annotations`
/// and `typing.get_type_hints` do.
///
/// `hints` holds `[name, text]` pairs, rewritten in place; `scopes` is
/// `(globals, locals, fwd_globals, fwd_locals, fwd_globals_first)`: where a stored
/// annotation evaluates, then where a forward reference does. `format_` is the
/// `annotationlib.Format` code, `task` 0 for `get_annotations`, 1 for it with
/// `eval_str=True` and 2 for `get_type_hints`, and `future` marks PEP 563 strings.
/// `eval_hint`'s last three arguments are the name mode (0 raise, 1 stringify
/// unresolved names, 2 stringify every name), the owner and the class flag.
pub(crate) const GET_TYPE_HINTS_SOURCE: &str = r"
def __monty_typing_get_type_hints(hints, scopes, format_, task, future, owner, is_class, eval_hint, forward_ref, isinstance_, len_, str_, tuple_, generic_alias, union, none_type, exception, syntax_error):
    FORWARDREF = 3
    STRING = 4
    globalns, localns, fwd_globalns, fwd_localns, fwd_globals_first = scopes
    # Pass 1, CPython's `__annotate__(VALUE)`: every annotation evaluates where it
    # was written before any forward reference resolves. PEP 563 strings and the
    # STRING format are reported as stored.
    if not future and format_ != STRING:
        for entry in hints:
            text = entry[1]
            if not isinstance_(text, str_):
                continue
            if format_ != FORWARDREF:
                entry[1] = eval_hint(text, globalns, localns, False, False, 0, None, False)
                continue
            try:
                entry[1] = eval_hint(text, globalns, localns, False, False, 1, owner, is_class)
            except exception:
                # annotationlib's fallback for an annotation that raises: stringify
                # every name, then evaluate the reference once more, keeping it on failure.
                value = eval_hint(text, globalns, localns, False, False, 2, owner, is_class)
                if isinstance_(value, forward_ref):
                    try:
                        evaluated = eval_hint(text, globalns, localns, False, True, 1, owner, is_class)
                    except exception:
                        evaluated = value
                    if not (isinstance_(evaluated, forward_ref) and text.isidentifier()):
                        value = evaluated
                entry[1] = value
    # `get_annotations(eval_str=True)` evaluates string values with plain `eval()`.
    if task == 1:
        for entry in hints:
            if isinstance_(entry[1], str_):
                entry[1] = eval_hint(entry[1], fwd_globalns, fwd_localns, False, False, 0, None, False)
    if task != 2 or format_ == STRING:
        result = {}
        for name, value in hints:
            result[name] = value
        return result
    # Pass 2, `typing._eval_type`: resolve forward references at any depth of a
    # generic alias or union. An explicit stack, since frozen code cannot recurse;
    # each frame is [value, resolved args or None, texts seen, next index, parent reference].
    result = {}
    for name, value in hints:
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
                if isinstance_(value, generic_alias) or isinstance_(value, union):
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
            if resolved != args:
                if isinstance_(value, generic_alias):
                    value = value.__origin__[resolved]
                else:
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
    /// Accepted and ignored: Monty has no `Annotated` metadata to strip.
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
    collect_hints(
        obj,
        &HintsRequest {
            format,
            globalns,
            localns,
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
    pub task: HintsTask,
}

/// Collects `obj`'s stringized annotations and the namespaces they evaluate in,
/// then hands them to the frozen evaluator, which can push frames and suspend
/// for host name lookups. A host function or class suspends with an OS call
/// instead: only the host can evaluate its hints, in `VALUE` form.
pub(crate) fn collect_hints(obj: &Value, request: &HintsRequest, vm: &mut VM<'_>) -> RunResult<CallResult> {
    match hint_source(obj, vm) {
        HintSource::Function(func_id, globals) => {
            let function = vm.interns.get_function(func_id);
            let future = function.future_annotations;
            let pairs: Vec<(Value, Value)> = function
                .annotations
                .iter()
                .map(|&(name, hint)| (Value::InternString(name), Value::InternString(hint)))
                .collect();
            // An annotation evaluates in the function's globals, as its `__annotate__`
            // would; a forward reference in `globalns` (default: those globals), then
            // `localns` (default: `globalns`).
            let scopes = Scopes {
                globals,
                locals: None,
                fwd_globals: request.globalns.or(globals),
                fwd_locals: request.localns,
                fwd_globals_first: false,
            };
            evaluate_hints(&pairs, scopes, future, obj, false, request, vm)
        }
        HintSource::Class(class_id) => {
            let HeapData::Class(class) = vm.heap.get(class_id) else {
                unreachable!("hint_source classified a class");
            };
            let future = class.future_annotations();
            let annotations = match class.annotations(vm.heap, vm.interns) {
                ClassAnnotations::Missing => return Ok(CallResult::Value(empty_dict(vm))),
                ClassAnnotations::Dict(annotations) => annotations,
                // CPython's `annotationlib` rejects `None` here too.
                ClassAnnotations::NotADict => return Err(not_a_dict_error(class_id, vm)),
            };
            let pairs: Vec<(Value, Value)> = annotations
                .iter()
                .map(|(name, hint)| (name.clone_with_heap(vm.heap), hint.clone_with_heap(vm.heap)))
                .collect();
            defer_drop!(pairs, vm);
            // An annotation evaluates with a copy of the class namespace (CPython's
            // `dict(vars(cls))`) over the module globals, as its `__annotate__` would.
            let namespace = class_namespace_copy(class_id, vm)?;
            defer_drop!(namespace, vm);
            let namespace_id = namespace.ref_id();
            let (fwd_globals, fwd_locals, fwd_globals_first) = match request.task {
                // Without explicit namespaces `get_type_hints` searches the module
                // globals before the class namespace, as CPython swaps them.
                HintsTask::TypeHints => match (request.globalns, request.localns) {
                    (None, None) => (None, namespace_id, true),
                    (Some(_), None) => (request.globalns, namespace_id, false),
                    (_, Some(_)) => (request.globalns, request.localns, false),
                },
                HintsTask::Annotations { .. } => (request.globalns, request.localns.or(namespace_id), false),
            };
            let scopes = Scopes {
                globals: None,
                locals: namespace_id,
                fwd_globals,
                fwd_locals,
                fwd_globals_first,
            };
            evaluate_hints(pairs, scopes, future, obj, true, request, vm)
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

/// The namespaces an evaluation uses: `globals` / `locals` for a stored annotation
/// and `fwd_*` for a forward reference, each a borrowed dict id or `None` for the
/// module slots (globals) or no locals.
struct Scopes {
    globals: Option<HeapId>,
    locals: Option<HeapId>,
    fwd_globals: Option<HeapId>,
    fwd_locals: Option<HeapId>,
    /// Bound module globals shadow `fwd_locals`.
    fwd_globals_first: bool,
}

impl Scopes {
    /// The `scopes` tuple the frozen evaluator takes, each dict a new reference.
    fn into_tuple(self, vm: &VM<'_>) -> Value {
        let mut items = SmallVec::new();
        for scope in [self.globals, self.locals, self.fwd_globals, self.fwd_locals] {
            items.push(scope.map_or(Value::None, |id| {
                vm.heap.inc_ref(id);
                Value::Ref(id)
            }));
        }
        items.push(Value::Bool(self.fwd_globals_first));
        allocate_tuple(items, vm.heap)
    }
}

/// Runs the frozen evaluator over borrowed `(name, text)` pairs; with none, `{}`
/// without a call. `future` marks PEP 563 strings, `owner` is the annotated
/// object and `is_class` whether the annotations come from a class body.
fn evaluate_hints(
    pairs: &[(Value, Value)],
    scopes: Scopes,
    future: bool,
    owner: &Value,
    is_class: bool,
    request: &HintsRequest,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    if pairs.is_empty() {
        return Ok(CallResult::Value(empty_dict(vm)));
    }
    let mut entries_guard = DropGuard::new(Vec::with_capacity(pairs.len()), vm);
    let (entries, vm) = entries_guard.as_parts_mut();
    for (name, hint) in pairs {
        // `STRING` reports a plain string literal unquoted, as annotationlib's
        // stringifier does; a PEP 563 string is already in that form.
        let hint = match hint.as_str(vm) {
            Some(text) if request.format == HintFormat::String && !future => match string_literal_text(text) {
                Some(literal) => allocate_string(literal, vm.heap),
                None => hint.clone_with_heap(vm),
            },
            _ => hint.clone_with_heap(vm),
        };
        let entry = List::new(vec![name.clone_with_heap(vm), hint]);
        entries.push(Value::Ref(vm.heap.allocate(HeapData::List(entry))));
    }
    let (entries, vm) = entries_guard.into_parts();
    let hints = Value::Ref(vm.heap.allocate(HeapData::List(List::new(entries))));
    let scopes = scopes.into_tuple(vm);
    vm.call_frozen_exact(
        FrozenFunction::TypingGetTypeHints,
        [
            hints,
            scopes,
            Value::Int(request.format.code()),
            Value::Int(request.task.code()),
            Value::Bool(future),
            owner.clone_with_heap(vm),
            Value::Bool(is_class),
            Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::EvalHint)),
            Value::Builtin(Builtins::Type(BuiltinType::ForwardRef)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Len)),
            Value::Builtin(Builtins::Type(BuiltinType::Str)),
            Value::Builtin(Builtins::Type(BuiltinType::Tuple)),
            Value::Builtin(Builtins::Type(BuiltinType::GenericAlias)),
            Value::Builtin(Builtins::Type(BuiltinType::Union)),
            Value::Builtin(Builtins::Type(BuiltinType::NoneType)),
            Value::Builtin(Builtins::ExcType(ExcType::Exception)),
            Value::Builtin(Builtins::ExcType(ExcType::SyntaxError)),
        ],
    )
}

/// A fresh empty dict.
fn empty_dict(vm: &VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::Dict(Dict::new())))
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
        HintSource::Class(class_id) => class_namespace_copy(class_id, vm),
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
