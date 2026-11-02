//! Building a validator arena from a type hint: the walk pydantic's
//! `_generate_schema.py` does, over the small, closed set of hint forms Monty
//! can evaluate.
//!
//! Dataclasses are memoised by class id, with their slot reserved before the
//! fields are walked, so a field that names the class again gets the reserved
//! id and a recursive type builds without definitions or late binding. A
//! dataclass's field hints come from `typing.get_type_hints`, evaluated
//! synchronously, which is where a forward reference to an undefined name
//! fails.
//!
//! A `Field(...)` reaches a validator from `Annotated` metadata or a dataclass
//! default, and a config from `TypeAdapter(config=)` or a dataclass's
//! `__pydantic_config__`; both become `Strict` and `Constrained` wrappers
//! around the validator they apply to, so validation itself stays unaware of them.

use std::mem;

use ahash::AHashMap;

use crate::{
    args::{ArgValues, KwargsValues},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        dataclasses::is_dataclass_class,
        pydantic::{
            arena::{
                Arena, Constraints, DataclassValidator, FieldDefault, FieldValidator, LiteralValidator,
                NamedTupleValidator, Pattern, SchemaMeta, StrConstraints, Validator, ValidatorId,
            },
            config::Config,
            constraints::{NumberKind, number_constraints},
            errors::{ERROR_URL_PREFIX, dict_pairs},
            snapshot::repr_string,
        },
        typing::TypingFunctions,
    },
    types::{BuiltinType, Dict, FieldInfo, Type, typing_alias::TypingForm, union::tuple_items},
    value::{Marker, Value},
};

/// Builds the validator for `hint` into `arena` under `config`, returning its id.
///
/// A hint that cannot be validated raises `PydanticSchemaGenerationError`; the
/// caller discards the arena then.
pub(crate) fn build(hint: &Value, config: &Config, arena: &mut Arena, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
    build_field(hint, None, config, arena, vm)
}

/// [`build`] for a parameter or field whose `Field(...)` came from its default.
pub(crate) fn build_field(
    hint: &Value,
    field: Option<&FieldInfo>,
    config: &Config,
    arena: &mut Arena,
    vm: &mut VM<'_>,
) -> RunResult<ValidatorId> {
    let mut builder = Builder {
        arena,
        classes: AHashMap::new(),
        config: config.clone(),
    };
    builder.walk(hint, field, vm)
}

/// The walk's state: the arena under construction, the dataclasses already
/// placed in it, and the config in force (a dataclass's own while its fields are walked).
struct Builder<'a> {
    arena: &'a mut Arena,
    classes: AHashMap<HeapId, ValidatorId>,
    config: Config,
}

impl Builder<'_> {
    /// Builds `hint`, applying `field`'s constraints, strictness and schema
    /// metadata to the result; `Annotated` layers are merged into `field` first,
    /// so each applies once however deep the hint nests them.
    fn walk(&mut self, hint: &Value, field: Option<&FieldInfo>, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        let (hint, merged) = unwrap_annotated(hint, field, vm)?;
        defer_drop!(hint, vm);
        defer_drop!(merged, vm);
        let field = merged.as_ref();
        let id = match hint {
            Value::None => self.arena.push(Validator::None),
            Value::Marker(Marker(StaticStrings::Any)) => self.arena.push(Validator::Any),
            Value::Builtin(Builtins::Type(ty)) => self.builtin(*ty, hint, field, vm)?,
            Value::Ref(id) => self.heap_hint(*id, hint, field, vm)?,
            _ => return Err(schema_error(hint, vm)),
        };
        let id = match field.and_then(|field| field.strict) {
            Some(strict) => self.arena.push(Validator::Strict {
                inner: id,
                strict,
                scope: false,
            }),
            None => id,
        };
        let meta = field.map(|field| SchemaMeta {
            title: field.title.clone(),
            description: field.description.clone(),
            examples: field.examples.as_ref().map(|v| v.clone_with_heap(vm.heap)),
            json_schema_extra: field.json_schema_extra.as_ref().map(|v| v.clone_with_heap(vm.heap)),
            default: field.default.as_ref().map(|v| v.clone_with_heap(vm.heap)),
        });
        Ok(match meta {
            Some(meta) if !meta.is_empty() => self.arena.push(Validator::Meta {
                inner: id,
                meta: Box::new(meta),
            }),
            Some(meta) => {
                meta.drop_with(vm);
                id
            }
            None => id,
        })
    }

    /// Wraps `inner` in the constraints `field` and the config put on its
    /// kind of value; a constraint that does not apply to the kind is dropped.
    fn constrain(&mut self, inner: ValidatorId, field: Option<&FieldInfo>, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let mut number = |kind| field.map_or(Ok(None), |field| number_constraints(field, kind, vm));
        let length = || {
            field
                .filter(|field| field.min_length.is_some() || field.max_length.is_some())
                .map(|field| (field.min_length, field.max_length))
        };
        let constraints = match self.arena.get(inner) {
            Validator::Int => number(NumberKind::Int)?.map(Constraints::Number),
            Validator::Float => number(NumberKind::Float)?.map(Constraints::Number),
            Validator::Decimal => number(NumberKind::Decimal)?.map(Constraints::Number),
            Validator::Str => self
                .str_constraints(field)?
                .map(|rules| Constraints::Str(Box::new(rules))),
            Validator::Bytes => length().map(|(min_length, max_length)| Constraints::Bytes { min_length, max_length }),
            Validator::List(_)
            | Validator::Set(_)
            | Validator::FrozenSet(_)
            | Validator::Tuple { .. }
            | Validator::Dict { .. } => {
                length().map(|(min_length, max_length)| Constraints::Length { min_length, max_length })
            }
            // pydantic bounds these by value too; Monty does not yet (see limitations/pydantic.md).
            Validator::DateTime | Validator::Date | Validator::Time | Validator::TimeDelta
                if field.is_some_and(|field| {
                    field.gt.is_some() || field.ge.is_some() || field.lt.is_some() || field.le.is_some()
                }) =>
            {
                return Err(ExcType::not_implemented(
                    "Field() bounds (gt, ge, lt, le) on a date, time, datetime or timedelta are not supported",
                )
                .into());
            }
            _ => None,
        };
        Ok(match constraints {
            Some(constraints) => self.arena.push(Validator::Constrained {
                inner,
                constraints: Box::new(constraints),
            }),
            None => inner,
        })
    }

    /// The string rules in force: the field's where set, the config's otherwise.
    fn str_constraints(&self, field: Option<&FieldInfo>) -> RunResult<Option<StrConstraints>> {
        let config = &self.config;
        let rules = StrConstraints {
            coerce_numbers_to_str: field
                .and_then(|field| field.coerce_numbers_to_str)
                .unwrap_or(config.coerce_numbers_to_str),
            strip_whitespace: config.str_strip_whitespace,
            to_lower: config.str_to_lower,
            to_upper: config.str_to_upper,
            min_length: field.and_then(|field| field.min_length).or(config.str_min_length),
            max_length: field.and_then(|field| field.max_length).or(config.str_max_length),
            pattern: match field.and_then(|field| field.pattern.as_deref()) {
                Some(source) => Some(Pattern::new(source.to_owned())?),
                None => None,
            },
        };
        let any = rules.coerce_numbers_to_str
            || rules.strip_whitespace
            || rules.to_lower
            || rules.to_upper
            || rules.min_length.is_some()
            || rules.max_length.is_some()
            || rules.pattern.is_some();
        Ok(any.then_some(rules))
    }

    /// A bare builtin type: the scalar validators, and the containers with `Any` items.
    fn builtin(
        &mut self,
        ty: BuiltinType,
        hint: &Value,
        field: Option<&FieldInfo>,
        vm: &mut VM<'_>,
    ) -> RunResult<ValidatorId> {
        let validator = match ty {
            BuiltinType::NoneType => Validator::None,
            BuiltinType::Object => Validator::Any,
            BuiltinType::Bool => Validator::Bool,
            BuiltinType::Int => Validator::Int,
            BuiltinType::Float => Validator::Float,
            BuiltinType::Str => Validator::Str,
            BuiltinType::Bytes => Validator::Bytes,
            BuiltinType::DateTime => Validator::DateTime,
            BuiltinType::Date => Validator::Date,
            BuiltinType::Time => Validator::Time,
            BuiltinType::TimeDelta => Validator::TimeDelta,
            BuiltinType::Decimal => Validator::Decimal,
            BuiltinType::List => Validator::List(self.arena.push(Validator::Any)),
            BuiltinType::Set => Validator::Set(self.arena.push(Validator::Any)),
            BuiltinType::FrozenSet => Validator::FrozenSet(self.arena.push(Validator::Any)),
            BuiltinType::Tuple => Validator::Tuple {
                items: vec![self.arena.push(Validator::Any)],
                variadic: true,
            },
            BuiltinType::Dict => Validator::Dict {
                key: self.arena.push(Validator::Any),
                value: self.arena.push(Validator::Any),
            },
            _ => return Err(schema_error(hint, vm)),
        };
        let inner = self.arena.push(validator);
        self.constrain(inner, field, vm)
    }

    /// A heap-allocated hint: a generic alias, a union, a `Literal` or
    /// `Annotated`, a dataclass, a namedtuple class or another `TypeAdapter`.
    fn heap_hint(
        &mut self,
        id: HeapId,
        hint: &Value,
        field: Option<&FieldInfo>,
        vm: &mut VM<'_>,
    ) -> RunResult<ValidatorId> {
        match vm.heap.get(id) {
            HeapData::GenericAlias(alias) => {
                let (origin, args) = (alias.origin(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let args = tuple_items(args, vm)?.expect("GenericAlias::args is a tuple");
                defer_drop!(args, vm);
                let inner = self.generic(origin, args, hint, vm)?;
                self.constrain(inner, field, vm)
            }
            HeapData::Union(union) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let members = tuple_items(args, vm)?.expect("Union::args is a tuple");
                defer_drop!(members, vm);
                self.union(members, field, vm)
            }
            HeapData::TypingAlias(alias) => {
                let (form, args) = (alias.form(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let args = tuple_items(args, vm)?.expect("TypingAlias::args is a tuple");
                defer_drop!(args, vm);
                match form {
                    TypingForm::Literal => Ok(self.literal(args, vm)),
                    // `walk` has already peeled every `Annotated` layer.
                    TypingForm::Annotated => self.walk(&args[0], field, vm),
                }
            }
            HeapData::Class(_) if is_dataclass_class(id, vm) => self.dataclass(id, hint, vm),
            HeapData::NamedTupleClass(class) => {
                let validator = NamedTupleValidator {
                    class: hint.clone_with_heap(vm.heap),
                    name: class.name(vm.interns).to_owned(),
                    field_names: class
                        .field_names()
                        .iter()
                        .map(|name| name.as_str(vm.interns).to_owned())
                        .collect(),
                    defaults: class.defaults().len(),
                };
                Ok(self.arena.push(Validator::NamedTuple(validator)))
            }
            _ => Err(schema_error(hint, vm)),
        }
    }

    /// `list[T]`, `set[T]`, `frozenset[T]`, `tuple[...]`, `dict[K, V]`. As in
    /// pydantic, surplus arguments are ignored, `dict` needs its two, and a
    /// variadic tuple has one item before its `...`.
    fn generic(
        &mut self,
        origin: BuiltinType,
        args: &[Value],
        hint: &Value,
        vm: &mut VM<'_>,
    ) -> RunResult<ValidatorId> {
        let validator = match (origin, args) {
            (BuiltinType::List, [item, ..]) => Validator::List(self.walk(item, None, vm)?),
            (BuiltinType::Set, [item, ..]) => Validator::Set(self.walk(item, None, vm)?),
            (BuiltinType::FrozenSet, [item, ..]) => Validator::FrozenSet(self.walk(item, None, vm)?),
            (BuiltinType::Dict, [key, value, ..]) => Validator::Dict {
                key: self.walk(key, None, vm)?,
                value: self.walk(value, None, vm)?,
            },
            (BuiltinType::Dict, args) => {
                return Err(ExcType::type_error(format!(
                    "Expected two type arguments for <class 'dict'>, got {}",
                    args.len()
                )));
            }
            (BuiltinType::Tuple, [item, Value::Ellipsis]) => Validator::Tuple {
                items: vec![self.walk(item, None, vm)?],
                variadic: true,
            },
            (BuiltinType::Tuple, items) if items.last().is_some_and(|last| matches!(last, Value::Ellipsis)) => {
                return Err(ExcType::value_error("Variable tuples can only have one type"));
            }
            (BuiltinType::Tuple, items) => {
                let mut ids = Vec::with_capacity(items.len());
                for item in items {
                    ids.push(self.walk(item, None, vm)?);
                }
                Validator::Tuple {
                    items: ids,
                    variadic: false,
                }
            }
            _ => return Err(schema_error(hint, vm)),
        };
        Ok(self.arena.push(validator))
    }

    /// `X | Y`: a `nullable` around the other members when `None` is one of
    /// them, a smart union of the rest. `field`'s constraints apply to each
    /// member, as they do to the inner type of an `Optional`.
    fn union(&mut self, members: &[Value], field: Option<&FieldInfo>, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let is_none = |member: &Value| {
            matches!(
                member,
                Value::None | Value::Builtin(Builtins::Type(BuiltinType::NoneType))
            )
        };
        let nullable = members.iter().any(is_none);
        let mut choices = Vec::new();
        // Strictness and schema metadata are applied once by the caller, so the
        // members see only the constraints.
        let constraints_only = field.map(|field| field.constraints_only(vm.heap));
        let mut walked = Ok(());
        for member in members.iter().filter(|member| !is_none(member)) {
            match self.walk(member, constraints_only.as_ref(), vm) {
                Ok(id) => choices.push(id),
                Err(err) => {
                    walked = Err(err);
                    break;
                }
            }
        }
        if let Some(constraints_only) = constraints_only {
            constraints_only.drop_with(vm);
        }
        walked?;
        let inner = match choices.len() {
            0 => self.arena.push(Validator::None),
            1 => choices[0],
            _ => self.arena.push(Validator::Union(choices)),
        };
        Ok(if nullable && !matches!(self.arena.get(inner), Validator::None) {
            self.arena.push(Validator::Nullable(inner))
        } else {
            inner
        })
    }

    /// `Literal[...]`: the choices as given, named by their reprs.
    fn literal(&mut self, choices: &[Value], vm: &mut VM<'_>) -> ValidatorId {
        let reprs: Vec<String> = choices.iter().map(|choice| repr_string(choice, vm)).collect();
        let expected = match reprs.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            Some((last, _)) => last.clone(),
            None => String::new(),
        };
        let validator = LiteralValidator {
            choices: choices.iter().map(|choice| choice.clone_with_heap(vm.heap)).collect(),
            expected,
            name: format!("literal[{}]", reprs.join(",")),
        };
        self.arena.push(Validator::Literal(validator))
    }

    /// A dataclass, memoised by class: the slot is reserved before the fields
    /// are walked so a self-referential field finds it.
    fn dataclass(&mut self, class_id: HeapId, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        if let Some(id) = self.classes.get(&class_id) {
            return Ok(*id);
        }
        let id = self.arena.reserve();
        self.classes.insert(class_id, id);
        let hints = type_hints(hint, vm)?;
        defer_drop!(hints, vm);
        let HeapData::Class(class) = vm.heap.get(class_id) else {
            unreachable!("is_dataclass_class checked a class")
        };
        let name = class.name().as_str(vm.interns).to_owned();
        let post_init = class
            .namespace()
            .get_by_str("__post_init__", vm.heap, vm.interns)
            .is_some();
        // The class's own config governs its fields; the enclosing one is restored after.
        let class_config = class
            .namespace()
            .get_by_str("__pydantic_config__", vm.heap, vm.interns)
            .map(|config| config.clone_with_heap(vm.heap));
        let class_config = match class_config {
            Some(config) => {
                defer_drop!(config, vm);
                Config::parse(config, vm)?
            }
            None => Config::default(),
        };
        let outer_config = mem::replace(&mut self.config, class_config);
        let built = self.dataclass_fields(class_id, id, hint, hints, name, post_init, vm);
        let class_config = mem::replace(&mut self.config, outer_config);
        built?;
        Ok(if class_config.strict {
            self.arena.push(Validator::Strict {
                inner: id,
                strict: true,
                scope: true,
            })
        } else {
            id
        })
    }

    /// Fills the reserved `id` with the dataclass's field validators, under the
    /// class's config already in force.
    #[expect(
        clippy::too_many_arguments,
        reason = "the reserved slot and the class's read-out parts"
    )]
    fn dataclass_fields(
        &mut self,
        class_id: HeapId,
        id: ValidatorId,
        hint: &Value,
        hints: &Value,
        name: String,
        post_init: bool,
        vm: &mut VM<'_>,
    ) -> RunResult<()> {
        let HeapData::Class(class) = vm.heap.get(class_id) else {
            unreachable!("is_dataclass_class checked a class")
        };
        let fields_dict = class
            .namespace()
            .get_by_str("__dataclass_fields__", vm.heap, vm.interns)
            .map(|fields| fields.clone_with_heap(vm.heap));
        let Some(fields_dict) = fields_dict else {
            unreachable!("is_dataclass_class checked for __dataclass_fields__")
        };
        defer_drop!(fields_dict, vm);
        let Some(pairs) = dict_pairs(fields_dict, vm) else {
            return Err(schema_error(hint, vm));
        };
        defer_drop!(pairs, vm);
        let mut fields = Vec::with_capacity(pairs.len());
        for (key, field) in pairs {
            // Field names come from the class's annotations, which are always interned.
            let Value::InternString(field_name) = key else {
                fields.drop_with(vm);
                return Err(schema_error(hint, vm));
            };
            let field_name = *field_name;
            let (default, factory) = match field {
                Value::Ref(field_id) => match vm.heap.get(*field_id) {
                    HeapData::DataclassField(field) => (
                        field.default().map(|v| v.clone_with_heap(vm.heap)),
                        field.default_factory().map(|v| v.clone_with_heap(vm.heap)),
                    ),
                    _ => (None, None),
                },
                _ => (None, None),
            };
            // A `Field(...)` as the default supplies the default and the field's settings.
            let info = match &default {
                Some(Value::Ref(default_id)) => match vm.heap.get(*default_id) {
                    HeapData::FieldInfo(info) => Some(info.clone_with_heap(vm.heap)),
                    _ => None,
                },
                _ => None,
            };
            let (default, factory) = match &info {
                Some(info) => {
                    default.drop_with(vm);
                    (
                        info.default.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                        info.default_factory.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                    )
                }
                None => (default, factory),
            };
            let default = match (default, factory) {
                (Some(value), _) => FieldDefault::Value(value),
                (None, Some(factory)) => FieldDefault::Factory(factory),
                (None, None) => FieldDefault::Required,
            };
            let hint_value = dict_of(hints, vm)
                .get_by_str(vm.interns.get_str(field_name), vm.heap, vm.interns)
                .map(|value| value.clone_with_heap(vm.heap));
            let validator = match &hint_value {
                Some(field_hint) => match self.walk(field_hint, info.as_ref(), vm) {
                    Ok(id) => id,
                    Err(err) => {
                        hint_value.drop_with(vm);
                        info.drop_with(vm);
                        default.drop_with(vm);
                        fields.drop_with(vm);
                        return Err(err);
                    }
                },
                None => self.arena.push(Validator::Any),
            };
            // Aliases and `exclude` come from the hint's `Annotated` metadata as well as the default.
            let effective = match &hint_value {
                Some(field_hint) => match unwrap_annotated(field_hint, info.as_ref(), vm) {
                    Ok(effective) => Some(effective),
                    Err(err) => {
                        hint_value.drop_with(vm);
                        info.drop_with(vm);
                        default.drop_with(vm);
                        fields.drop_with(vm);
                        return Err(err);
                    }
                },
                None => None,
            };
            let (alias, serialization_alias, exclude) = match effective.as_ref().and_then(|(_, info)| info.as_ref()) {
                Some(info) => (
                    info.lookup_alias().map(str::to_owned),
                    info.output_alias().map(str::to_owned),
                    info.exclude.unwrap_or(false),
                ),
                None => (None, None, false),
            };
            effective.drop_with(vm);
            hint_value.drop_with(vm);
            info.drop_with(vm);
            fields.push(FieldValidator {
                name: field_name,
                validator,
                default,
                alias,
                serialization_alias,
                exclude,
            });
        }
        self.arena.fill(
            id,
            Validator::Dataclass(DataclassValidator {
                class: hint.clone_with_heap(vm.heap),
                name,
                fields,
                post_init,
                populate_by_name: self.config.populate_by_name,
                extra_forbid: self.config.extra_forbid,
                title: self.config.title.clone(),
            }),
        );
        Ok(())
    }
}

/// `hint` with its `Annotated` layers peeled: the inner type and the `Field(...)`
/// items of every layer merged, inner layers first (pydantic flattens nesting),
/// then `field` (a dataclass default's) taking precedence over them all.
fn unwrap_annotated(hint: &Value, field: Option<&FieldInfo>, vm: &mut VM<'_>) -> RunResult<(Value, Option<FieldInfo>)> {
    let mut hint = hint.clone_with_heap(vm.heap);
    // Outermost layer first; merged in reverse below so an outer layer's items
    // follow an inner's, as `typing` flattens nested `Annotated`.
    let mut layers: Vec<Vec<FieldInfo>> = Vec::new();
    while let Value::Ref(id) = &hint {
        let HeapData::TypingAlias(alias) = vm.heap.get(*id) else {
            break;
        };
        if alias.form() != TypingForm::Annotated {
            break;
        }
        let args = alias.args(vm.heap);
        let items = match tuple_items(&args, vm) {
            Ok(items) => items,
            Err(err) => {
                args.drop_with(vm);
                hint.drop_with(vm);
                layers.drop_with(vm);
                return Err(err);
            }
        };
        args.drop_with(vm);
        let Some(mut items) = items else { break };
        let mut layer = Vec::new();
        for item in items.drain(1..) {
            if let Value::Ref(item_id) = &item
                && let HeapData::FieldInfo(info) = vm.heap.get(*item_id)
            {
                layer.push(info.clone_with_heap(vm.heap));
            }
            item.drop_with(vm);
        }
        layers.push(layer);
        hint.drop_with(vm);
        hint = items.pop().expect("Annotated has a subject");
    }
    let mut merged: Option<FieldInfo> = None;
    for info in layers
        .into_iter()
        .rev()
        .flatten()
        .chain(field.map(|field| field.clone_with_heap(vm.heap)))
    {
        merged = Some(match merged.take() {
            Some(so_far) => {
                let combined = so_far.merged(&info, vm);
                so_far.drop_with(vm);
                info.drop_with(vm);
                combined
            }
            None => info,
        });
    }
    Ok((hint, merged))
}

/// `typing.get_type_hints(cls, include_extras=True)`, run to completion: the
/// evaluated field hints keyed by name.
pub(crate) fn type_hints(class: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let get_type_hints = Value::ModuleFunction(ModuleFunctions::Typing(TypingFunctions::GetTypeHints));
    let args = ArgValues::ArgsKargs {
        args: vec![class.clone_with_heap(vm.heap)],
        kwargs: KwargsValues::Inline(vec![(
            vm.interns.intern_static(StaticStrings::IncludeExtras),
            Value::Bool(true),
        )]),
    };
    vm.evaluate_function(&get_type_hints, args)
}

/// The dict a `get_type_hints` result is.
pub(crate) fn dict_of<'a>(value: &Value, vm: &'a VM<'_>) -> &'a Dict {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Dict(dict) => dict,
            _ => unreachable!("get_type_hints returns a dict"),
        },
        _ => unreachable!("get_type_hints returns a dict"),
    }
}

/// pydantic's `display_as_type(hint)`, the text `repr(TypeAdapter(hint))` shows:
/// bare names for types and classes, `Union[int, NoneType]` for a union,
/// `tuple[]` for `tuple[()]`.
pub(crate) fn display_as_type(hint: &Value, vm: &mut VM<'_>) -> String {
    match hint {
        Value::None => "NoneType".to_owned(),
        Value::Ellipsis => "...".to_owned(),
        Value::Marker(Marker(name)) => <&'static str>::from(*name).to_owned(),
        Value::Builtin(Builtins::Type(ty)) => Type::from(*ty).dunder_name(vm.heap, vm.interns).into_owned(),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::GenericAlias(alias) => {
                let (origin, args) = (alias.origin(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let items = tuple_items(args, vm).ok().flatten().unwrap_or_default();
                defer_drop!(items, vm);
                let origin = Type::from(origin).dunder_name(vm.heap, vm.interns).into_owned();
                format!("{origin}[{}]", display_all(items, vm))
            }
            HeapData::Union(union) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let members = tuple_items(args, vm).ok().flatten().unwrap_or_default();
                defer_drop!(members, vm);
                format!("Union[{}]", display_all(members, vm))
            }
            HeapData::TypingAlias(alias) => {
                let (form, args) = (alias.form(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let items = tuple_items(args, vm).ok().flatten().unwrap_or_default();
                defer_drop!(items, vm);
                match form {
                    TypingForm::Literal => {
                        let reprs: Vec<String> = items.iter().map(|item| repr_string(item, vm)).collect();
                        format!("Literal[{}]", reprs.join(", "))
                    }
                    TypingForm::Annotated => format!("Annotated[{}]", display_all(items, vm)),
                }
            }
            HeapData::Class(class) => class.name().as_str(vm.interns).to_owned(),
            HeapData::NamedTupleClass(class) => class.name(vm.interns).to_owned(),
            _ => repr_string(hint, vm),
        },
        other => repr_string(other, vm),
    }
}

/// `display_as_type` of each hint, comma-separated.
fn display_all(hints: &[Value], vm: &mut VM<'_>) -> String {
    hints
        .iter()
        .map(|hint| display_as_type(hint, vm))
        .collect::<Vec<_>>()
        .join(", ")
}

/// pydantic's `PydanticSchemaGenerationError` for a hint with no validator.
fn schema_error(hint: &Value, vm: &mut VM<'_>) -> RunError {
    let repr = repr_string(hint, vm);
    SimpleException::new_msg(
        ExcType::PydanticSchemaGenerationError,
        format!(
            "Unable to generate pydantic-core schema for {repr}. Set `arbitrary_types_allowed=True` in the model_config to ignore this error or implement `__get_pydantic_core_schema__` on your type to fully support it.\n\nIf you got this error by calling handler(<some type>) within `__get_pydantic_core_schema__` then you likely need to call `handler.generate_schema(<some type>)` since we do not call `__get_pydantic_core_schema__` on `<some type>` otherwise to avoid infinite recursion.\n\nFor further information visit {}u/schema-for-unknown-type",
            ERROR_URL_PREFIX.trim_end_matches("v/")
        ),
    )
    .into()
}
