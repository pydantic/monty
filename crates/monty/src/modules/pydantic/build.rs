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
                Arena, DataclassValidator, FieldDefault, FieldValidator, LiteralValidator, NamedTupleValidator,
                Validator, ValidatorId,
            },
            errors::{ERROR_URL_PREFIX, dict_pairs},
            snapshot::repr_string,
        },
        typing::TypingFunctions,
    },
    types::{BuiltinType, Dict, Type, typing_alias::TypingForm, union::tuple_items},
    value::{Marker, Value},
};

/// Builds the validator for `hint` into `arena`, returning its id.
///
/// A hint that cannot be validated raises `PydanticSchemaGenerationError`; the
/// caller discards the arena then.
pub(crate) fn build(hint: &Value, arena: &mut Arena, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
    let mut builder = Builder {
        arena,
        classes: AHashMap::new(),
    };
    builder.walk(hint, vm)
}

/// The walk's state: the arena under construction and the dataclasses already placed in it.
struct Builder<'a> {
    arena: &'a mut Arena,
    classes: AHashMap<HeapId, ValidatorId>,
}

impl Builder<'_> {
    fn walk(&mut self, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        match hint {
            Value::None => Ok(self.arena.push(Validator::None)),
            Value::Marker(Marker(StaticStrings::Any)) => Ok(self.arena.push(Validator::Any)),
            Value::Builtin(Builtins::Type(ty)) => self.builtin(*ty, hint, vm),
            Value::Ref(id) => self.heap_hint(*id, hint, vm),
            _ => Err(schema_error(hint, vm)),
        }
    }

    /// A bare builtin type: the scalar validators, and the containers with `Any` items.
    fn builtin(&mut self, ty: BuiltinType, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
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
        Ok(self.arena.push(validator))
    }

    /// A heap-allocated hint: a generic alias, a union, a `Literal` or
    /// `Annotated`, a dataclass, a namedtuple class or another `TypeAdapter`.
    fn heap_hint(&mut self, id: HeapId, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        match vm.heap.get(id) {
            HeapData::GenericAlias(alias) => {
                let (origin, args) = (alias.origin(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let args = tuple_items(args, vm)?.expect("GenericAlias::args is a tuple");
                defer_drop!(args, vm);
                self.generic(origin, args, hint, vm)
            }
            HeapData::Union(union) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let members = tuple_items(args, vm)?.expect("Union::args is a tuple");
                defer_drop!(members, vm);
                self.union(members, vm)
            }
            HeapData::TypingAlias(alias) => {
                let (form, args) = (alias.form(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let args = tuple_items(args, vm)?.expect("TypingAlias::args is a tuple");
                defer_drop!(args, vm);
                match form {
                    TypingForm::Literal => Ok(self.literal(args, vm)),
                    // The metadata is read once `Field()` exists; the type is validated as itself.
                    TypingForm::Annotated => self.walk(&args[0], vm),
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
            (BuiltinType::List, [item, ..]) => Validator::List(self.walk(item, vm)?),
            (BuiltinType::Set, [item, ..]) => Validator::Set(self.walk(item, vm)?),
            (BuiltinType::FrozenSet, [item, ..]) => Validator::FrozenSet(self.walk(item, vm)?),
            (BuiltinType::Dict, [key, value, ..]) => Validator::Dict {
                key: self.walk(key, vm)?,
                value: self.walk(value, vm)?,
            },
            (BuiltinType::Dict, args) => {
                return Err(ExcType::type_error(format!(
                    "Expected two type arguments for <class 'dict'>, got {}",
                    args.len()
                )));
            }
            (BuiltinType::Tuple, [item, Value::Ellipsis]) => Validator::Tuple {
                items: vec![self.walk(item, vm)?],
                variadic: true,
            },
            (BuiltinType::Tuple, items) if items.last().is_some_and(|last| matches!(last, Value::Ellipsis)) => {
                return Err(ExcType::value_error("Variable tuples can only have one type"));
            }
            (BuiltinType::Tuple, items) => {
                let mut ids = Vec::with_capacity(items.len());
                for item in items {
                    ids.push(self.walk(item, vm)?);
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
    /// them, a smart union of the rest.
    fn union(&mut self, members: &[Value], vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let is_none = |member: &Value| {
            matches!(
                member,
                Value::None | Value::Builtin(Builtins::Type(BuiltinType::NoneType))
            )
        };
        let nullable = members.iter().any(is_none);
        let mut choices = Vec::new();
        for member in members.iter().filter(|member| !is_none(member)) {
            choices.push(self.walk(member, vm)?);
        }
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
            let default = match (default, factory) {
                (Some(value), _) => FieldDefault::Value(value),
                (None, Some(factory)) => FieldDefault::Factory(factory),
                (None, None) => FieldDefault::Required,
            };
            let hint_value = dict_of(hints, vm)
                .get_by_str(vm.interns.get_str(field_name), vm.heap, vm.interns)
                .map(|value| value.clone_with_heap(vm.heap));
            let validator = match hint_value {
                Some(field_hint) => {
                    defer_drop!(field_hint, vm);
                    match self.walk(field_hint, vm) {
                        Ok(id) => id,
                        Err(err) => {
                            default.drop_with(vm);
                            fields.drop_with(vm);
                            return Err(err);
                        }
                    }
                }
                None => self.arena.push(Validator::Any),
            };
            fields.push(FieldValidator {
                name: field_name,
                validator,
                default,
            });
        }
        self.arena.fill(
            id,
            Validator::Dataclass(DataclassValidator {
                class: hint.clone_with_heap(vm.heap),
                name,
                fields,
                post_init,
            }),
        );
        Ok(id)
    }
}

/// `typing.get_type_hints(cls, include_extras=True)`, run to completion: the
/// evaluated field hints keyed by name.
fn type_hints(class: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
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
fn dict_of<'a>(value: &Value, vm: &'a VM<'_>) -> &'a Dict {
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
