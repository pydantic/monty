//! `pydantic.TypeAdapter`: a validator arena built once from a type hint,
//! with the methods that run it.
//!
//! The arena is shared through an `Rc` so a validation can run Python (a
//! `default_factory`, `__post_init__`) while reading it without borrowing
//! the heap entry; the adapter object is kept alive for the call so the
//! values the arena owns stay valid.

use std::{cell::RefCell, fmt::Write, rc::Rc};

use jiter::Jiter;
use monty_types::ValidationInputType;
use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    modules::{
        dataclasses::is_dataclass_class,
        pydantic::{
            arena::{Arena, ValidatorId},
            build,
            catalogue::ErrorType,
            config::Config,
            errors::{ValError, ValResult, into_run_error, json_invalid_error},
            input::State,
            json_input::JsonInput,
            json_schema,
            python_input::PyInput,
            serialize::{self, Mode, Options},
            validate::validate,
        },
    },
    types::{
        LazyHeapSet, PyTrait, Type,
        decimal::py_context::{current_context, settings_of},
    },
    value::{EitherStr, Value},
};

/// A built `TypeAdapter`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TypeAdapter {
    arena: Rc<Arena>,
    root: ValidatorId,
    /// The `ValidationError` title: the config's `title`, else the root validator's name.
    title: String,
    /// `display_as_type` of the hint, for `repr()`.
    type_display: String,
    /// The config's `strict`, the default when a call passes `strict=None`.
    #[serde(default)]
    strict: bool,
}

/// Arguments of `TypeAdapter(type, /, *, config=None, _parent_depth=2, module=None)`.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.__init__", style = def)]
struct TypeAdapterArgs {
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
            r#type: hint,
            config,
            parent_depth,
            module,
        } = TypeAdapterArgs::from_args(args, vm)?;
        defer_drop!(hint, vm);
        defer_drop!(config, vm);
        defer_drop!(parent_depth, vm);
        defer_drop!(module, vm);
        if !matches!(module, Value::None) {
            return Err(unsupported_argument("TypeAdapter.__init__", "module"));
        }
        // A dataclass carries its own `__pydantic_config__`, which pydantic refuses to shadow.
        if !matches!(config, Value::None) && matches!(hint, Value::Ref(id) if is_dataclass_class(*id, vm)) {
            return Err(SimpleException::new_msg(
                ExcType::PydanticUserError,
                "Cannot use `config` when the type is a BaseModel, dataclass or TypedDict. These types can have \
                 their own config and setting the config via the `config` parameter to TypeAdapter will not \
                 override it, thus the `config` you passed to TypeAdapter becomes meaningless, which is probably \
                 not what you want."
                    .to_owned(),
            )
            .into());
        }
        let config = Config::parse(config, vm)?;
        let mut arena = Arena::default();
        let root = match build::build(hint, &config, &mut arena, vm) {
            Ok(root) => root,
            Err(err) => {
                arena.drop_with(vm);
                return Err(err);
            }
        };
        let title = config.title.clone().unwrap_or_else(|| arena.name(root));
        let adapter = Self {
            arena: Rc::new(arena),
            root,
            title,
            type_display: build::display_as_type(hint, vm),
            strict: config.strict,
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::TypeAdapter(Box::new(adapter)))))
    }

    /// The arena, its root and the error title, for a pydantic dataclass's `__init__`.
    pub(crate) fn parts(&self) -> (Rc<Arena>, ValidatorId, String) {
        (Rc::clone(&self.arena), self.root, self.title.clone())
    }

    /// Invokes `on_child` for every heap id the arena owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, on_child: impl FnMut(HeapId)) {
        self.arena.for_each_child_id(on_child);
    }
}

impl HeapItem for TypeAdapter {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // A validation in flight keeps the adapter alive, so the arena is unshared here.
        match Rc::get_mut(&mut self.arena) {
            Some(arena) => arena.py_dec_ref_ids(stack),
            None => self.arena.for_each_child_id(|id| stack.push(id)),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for TypeAdapter {
    fn drop_with(self, ctx: &mut C) {
        if let Ok(arena) = Rc::try_unwrap(self.arena) {
            arena.drop_with(ctx);
        }
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
    /// A shared handle on the arena, plus a reference that keeps the adapter
    /// alive while a validation runs Python.
    fn arena_handle(&self, vm: &mut VM<'h>) -> (Rc<Arena>, ValidatorId, String, Value) {
        let this = self.get(vm.heap);
        let parts = (Rc::clone(&this.arena), this.root, this.title.clone());
        let keep_alive = self.clone_value(vm.heap);
        (parts.0, parts.1, parts.2, keep_alive)
    }

    /// The strictness a call validates with: its `strict=` when given, else the config's.
    fn call_strict(&self, strict: &Value, vm: &VM<'h>) -> RunResult<bool> {
        Ok(strict_flag(strict, vm)?.unwrap_or(self.get(vm.heap).strict))
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
        if !matches!(experimental_allow_partial, Value::Bool(false)) {
            return Err(unsupported_argument(
                "TypeAdapter.validate_python",
                "experimental_allow_partial",
            ));
        }
        let strict = self.call_strict(strict, vm)?;
        let (arena, root, title, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
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
        if !matches!(experimental_allow_partial, Value::Bool(false)) {
            return Err(unsupported_argument(
                "TypeAdapter.validate_json",
                "experimental_allow_partial",
            ));
        }
        let strict = self.call_strict(strict, vm)?;
        let (arena, root, title, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
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
                let description = err.description(&parser.borrow());
                json_invalid_error(&title, description, data.clone_with_heap(vm.heap), vm)
            }
            other => into_run_error(other, &title, ValidationInputType::Json, vm),
        })
    }
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
        let by_alias = !matches!(by_alias, Value::None) && bool_flag("by_alias", by_alias, vm)?;
        bool_flag("round_trip", round_trip, vm)?;
        let exclude_none = bool_flag("exclude_none", exclude_none, vm)?;
        for (name, value) in [
            ("exclude_unset", exclude_unset),
            ("exclude_defaults", exclude_defaults),
            ("exclude_computed_fields", exclude_computed_fields),
            ("serialize_as_any", serialize_as_any),
        ] {
            if bool_flag(name, value, vm)? {
                return Err(unsupported_argument(function, name));
            }
        }
        let context = current_context(vm);
        let capitals = settings_of(vm, context).capitals;
        Ok(Options {
            mode,
            exclude_none,
            by_alias,
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
        let (arena, root, _, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
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
        let indent = match indent {
            Value::None => None,
            Value::Int(n) if *n >= 0 => Some(usize::try_from(*n).unwrap_or(usize::MAX)),
            Value::Int(_) => {
                return Err(SimpleException::new_msg(
                    ExcType::OverflowError,
                    "can't convert negative int to unsigned".to_owned(),
                )
                .into());
            }
            other => {
                return Err(ExcType::type_error(format!(
                    "argument 'indent': '{}' object cannot be interpreted as an integer",
                    other.py_type_name(vm)
                )));
            }
        };
        let ensure_ascii = bool_flag("ensure_ascii", ensure_ascii, vm)?;
        let options = common.check("TypeAdapter.dump_json", Mode::Json, vm)?;
        let (arena, root, _, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
        serialize::dump_json(&arena, root, instance, options, indent, ensure_ascii, vm)
    }
}

/// `mode='json'` selects JSON mode; pydantic treats any other `str` as `'python'`.
fn dump_mode(mode: &Value, vm: &VM<'_>) -> RunResult<Mode> {
    match (mode, mode.as_str(vm)) {
        (Value::None, _) => Ok(Mode::Python),
        (_, Some("json")) => Ok(Mode::Json),
        (_, Some(_)) => Ok(Mode::Python),
        (other, None) => Err(ExcType::type_error(format!(
            "argument 'mode': '{}' object is not an instance of 'str'",
            other.py_type_name(vm)
        ))),
    }
}

/// A `bool` option, with pydantic-core's `TypeError` for any other type.
fn bool_flag(name: &str, value: &Value, vm: &VM<'_>) -> RunResult<bool> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(ExcType::type_error(format!(
            "argument '{name}': '{}' object is not an instance of 'bool'",
            other.py_type_name(vm)
        ))),
    }
}

/// Arguments of `json_schema(*, by_alias=None, ref_template=..., union_format='any_of',
/// schema_generator=GenerateJsonSchema, mode='validation')`.
#[derive(FromArgs)]
#[from_args(name = "TypeAdapter.json_schema", style = def)]
struct JsonSchemaArgs {
    #[from_args(kw_only, default = Value::None)]
    by_alias: Value,
    #[from_args(kw_only, default = Value::None)]
    ref_template: Value,
    #[from_args(kw_only, default = Value::None)]
    union_format: Value,
    #[from_args(kw_only, default = Value::None)]
    schema_generator: Value,
    #[from_args(kw_only, default = Value::None)]
    mode: Value,
}

impl<'h> HeapObjectRead<'h, TypeAdapter> {
    /// `json_schema(...)`: the schema `json_schema::generate` builds from the arena.
    fn json_schema(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let JsonSchemaArgs {
            by_alias,
            ref_template,
            union_format,
            schema_generator,
            mode,
        } = JsonSchemaArgs::from_args(args, vm)?;
        defer_drop!(by_alias, vm);
        defer_drop!(ref_template, vm);
        defer_drop!(union_format, vm);
        defer_drop!(schema_generator, vm);
        defer_drop!(mode, vm);
        if !matches!(schema_generator, Value::None) {
            return Err(unsupported_argument("TypeAdapter.json_schema", "schema_generator"));
        }
        if !matches!(union_format, Value::None) && union_format.as_str(vm) != Some("any_of") {
            return Err(unsupported_argument("TypeAdapter.json_schema", "union_format"));
        }
        let by_alias = match by_alias {
            Value::None => None,
            Value::Bool(flag) => Some(*flag),
            other => {
                return Err(ExcType::type_error(format!(
                    "argument 'by_alias': '{}' object is not an instance of 'bool'",
                    other.py_type_name(vm)
                )));
            }
        };
        // pydantic calls `ref_template.format(model=...)`, so a non-string fails on `.format`.
        let ref_template = match ref_template {
            Value::None => json_schema::DEFAULT_REF_TEMPLATE.to_owned(),
            other => other
                .as_str(vm)
                .ok_or_else(|| ExcType::attribute_error(other.py_type_name(vm), "format"))?
                .to_owned(),
        };
        let serialization = match mode {
            Value::None => false,
            other => match other.as_str(vm) {
                Some("validation") => false,
                Some("serialization") => true,
                _ => {
                    let mut shown = String::new();
                    other.py_repr_fmt(&mut shown, vm, &mut LazyHeapSet::default())?;
                    return Err(SimpleException::new_msg(
                        ExcType::AssertionError,
                        format!("Expected code to be unreachable, but got: {shown}"),
                    )
                    .into());
                }
            },
        };
        let (arena, root, _, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
        json_schema::generate(&arena, root, by_alias, &ref_template, serialization, vm)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, TypeAdapter> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::TypeAdapter
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// `TypeAdapter(list[int])`, as pydantic's `__repr__` shows the type.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "TypeAdapter({})", self.get(vm.heap).type_display)?)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::ValidatePython) => self.validate_python(args, vm).map(CallResult::Value),
            Some(StaticStrings::ValidateJson) => self.validate_json(args, vm).map(CallResult::Value),
            Some(StaticStrings::DumpPython) => self.dump_python(args, vm).map(CallResult::Value),
            Some(StaticStrings::DumpJson) => self.dump_json(args, vm).map(CallResult::Value),
            Some(StaticStrings::JsonSchema) => self.json_schema(args, vm).map(CallResult::Value),
            Some(StaticStrings::ValidateStrings) => {
                args.drop_with(vm);
                Err(SimpleException::new_msg(
                    ExcType::NotImplementedError,
                    "TypeAdapter.validate_strings() is not supported".to_owned(),
                )
                .into())
            }
            _ => Err(ExcType::attribute_error_method("TypeAdapter", attr, args, vm)),
        }
    }
}

/// `strict=None` or a `bool`, with pydantic-core's `TypeError` for anything else.
fn strict_flag(value: &Value, vm: &VM<'_>) -> RunResult<Option<bool>> {
    match value {
        Value::None => Ok(None),
        Value::Bool(strict) => Ok(Some(*strict)),
        other => Err(ExcType::type_error(format!(
            "argument 'strict': '{}' object is not an instance of 'bool'",
            other.py_type_name(vm)
        ))),
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
