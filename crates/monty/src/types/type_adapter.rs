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
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    modules::pydantic::{
        arena::{Arena, ValidatorId},
        build,
        catalogue::ErrorType,
        errors::{ValError, ValResult, into_run_error, json_invalid_error},
        input::State,
        json_input::JsonInput,
        python_input::PyInput,
        validate::validate,
    },
    types::{LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// A built `TypeAdapter`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TypeAdapter {
    arena: Rc<Arena>,
    root: ValidatorId,
    /// The root validator's name: the `ValidationError` title.
    title: String,
    /// `display_as_type` of the hint, for `repr()`.
    type_display: String,
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
        if !matches!(config, Value::None) {
            return Err(unsupported_argument("TypeAdapter.__init__", "config"));
        }
        if !matches!(module, Value::None) {
            return Err(unsupported_argument("TypeAdapter.__init__", "module"));
        }
        let mut arena = Arena::default();
        let root = match build::build(hint, &mut arena, vm) {
            Ok(root) => root,
            Err(err) => {
                arena.drop_with(vm);
                return Err(err);
            }
        };
        let title = arena.name(root);
        let adapter = Self {
            arena: Rc::new(arena),
            root,
            title,
            type_display: build::display_as_type(hint, vm),
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::TypeAdapter(Box::new(adapter)))))
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
                object.drop_with(vm);
                return Err(unsupported_argument("TypeAdapter.validate_python", name));
            }
        }
        if !matches!(experimental_allow_partial, Value::Bool(false)) {
            object.drop_with(vm);
            return Err(unsupported_argument(
                "TypeAdapter.validate_python",
                "experimental_allow_partial",
            ));
        }
        let strict = strict_flag(strict, vm)?;
        let (arena, root, title, keep_alive) = self.arena_handle(vm);
        defer_drop!(keep_alive, vm);
        let mut input = PyInput::new(object);
        let mut state = State::new(strict);
        let result = validate(&arena, root, &mut input, &mut state, vm);
        input.drop_with(vm);
        result.map_err(|err| into_run_error(err, &title, ValidationInputType::Python, vm))
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
        let strict = strict_flag(strict, vm)?;
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
            Some(
                StaticStrings::ValidateStrings
                | StaticStrings::DumpPython
                | StaticStrings::DumpJson
                | StaticStrings::JsonSchema,
            ) => {
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
        Value::Bool(strict) => Ok(*strict),
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
