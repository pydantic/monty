//! `pydantic.Field(...)` and `pydantic.ConfigDict(...)`.
//!
//! `Field` builds a [`FieldInfo`] record from the arguments
//! `limitations/pydantic.md` lists, refusing the rest; the builder reads the
//! record from `Annotated` metadata or a dataclass default. `ConfigDict` is a
//! `TypedDict` in pydantic, so it returns the plain dict its keywords make.

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext, HeapData},
    intern::StaticStrings,
    types::{Dict, FieldInfo},
    value::{Marker, Value},
};

/// pydantic's `Field` signature; `default` is positional, the rest keyword-only.
///
/// Raw `Value`s because `Field` is a `def`: pydantic stores what it is given and
/// checks types while building the schema, which here is the body below.
#[derive(FromArgs)]
#[from_args(name = "Field", style = def)]
struct FieldArgs {
    #[from_args(default = Value::Marker(Marker(StaticStrings::Missing)))]
    default: Value,
    #[from_args(kw_only, default = Value::None)]
    default_factory: Value,
    #[from_args(kw_only, default = Value::None)]
    alias: Value,
    #[from_args(kw_only, default = Value::None)]
    alias_priority: Value,
    #[from_args(kw_only, default = Value::None)]
    validation_alias: Value,
    #[from_args(kw_only, default = Value::None)]
    serialization_alias: Value,
    #[from_args(kw_only, default = Value::None)]
    title: Value,
    #[from_args(kw_only, default = Value::None)]
    field_title_generator: Value,
    #[from_args(kw_only, default = Value::None)]
    description: Value,
    #[from_args(kw_only, default = Value::None)]
    examples: Value,
    #[from_args(kw_only, default = Value::None)]
    exclude: Value,
    #[from_args(kw_only, default = Value::None)]
    exclude_if: Value,
    #[from_args(kw_only, default = Value::None)]
    discriminator: Value,
    #[from_args(kw_only, default = Value::None)]
    deprecated: Value,
    #[from_args(kw_only, default = Value::None)]
    json_schema_extra: Value,
    #[from_args(kw_only, default = Value::None)]
    frozen: Value,
    #[from_args(kw_only, default = Value::None)]
    validate_default: Value,
    #[from_args(kw_only, default = Value::None)]
    repr: Value,
    #[from_args(kw_only, default = Value::None)]
    init: Value,
    #[from_args(kw_only, default = Value::None)]
    init_var: Value,
    #[from_args(kw_only, default = Value::None)]
    kw_only: Value,
    #[from_args(kw_only, default = Value::None, static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(kw_only, default = Value::None)]
    strict: Value,
    #[from_args(kw_only, default = Value::None)]
    coerce_numbers_to_str: Value,
    #[from_args(kw_only, default = Value::None)]
    gt: Value,
    #[from_args(kw_only, default = Value::None)]
    ge: Value,
    #[from_args(kw_only, default = Value::None)]
    lt: Value,
    #[from_args(kw_only, default = Value::None)]
    le: Value,
    #[from_args(kw_only, default = Value::None)]
    multiple_of: Value,
    #[from_args(kw_only, default = Value::None)]
    allow_inf_nan: Value,
    #[from_args(kw_only, default = Value::None)]
    max_digits: Value,
    #[from_args(kw_only, default = Value::None)]
    decimal_places: Value,
    #[from_args(kw_only, default = Value::None)]
    min_length: Value,
    #[from_args(kw_only, default = Value::None)]
    max_length: Value,
    #[from_args(kw_only, default = Value::None)]
    union_mode: Value,
    #[from_args(kw_only, default = Value::None)]
    fail_fast: Value,
    /// pydantic folds unknown keywords into `json_schema_extra`, with a deprecation warning Monty has no channel for.
    #[from_args(varkwargs)]
    extra: KwargsValues,
}

/// `Field(...)`: a [`FieldInfo`] on the heap.
pub(super) fn field(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FieldArgs {
        default,
        default_factory,
        alias,
        alias_priority,
        validation_alias,
        serialization_alias,
        title,
        field_title_generator,
        description,
        examples,
        exclude,
        exclude_if,
        discriminator,
        deprecated,
        json_schema_extra,
        frozen,
        validate_default,
        repr,
        init,
        init_var,
        kw_only,
        pattern,
        strict,
        coerce_numbers_to_str,
        gt,
        ge,
        lt,
        le,
        multiple_of,
        allow_inf_nan,
        max_digits,
        decimal_places,
        min_length,
        max_length,
        union_mode,
        fail_fast,
        extra,
    } = FieldArgs::from_args(args, vm)?;
    // Every argument is guarded until it is stored or refused.
    let mut guard = DropGuard::new(
        vec![
            default,
            default_factory,
            alias,
            validation_alias,
            serialization_alias,
            title,
            description,
            examples,
            exclude,
            json_schema_extra,
            frozen,
            pattern,
            strict,
            coerce_numbers_to_str,
            gt,
            ge,
            lt,
            le,
            multiple_of,
            min_length,
            max_length,
        ],
        vm,
    );
    let (values, vm) = guard.as_parts();
    let unsupported = [
        ("alias_priority", alias_priority),
        ("field_title_generator", field_title_generator),
        ("exclude_if", exclude_if),
        ("discriminator", discriminator),
        ("deprecated", deprecated),
        ("validate_default", validate_default),
        ("repr", repr),
        ("init", init),
        ("init_var", init_var),
        ("kw_only", kw_only),
        ("allow_inf_nan", allow_inf_nan),
        ("max_digits", max_digits),
        ("decimal_places", decimal_places),
        ("union_mode", union_mode),
        ("fail_fast", fail_fast),
    ];
    let refused = unsupported.iter().find(|(_, value)| !matches!(value, Value::None));
    let refused = refused.map(|(name, _)| *name);
    for (_, value) in unsupported {
        value.drop_with(vm);
    }
    if let Some(name) = refused {
        extra.drop_with(vm);
        return Err(unsupported_argument(name));
    }
    let [
        default,
        default_factory,
        alias,
        validation_alias,
        serialization_alias,
        title,
        description,
        examples,
        exclude,
        json_schema_extra,
        frozen,
        pattern,
        strict,
        coerce_numbers_to_str,
        gt,
        ge,
        lt,
        le,
        multiple_of,
        min_length,
        max_length,
    ] = values.as_slice()
    else {
        unreachable!("the guard holds the 21 stored arguments")
    };
    let unset = |value: &Value| matches!(value, Value::None);
    if !matches!(default, Value::Marker(Marker(StaticStrings::Missing))) && !unset(default_factory) {
        extra.drop_with(vm);
        return Err(ExcType::type_error("cannot specify both default and default_factory"));
    }
    let text = |name: &str, value: &Value| -> RunResult<Option<String>> {
        match value {
            Value::None => Ok(None),
            other => match other.as_str(vm) {
                Some(text) => Ok(Some(text.to_owned())),
                None => Err(argument_type_error(name, "str", other, vm)),
            },
        }
    };
    let flag = |name: &str, value: &Value| -> RunResult<Option<bool>> {
        match value {
            Value::None => Ok(None),
            Value::Bool(flag) => Ok(Some(*flag)),
            other => Err(argument_type_error(name, "bool", other, vm)),
        }
    };
    let length = |name: &str, value: &Value| -> RunResult<Option<usize>> {
        match value {
            Value::None => Ok(None),
            Value::Int(n) if *n >= 0 => Ok(Some(usize::try_from(*n).expect("non-negative i64 fits usize"))),
            Value::Int(_) => Err(ExcType::value_error(format!("{name} must be non-negative"))),
            other => Err(argument_type_error(name, "int", other, vm)),
        }
    };
    // Checked before anything is cloned, so a rejection leaves nothing to release.
    let checked = (|| -> RunResult<FieldInfo> {
        let pattern = match pattern {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::RePattern(compiled) => Some(compiled.pattern().to_owned()),
                _ => text("pattern", pattern)?,
            },
            other => text("pattern", other)?,
        };
        if !unset(examples) && !matches!(examples, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::List(_))) {
            return Err(argument_type_error("examples", "list", examples, vm));
        }
        if !unset(json_schema_extra)
            && !matches!(json_schema_extra, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)))
        {
            return Err(argument_type_error("json_schema_extra", "dict", json_schema_extra, vm));
        }
        Ok(FieldInfo {
            default: None,
            default_factory: None,
            alias: text("alias", alias)?,
            validation_alias: text("validation_alias", validation_alias)?,
            serialization_alias: text("serialization_alias", serialization_alias)?,
            title: text("title", title)?,
            description: text("description", description)?,
            examples: None,
            exclude: flag("exclude", exclude)?,
            frozen: flag("frozen", frozen)?,
            json_schema_extra: None,
            strict: flag("strict", strict)?,
            coerce_numbers_to_str: flag("coerce_numbers_to_str", coerce_numbers_to_str)?,
            gt: None,
            ge: None,
            lt: None,
            le: None,
            multiple_of: None,
            min_length: length("min_length", min_length)?,
            max_length: length("max_length", max_length)?,
            pattern,
        })
    })();
    let mut info = match checked {
        Ok(info) => info,
        Err(err) => {
            extra.drop_with(vm);
            return Err(err);
        }
    };
    let owned = |value: &Value| (!unset(value)).then(|| value.clone_with_heap(vm.heap));
    info.default =
        (!matches!(default, Value::Marker(Marker(StaticStrings::Missing)))).then(|| default.clone_with_heap(vm.heap));
    info.default_factory = owned(default_factory);
    info.examples = owned(examples);
    info.gt = owned(gt);
    info.ge = owned(ge);
    info.lt = owned(lt);
    info.le = owned(le);
    info.multiple_of = owned(multiple_of);
    info.json_schema_extra = owned(json_schema_extra);
    let (values, vm) = guard.into_parts();
    values.drop_with(vm);
    let mut info = DropGuard::new(info, vm);
    let (record, vm) = info.as_parts_mut();
    if !extra.is_empty() {
        // Unknown keywords join `json_schema_extra`, as pydantic's deprecated path does.
        let mut extras = Dict::from_pairs(extra.into_iter().collect(), vm)?;
        if let Some(given) = record.json_schema_extra.take() {
            defer_drop!(given, vm);
            let Value::Ref(id) = given else {
                unreachable!("json_schema_extra was checked to be a dict")
            };
            let HeapData::Dict(given) = vm.heap.get(*id) else {
                unreachable!("json_schema_extra was checked to be a dict")
            };
            let pairs: Vec<(Value, Value)> = given
                .iter()
                .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                .collect();
            for (key, value) in pairs {
                if let Some(old) = extras.set(key, value, vm)? {
                    old.drop_with(vm);
                }
            }
        }
        record.json_schema_extra = Some(Value::Ref(vm.heap.allocate(HeapData::Dict(extras))));
    }
    let (info, vm) = info.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::FieldInfo(Box::new(info)))))
}

/// `ConfigDict(**settings)`: the dict pydantic's `TypedDict` call returns.
pub(super) fn config_dict(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ConfigDictArgs { settings } = ConfigDictArgs::from_args(args, vm)?;
    let dict = Dict::from_pairs(settings.into_iter().collect(), vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// `ConfigDict` takes keywords only; a `TypedDict` call checks nothing at runtime.
#[derive(FromArgs)]
#[from_args(name = "ConfigDict", style = def)]
struct ConfigDictArgs {
    #[from_args(varkwargs)]
    settings: KwargsValues,
}

/// `NotImplementedError` for a `Field()` argument Monty does not model.
fn unsupported_argument(argument: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::NotImplementedError,
        format!("Field() argument '{argument}' is not supported"),
    )
    .into()
}

/// `TypeError` for a `Field()` argument of the wrong type. pydantic stores the
/// value and fails while building the schema; Monty checks here.
fn argument_type_error(argument: &str, expected: &str, value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!(
        "Field() argument '{argument}' must be {expected}, not {}",
        value.py_type_name(vm)
    ))
}
