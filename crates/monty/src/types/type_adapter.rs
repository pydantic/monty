//! `pydantic.TypeAdapter`: a validator arena built once from a type hint,
//! with the methods that run it.
//!
//! The arena is shared through an `Rc` so a validation can run Python (a
//! `default_factory`, `__post_init__`) while reading it without borrowing
//! the heap entry. Each adapter owns its arena outright: the only other
//! handles are those of validations in flight, whose method call holds a
//! reference to the adapter, so the arena is unshared whenever it is freed.

use std::{cell::RefCell, fmt::Write, mem, rc::Rc};

use jiter::{Jiter, JiterError, JiterErrorType, JsonErrorType};
use monty_types::ValidationInputType;
use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    modules::pydantic::{
        arena::{Arena, ValidatorId},
        build,
        catalogue::ErrorType,
        errors::{ValError, ValResult, bool_arg, into_run_error, json_indent, json_invalid_error},
        input::State,
        json_input::JsonInput,
        python_input::PyInput,
        serialize::{self, Mode, Options},
        validate::validate,
    },
    types::{
        LazyHeapSet, PyTrait, Type, TypeRef, allocate_string,
        decimal::py_context::{current_context, settings_of},
    },
    value::{EitherStr, Value},
};

/// A built `TypeAdapter`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TypeAdapter {
    arena: Rc<Arena>,
    root: ValidatorId,
    /// The root validator's name: the `ValidationError` title.
    title: String,
    /// The hint the adapter was built from, an OWNED reference; `repr()`
    /// displays it on demand, as pydantic's `__repr__` does.
    hint: Value,
}

/// Arguments of `TypeAdapter.__init__(self, type, *, config=None, _parent_depth=2, module=None)`.
///
/// `this` is a placeholder the constructor prepends, so arity errors count
/// `self` as pydantic's do (`takes 2 positional arguments but 3 were given`).
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.__init__", style = def)]
struct TypeAdapterArgs {
    #[from_args(pos_only)]
    this: Value,
    #[from_args(static_string = "TypeKey")]
    r#type: Value,
    #[from_args(kw_only, default = Value::None)]
    config: Value,
    #[from_args(kw_only, default = Value::Int(2))]
    parent_depth: Value,
    #[from_args(kw_only, default = Value::None)]
    module: Value,
}

impl TypeAdapter {
    /// `TypeAdapter(type)`: builds the arena, raising
    /// `PydanticSchemaGenerationError` for a hint with no validator.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let TypeAdapterArgs {
            this,
            r#type: hint,
            config,
            parent_depth,
            module,
        } = TypeAdapterArgs::from_args(args.prepend(Value::None), vm)?;
        debug_assert!(matches!(this, Value::None), "the placeholder `self` is `None`");
        defer_drop!(config, vm);
        defer_drop!(parent_depth, vm);
        defer_drop!(module, vm);
        // Kept in a guard rather than `defer_drop!` since the adapter takes it on success.
        let mut hint = DropGuard::new(hint, vm);
        if !matches!(config, Value::None) {
            return Err(unsupported_argument("TypeAdapter.__init__", "config"));
        }
        if !matches!(module, Value::None) {
            return Err(unsupported_argument("TypeAdapter.__init__", "module"));
        }
        let (hint_value, vm) = hint.as_parts_mut();
        let mut arena = DropGuard::new(Arena::default(), vm);
        let (arena_mut, vm) = arena.as_parts_mut();
        let root = build::build(hint_value, arena_mut, vm)?;
        let (arena, _) = arena.into_parts();
        let title = arena.name(root);
        let (hint, vm) = hint.into_parts();
        let adapter = Self {
            arena: Rc::new(arena),
            root,
            title,
            hint,
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::TypeAdapter(Box::new(adapter)))))
    }

    /// Invokes `on_child` for every heap id the adapter owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = self.hint {
            on_child(id);
        }
        self.arena.for_each_child_id(on_child);
    }
}

impl HeapItem for TypeAdapter {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.hint.py_dec_ref_ids(stack);
        unshared(&mut self.arena).py_dec_ref_ids(stack);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for TypeAdapter {
    fn drop_with(mut self, ctx: &mut C) {
        self.hint.drop_with(ctx);
        mem::take(unshared(&mut self.arena)).drop_with(ctx);
    }
}

/// The arena of an adapter being freed. A validation clones the `Rc` only for
/// the length of a method call, which holds a reference to the adapter, and a
/// copy rebuilds the arena, so nothing else can share it by now.
fn unshared(arena: &mut Rc<Arena>) -> &mut Arena {
    Rc::get_mut(arena).expect("a freed adapter's arena is unshared")
}

/// The class attributes instances also see: `__module__`, which a Python
/// class keeps in its namespace.
pub(crate) fn class_attr(attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
    match attr.static_string(vm.interns)? {
        StaticStrings::DunderModule => Type::TypeAdapter
            .module()
            .map(|module| allocate_string(module, vm.heap)),
        _ => None,
    }
}

/// Arguments of `validate_python(object, /, *, strict=None, ...)`; the
/// options pydantic has and Monty does not are refused when set.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.validate_python", style = def)]
struct ValidatePythonArgs {
    #[from_args(pos_only)]
    object: Value,
    #[from_args(kw_only, default = Value::None)]
    strict: Value,
    #[from_args(kw_only, default = Value::None)]
    extra: Value,
    #[from_args(kw_only, default = Value::None)]
    from_attributes: Value,
    #[from_args(kw_only, default = Value::None)]
    context: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    experimental_allow_partial: Value,
    #[from_args(kw_only, default = Value::None)]
    by_alias: Value,
    #[from_args(kw_only, default = Value::None)]
    by_name: Value,
}

impl<'h> HeapObjectRead<'h, TypeAdapter> {
    /// A shared handle on the arena, its root and the error title, so a
    /// validation can run Python without borrowing the heap entry. The method
    /// call holding `self` keeps the adapter, and so the values its arena
    /// owns, alive while the handle exists.
    fn arena_handle(&self, vm: &VM<'h>) -> (Rc<Arena>, ValidatorId, String) {
        let this = self.get(vm.heap);
        (Rc::clone(&this.arena), this.root, this.title.clone())
    }

    /// A new adapter over a copy of this one's validators, for `copy.copy`
    /// and `copy.deepcopy`.
    pub(crate) fn allocate_like(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let this = self.get(vm.heap);
        let adapter = TypeAdapter {
            arena: Rc::new(this.arena.clone_with_heap(vm)?),
            root: this.root,
            title: this.title.clone(),
            hint: this.hint.clone_with_heap(vm.heap),
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::TypeAdapter(Box::new(adapter)))))
    }

    /// `validate_python(object, /, *, strict=None)`.
    fn validate_python(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let ValidatePythonArgs {
            object,
            strict,
            extra,
            from_attributes,
            context,
            experimental_allow_partial,
            by_alias,
            by_name,
        } = ValidatePythonArgs::from_args(args, vm)?;
        // The input owns `object` from here; every argument error below releases it.
        let input = PyInput::new(object);
        defer_drop_mut!(input, vm);
        defer_drop!(strict, vm);
        defer_drop!(extra, vm);
        defer_drop!(from_attributes, vm);
        defer_drop!(context, vm);
        defer_drop!(experimental_allow_partial, vm);
        defer_drop!(by_alias, vm);
        defer_drop!(by_name, vm);
        let unsupported = [
            ("extra", extra),
            ("from_attributes", from_attributes),
            ("context", context),
            ("by_alias", by_alias),
            ("by_name", by_name),
        ];
        for (name, value) in unsupported {
            if !matches!(value, Value::None) {
                return Err(unsupported_argument("TypeAdapter.validate_python", name));
            }
        }
        allow_partial("TypeAdapter.validate_python", experimental_allow_partial, vm)?;
        let strict = strict_flag(strict, vm)?;
        let (arena, root, title) = self.arena_handle(vm);
        let mut state = State::new(strict);
        validate(&arena, root, input, &mut state, vm)
            .map_err(|err| into_run_error(err, &title, ValidationInputType::Python, vm))
    }
}

/// Arguments of `validate_json(data, /, *, strict=None, ...)`.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.validate_json", style = def)]
struct ValidateJsonArgs {
    #[from_args(pos_only)]
    data: Value,
    #[from_args(kw_only, default = Value::None)]
    strict: Value,
    #[from_args(kw_only, default = Value::None)]
    extra: Value,
    #[from_args(kw_only, default = Value::None)]
    context: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    experimental_allow_partial: Value,
    #[from_args(kw_only, default = Value::None)]
    by_alias: Value,
    #[from_args(kw_only, default = Value::None)]
    by_name: Value,
}

impl<'h> HeapObjectRead<'h, TypeAdapter> {
    /// `validate_json(data, /, *, strict=None)`: `data` is `str` or `bytes`,
    /// streamed through the validators; any other type is a `json_type` error
    /// and a document that is not JSON a `json_invalid` one.
    fn validate_json(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let ValidateJsonArgs {
            data,
            strict,
            extra,
            context,
            experimental_allow_partial,
            by_alias,
            by_name,
        } = ValidateJsonArgs::from_args(args, vm)?;
        defer_drop!(data, vm);
        defer_drop!(strict, vm);
        defer_drop!(extra, vm);
        defer_drop!(context, vm);
        defer_drop!(experimental_allow_partial, vm);
        defer_drop!(by_alias, vm);
        defer_drop!(by_name, vm);
        for (name, value) in [
            ("extra", extra),
            ("context", context),
            ("by_alias", by_alias),
            ("by_name", by_name),
        ] {
            if !matches!(value, Value::None) {
                return Err(unsupported_argument("TypeAdapter.validate_json", name));
            }
        }
        allow_partial("TypeAdapter.validate_json", experimental_allow_partial, vm)?;
        let strict = strict_flag(strict, vm)?;
        let (arena, root, title) = self.arena_handle(vm);
        // The document is copied out so the parser does not borrow the heap
        // while validators allocate on it.
        let Some(document) = json_document(data, vm) else {
            let line = ValError::owned(ErrorType::JsonType {}, data.clone_with_heap(vm.heap));
            return Err(into_run_error(line, &title, ValidationInputType::Json, vm));
        };
        let parser = RefCell::new(Jiter::new(&document).with_allow_inf_nan());
        let mut state = State::new(strict);
        let result = JsonInput::root(&parser).map_err(ValError::Json).and_then(|mut input| {
            let result = validate(&arena, root, &mut input, &mut state, vm);
            finish_document(result, &mut input, vm)
        });
        result.map_err(|err| match err {
            ValError::Json(err) => {
                let description = json_error_description(err, &parser.borrow());
                json_invalid_error(&title, description, data.clone_with_heap(vm.heap), vm)
            }
            other => into_run_error(other, &title, ValidationInputType::Json, vm),
        })
    }
}

/// The message of a JSON syntax error, positioned as pydantic-core reports it.
///
/// jiter 0.19 points an invalid UTF-8 error at the first bad byte, while the
/// jiter pydantic-core ships points one byte past it.
fn json_error_description(mut err: JiterError, parser: &Jiter) -> String {
    if matches!(
        err.error_type,
        JiterErrorType::JsonError(JsonErrorType::InvalidUnicodeCodePoint)
    ) {
        err.index += 1;
    }
    err.description(parser)
}

/// The bytes of a `str` or `bytes` document, or `None` for any other value.
fn json_document(data: &Value, vm: &VM<'_>) -> Option<Vec<u8>> {
    if let Some(text) = data.as_str(vm) {
        return Some(text.as_bytes().to_vec());
    }
    match data {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id).to_vec()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Some(bytes.as_slice().to_vec()),
            _ => None,
        },
        _ => None,
    }
}

/// Checks the document ends where the root value does, once validation has
/// finished with it: a document that is not JSON is `json_invalid` whatever
/// the validators made of the part they read, as pydantic parses it whole.
fn finish_document(result: ValResult<Value>, input: &mut JsonInput<'_, '_>, vm: &mut VM<'_>) -> ValResult<Value> {
    match result {
        Err(err @ (ValError::Run(_) | ValError::Json(_))) => Err(err),
        result => match input.finish() {
            Ok(()) => result,
            Err(err) => {
                match result {
                    Ok(value) => value.drop_with(vm),
                    Err(failure) => failure.drop_with(vm),
                }
                Err(ValError::Json(err))
            }
        },
    }
}

/// Arguments of `dump_python(instance, /, *, mode=, exclude_none=, ...)`.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.dump_python", style = def)]
struct DumpPythonArgs {
    #[from_args(pos_only)]
    instance: Value,
    #[from_args(kw_only, default = Value::None)]
    mode: Value,
    #[from_args(kw_only, default = Value::None)]
    include: Value,
    #[from_args(kw_only, default = Value::None)]
    exclude: Value,
    #[from_args(kw_only, default = Value::None)]
    by_alias: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_unset: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_defaults: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_none: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_computed_fields: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    round_trip: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    warnings: Value,
    #[from_args(kw_only, default = Value::None)]
    fallback: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    serialize_as_any: Value,
    #[from_args(kw_only, default = Value::None)]
    polymorphic_serialization: Value,
    #[from_args(kw_only, default = Value::None)]
    context: Value,
}

/// Arguments of `dump_json(instance, /, *, indent=None, ensure_ascii=False, ...)`.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.dump_json", style = def)]
struct DumpJsonArgs {
    #[from_args(pos_only)]
    instance: Value,
    #[from_args(kw_only, default = Value::None)]
    indent: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    ensure_ascii: Value,
    #[from_args(kw_only, default = Value::None)]
    include: Value,
    #[from_args(kw_only, default = Value::None)]
    exclude: Value,
    #[from_args(kw_only, default = Value::None)]
    by_alias: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_unset: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_defaults: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_none: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    exclude_computed_fields: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    round_trip: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    warnings: Value,
    #[from_args(kw_only, default = Value::None)]
    fallback: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    serialize_as_any: Value,
    #[from_args(kw_only, default = Value::None)]
    polymorphic_serialization: Value,
    #[from_args(kw_only, default = Value::None)]
    context: Value,
}

/// The dump options shared by both methods, checked in pydantic's order.
struct DumpCommon {
    include: Value,
    exclude: Value,
    by_alias: Value,
    exclude_unset: Value,
    exclude_defaults: Value,
    exclude_none: Value,
    exclude_computed_fields: Value,
    round_trip: Value,
    warnings: Value,
    fallback: Value,
    serialize_as_any: Value,
    polymorphic_serialization: Value,
    context: Value,
}

impl DumpCommon {
    /// Validates the options, yielding the serializer's `exclude_none`; the
    /// bool options must be `bool`, and the rest are refused when set.
    fn check(self, function: &str, mode: Mode, vm: &mut VM<'_>) -> RunResult<Options> {
        let Self {
            include,
            exclude,
            by_alias,
            exclude_unset,
            exclude_defaults,
            exclude_none,
            exclude_computed_fields,
            round_trip,
            warnings,
            fallback,
            serialize_as_any,
            polymorphic_serialization,
            context,
        } = self;
        defer_drop!(include, vm);
        defer_drop!(exclude, vm);
        defer_drop!(by_alias, vm);
        defer_drop!(exclude_unset, vm);
        defer_drop!(exclude_defaults, vm);
        defer_drop!(exclude_none, vm);
        defer_drop!(exclude_computed_fields, vm);
        defer_drop!(round_trip, vm);
        defer_drop!(warnings, vm);
        defer_drop!(fallback, vm);
        defer_drop!(serialize_as_any, vm);
        defer_drop!(polymorphic_serialization, vm);
        defer_drop!(context, vm);
        for (name, value) in [
            ("include", include),
            ("exclude", exclude),
            ("fallback", fallback),
            ("polymorphic_serialization", polymorphic_serialization),
            ("context", context),
        ] {
            if !matches!(value, Value::None) {
                return Err(unsupported_argument(function, name));
            }
        }
        if !matches!(by_alias, Value::None) {
            bool_arg("by_alias", by_alias, vm)?;
        }
        bool_arg("round_trip", round_trip, vm)?;
        check_warnings(warnings, vm)?;
        let exclude_none = bool_arg("exclude_none", exclude_none, vm)?;
        for (name, value) in [
            ("exclude_unset", exclude_unset),
            ("exclude_defaults", exclude_defaults),
            ("exclude_computed_fields", exclude_computed_fields),
            ("serialize_as_any", serialize_as_any),
        ] {
            if bool_arg(name, value, vm)? {
                return Err(unsupported_argument(function, name));
            }
        }
        let context = current_context(vm);
        let capitals = settings_of(vm, context).capitals;
        Ok(Options {
            mode,
            exclude_none,
            capitals,
        })
    }
}

impl<'h> HeapObjectRead<'h, TypeAdapter> {
    /// `dump_python(instance, /, *, mode='python', exclude_none=False, ...)`.
    fn dump_python(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let DumpPythonArgs {
            instance,
            mode,
            include,
            exclude,
            by_alias,
            exclude_unset,
            exclude_defaults,
            exclude_none,
            exclude_computed_fields,
            round_trip,
            warnings,
            fallback,
            serialize_as_any,
            polymorphic_serialization,
            context,
        } = DumpPythonArgs::from_args(args, vm)?;
        defer_drop!(instance, vm);
        defer_drop!(mode, vm);
        let common = DumpCommon {
            include,
            exclude,
            by_alias,
            exclude_unset,
            exclude_defaults,
            exclude_none,
            exclude_computed_fields,
            round_trip,
            warnings,
            fallback,
            serialize_as_any,
            polymorphic_serialization,
            context,
        };
        let mode = dump_mode(mode, vm)?;
        let options = common.check("TypeAdapter.dump_python", mode, vm)?;
        let (arena, root, _) = self.arena_handle(vm);
        serialize::dump_python(&arena, root, instance, options, vm)
    }

    /// `dump_json(instance, /, *, indent=None, ensure_ascii=False, exclude_none=False, ...)`.
    fn dump_json(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let DumpJsonArgs {
            instance,
            indent,
            ensure_ascii,
            include,
            exclude,
            by_alias,
            exclude_unset,
            exclude_defaults,
            exclude_none,
            exclude_computed_fields,
            round_trip,
            warnings,
            fallback,
            serialize_as_any,
            polymorphic_serialization,
            context,
        } = DumpJsonArgs::from_args(args, vm)?;
        defer_drop!(instance, vm);
        defer_drop!(indent, vm);
        defer_drop!(ensure_ascii, vm);
        let common = DumpCommon {
            include,
            exclude,
            by_alias,
            exclude_unset,
            exclude_defaults,
            exclude_none,
            exclude_computed_fields,
            round_trip,
            warnings,
            fallback,
            serialize_as_any,
            polymorphic_serialization,
            context,
        };
        let indent = json_indent(indent, vm)?;
        let ensure_ascii = bool_arg("ensure_ascii", ensure_ascii, vm)?;
        let options = common.check("TypeAdapter.dump_json", Mode::Json, vm)?;
        let (arena, root, _) = self.arena_handle(vm);
        serialize::dump_json(&arena, root, instance, options, indent, ensure_ascii, vm)
    }
}

/// `mode='json'` selects JSON mode; pydantic treats any other `str` as `'python'`.
fn dump_mode(mode: &Value, vm: &VM<'_>) -> RunResult<Mode> {
    match (mode, mode.as_str(vm)) {
        (Value::None, _) => Ok(Mode::Python),
        (_, Some("json")) => Ok(Mode::Json),
        (_, Some(_)) => Ok(Mode::Python),
        (other, None) => Err(ExcType::type_error_arg_not_instance(
            "mode",
            &other.py_type_name(vm),
            "str",
        )),
    }
}

/// How PyO3's failed downcast names a value: `'None'` for `None`, `'int' object` otherwise.
fn not_bool_subject(value: &Value, vm: &VM<'_>) -> String {
    match value {
        Value::None => "'None'".to_owned(),
        other => format!("'{}' object", other.py_type_name(vm)),
    }
}

/// `warnings=`: a `bool` or `'none'`, `'warn'` or `'error'`, with the
/// `TypeError` PyO3 builds from both failed variants. Monty emits no
/// serialization warnings, so a valid value changes nothing.
fn check_warnings(value: &Value, vm: &VM<'_>) -> RunResult<()> {
    match (value, value.as_str(vm)) {
        (Value::Bool(_), _) | (_, Some("none" | "warn" | "error")) => Ok(()),
        (_, text) => {
            let cause = if text.is_some() { "ValueError" } else { "TypeError" };
            Err(ExcType::type_error(format!(
                "argument 'warnings': failed to extract enum WarningsArg ('Bool | Literal')\n\
                 - variant Bool (Bool): TypeError: failed to extract field WarningsArg::Bool.0, \
                 caused by TypeError: {} is not an instance of 'bool'\n\
                 - variant Literal (Literal): TypeError: failed to extract field WarningsArg::Literal.0, \
                 caused by {cause}: Invalid warnings parameter, should be `'none'`, `'warn'`, `'error'` or a `bool`",
                not_bool_subject(value, vm)
            )))
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, TypeAdapter> {
    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::TypeAdapter.into()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Adapters hash by identity, as pydantic's do.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// `TypeAdapter(list[int])`, as pydantic's `__repr__` shows the type.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let hint = self.get(vm.heap).hint.clone_with_heap(vm.heap);
        defer_drop!(hint, vm);
        f.write_str("TypeAdapter(")?;
        build::display_as_type(hint, f, vm, heap_ids)?;
        Ok(f.write_char(')')?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok(class_attr(attr, vm).map(CallResult::Value))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::ValidatePython) => self.validate_python(args, vm).map(CallResult::Value),
            Some(StaticStrings::ValidateJson) => self.validate_json(args, vm).map(CallResult::Value),
            Some(StaticStrings::DumpPython) => self.dump_python(args, vm).map(CallResult::Value),
            Some(StaticStrings::DumpJson) => self.dump_json(args, vm).map(CallResult::Value),
            Some(StaticStrings::ValidateStrings | StaticStrings::JsonSchema) => {
                let name = attr.as_str(vm.interns).to_owned();
                args.drop_with(vm);
                Err(SimpleException::new_msg(
                    ExcType::NotImplementedError,
                    format!("TypeAdapter.{name}() is not supported"),
                )
                .into())
            }
            _ => Err(ExcType::attribute_error_method("TypeAdapter", attr, args, vm)),
        }
    }
}

/// `strict=None` or a `bool`, with pydantic-core's `TypeError` for anything else.
fn strict_flag(value: &Value, vm: &VM<'_>) -> RunResult<bool> {
    match value {
        Value::None => Ok(false),
        other => bool_arg("strict", other, vm),
    }
}

/// `experimental_allow_partial=`: `False` and `'off'` validate whole inputs;
/// `True`, `'on'` and `'trailing-strings'` are refused as unsupported, and
/// anything else raises pydantic-core's error (a `ValueError` for a `str`).
fn allow_partial(function: &str, value: &Value, vm: &VM<'_>) -> RunResult<()> {
    const MESSAGE: &str = "Invalid partial mode, should be `'off'`, `'on'`, `'trailing-strings'` or a `bool`";
    match (value, value.as_str(vm)) {
        (Value::Bool(false), _) | (_, Some("off")) => Ok(()),
        (Value::Bool(true), _) | (_, Some("on" | "trailing-strings")) => {
            Err(unsupported_argument(function, "experimental_allow_partial"))
        }
        (_, Some(_)) => Err(ExcType::value_error(MESSAGE)),
        _ => Err(ExcType::type_error(format!("argument 'allow_partial': {MESSAGE}"))),
    }
}

/// The `NotImplementedError` for a pydantic option Monty does not model.
fn unsupported_argument(function: &str, argument: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::NotImplementedError,
        format!("{function}() argument '{argument}' is not supported"),
    )
    .into()
}
