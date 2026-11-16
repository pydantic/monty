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

use std::fmt::Write;

use ahash::AHashMap;

use crate::{
    args::{ArgValues, KwargsValues},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropGuard, HeapData, HeapId},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        dataclasses::is_dataclass_class,
        pydantic::{
            arena::{
                Arena, DataclassValidator, FieldDefault, FieldValidator, LiteralValidator, NamedTupleValidator,
                Validator, ValidatorId,
            },
            errors::{SCHEMA_GENERATION_CODE, dict_pairs, user_error},
            snapshot::{repr_string, type_name},
        },
        typing::TypingFunctions,
    },
    types::{
        Dict, LazyHeapSet, PyTrait, Type, TypeRef,
        generic_alias::AliasOrigin,
        instance::{class_dunder, namespace_member},
        union::tuple_items,
    },
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
        steps: 0,
    };
    builder.walk(hint, vm)
}

/// The walk's state: the arena under construction and the dataclasses already placed in it.
struct Builder<'a> {
    arena: &'a mut Arena,
    classes: AHashMap<HeapId, ValidatorId>,
    /// Hints walked so far, keying the limit poll: a generic alias that names
    /// another twice (`tuple[h, h]`) is walked once per reference.
    steps: usize,
}

impl Builder<'_> {
    fn walk(&mut self, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        self.steps = self.steps.wrapping_add(1);
        vm.heap.tracker.check_memory_time_every(self.steps)?;
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        match hint {
            Value::None => self.arena.push(Validator::None, vm),
            Value::Marker(Marker(StaticStrings::Any)) => self.arena.push(Validator::Any, vm),
            // The bare `typing.List` and friends: the builtin with `Any` items.
            Value::Marker(form) if let Some(ty) = typing_container(*form) => self.builtin(ty, hint, vm),
            Value::Builtin(Builtins::Type(ty)) => self.builtin(*ty, hint, vm),
            Value::Ref(id) => self.heap_hint(*id, hint, vm),
            _ => Err(schema_error(hint, vm)),
        }
    }

    /// A bare builtin type: the scalar validators, and the containers with `Any` items.
    fn builtin(&mut self, ty: Type, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let validator = match ty {
            Type::NoneType => Validator::None,
            Type::Object => Validator::Any,
            Type::Bool => Validator::Bool,
            Type::Int => Validator::Int,
            Type::Float => Validator::Float,
            Type::Str => Validator::Str,
            Type::Bytes => Validator::Bytes,
            Type::DateTime => Validator::DateTime,
            Type::Date => Validator::Date,
            Type::Time => Validator::Time,
            Type::TimeDelta => Validator::TimeDelta,
            Type::Decimal => Validator::Decimal,
            Type::List => Validator::List(self.arena.push(Validator::Any, vm)?),
            Type::Set => Validator::Set(self.arena.push(Validator::Any, vm)?),
            Type::FrozenSet => Validator::FrozenSet(self.arena.push(Validator::Any, vm)?),
            Type::Tuple => Validator::Tuple {
                items: vec![self.arena.push(Validator::Any, vm)?],
                variadic: true,
            },
            Type::Dict => Validator::Dict {
                key: self.arena.push(Validator::Any, vm)?,
                value: self.arena.push(Validator::Any, vm)?,
            },
            _ => return Err(schema_error(hint, vm)),
        };
        self.arena.push(validator, vm)
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
                match origin {
                    AliasOrigin::Builtin(origin) => self.generic(origin, args, hint, vm),
                    AliasOrigin::Special(Marker(StaticStrings::Literal)) => self.literal(args, hint, vm),
                    // The metadata is read once `Field()` exists; the type is validated as itself.
                    AliasOrigin::Special(Marker(StaticStrings::Annotated)) => self.walk(&args[0], vm),
                    AliasOrigin::Special(form) => match typing_container(form) {
                        Some(origin) => self.generic(origin, args, hint, vm),
                        None => Err(schema_error(hint, vm)),
                    },
                }
            }
            HeapData::Union(union) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let members = tuple_items(args, vm)?.expect("Union::args is a tuple");
                defer_drop!(members, vm);
                self.union(members, vm)
            }
            HeapData::Class(_) if is_dataclass_class(id, vm) => self.dataclass(id, hint, vm),
            HeapData::NamedTupleClass(class) => {
                let validator = NamedTupleValidator {
                    class: hint.clone_with_heap(vm.heap),
                    name: class.name(vm.interns).to_owned(),
                    field_names: class
                        .field_names()
                        .iter()
                        .enumerate()
                        .map(|(index, name)| {
                            vm.heap.tracker.check_memory_time_every(index)?;
                            Ok(name.as_str(vm.interns).to_owned())
                        })
                        .collect::<RunResult<_>>()?,
                    defaults: class.defaults().len(),
                };
                self.arena.push(Validator::NamedTuple(validator), vm)
            }
            _ => Err(schema_error(hint, vm)),
        }
    }

    /// `list[T]`, `set[T]`, `frozenset[T]`, `tuple[...]`, `dict[K, V]` and
    /// their `typing` aliases. As in pydantic, surplus arguments are ignored,
    /// `dict` needs its two, and a variadic tuple has one item before its `...`.
    fn generic(&mut self, origin: Type, args: &[Value], hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let validator = match (origin, args) {
            (Type::List, [item, ..]) => Validator::List(self.walk(item, vm)?),
            (Type::Set, [item, ..]) => Validator::Set(self.walk(item, vm)?),
            (Type::FrozenSet, [item, ..]) => Validator::FrozenSet(self.walk(item, vm)?),
            (Type::Dict, [key, value, ..]) => Validator::Dict {
                key: self.walk(key, vm)?,
                value: self.walk(value, vm)?,
            },
            (Type::Dict, args) => {
                return Err(ExcType::type_error(format!(
                    "Expected two type arguments for <class 'dict'>, got {}",
                    args.len()
                )));
            }
            (Type::Tuple, [item, Value::Ellipsis]) => Validator::Tuple {
                items: vec![self.walk(item, vm)?],
                variadic: true,
            },
            (Type::Tuple, items) if items.last().is_some_and(|last| matches!(last, Value::Ellipsis)) => {
                return Err(ExcType::value_error("Variable tuples can only have one type"));
            }
            (Type::Tuple, items) => {
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
        self.arena.push(validator, vm)
    }

    /// `X | Y`: a `nullable` around the other members when `None` is one of
    /// them, a smart union of the rest.
    fn union(&mut self, members: &[Value], vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        let is_none = |member: &Value| matches!(member, Value::None | Value::Builtin(Builtins::Type(Type::NoneType)));
        let nullable = members.iter().any(is_none);
        let mut choices = Vec::new();
        for member in members.iter().filter(|member| !is_none(member)) {
            choices.push(self.walk(member, vm)?);
        }
        let inner = match choices.len() {
            0 => self.arena.push(Validator::None, vm)?,
            1 => choices[0],
            _ => self.arena.push(Validator::Union(choices), vm)?,
        };
        if nullable && !matches!(self.arena.get(inner), Validator::None) {
            self.arena.push(Validator::Nullable(inner), vm)
        } else {
            Ok(inner)
        }
    }

    /// `Literal[...]`: the choices as given, named by their reprs. pydantic's
    /// schema asserts there is at least one.
    fn literal(&mut self, choices: &[Value], hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        if choices.is_empty() {
            let repr = repr_string(hint, vm);
            return Err(SimpleException::new_msg(
                ExcType::AssertionError,
                format!("literal \"expected\" cannot be empty, obj={repr}"),
            )
            .into());
        }
        let mut reprs = Vec::with_capacity(choices.len());
        for (index, choice) in choices.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(index)?;
            reprs.push(repr_string(choice, vm));
        }
        let expected = match reprs.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            Some((last, _)) => last.clone(),
            None => unreachable!("an empty `Literal` was refused above"),
        };
        let validator = LiteralValidator {
            choices: choices.iter().map(|choice| choice.clone_with_heap(vm.heap)).collect(),
            expected,
            name: format!("literal[{}]", reprs.join(",")),
        };
        self.arena.push(Validator::Literal(validator), vm)
    }

    /// A dataclass, memoised by class: the slot is reserved before the fields
    /// are walked so a self-referential field finds it.
    ///
    /// `__dataclass_fields__` and `__post_init__` are looked up through the
    /// bases, as pydantic reads them with `getattr`: a plain subclass of a
    /// dataclass validates with its base's fields, and a mixin's hook runs.
    /// The field hints are those of the class declaring the fields.
    fn dataclass(&mut self, class_id: HeapId, hint: &Value, vm: &mut VM<'_>) -> RunResult<ValidatorId> {
        if let Some(id) = self.classes.get(&class_id) {
            return Ok(*id);
        }
        let id = self.arena.reserve(vm)?;
        self.classes.insert(class_id, id);
        let hints = type_hints(fields_owner(class_id, vm), vm)?;
        defer_drop!(hints, vm);
        let HeapData::Class(class) = vm.heap.get(class_id) else {
            unreachable!("is_dataclass_class checked a class")
        };
        let name = class.name().as_str(vm.interns).to_owned();
        let post_init = class_dunder(class_id, StaticStrings::PostInit.into(), vm).is_some();
        let Some(fields_dict) = class_dunder(class_id, StaticStrings::DataclassFields.into(), vm)
            .map(|fields| fields.clone_with_heap(vm.heap))
        else {
            unreachable!("is_dataclass_class found __dataclass_fields__")
        };
        defer_drop!(fields_dict, vm);
        let Some(pairs) = dict_pairs(fields_dict, vm) else {
            return Err(schema_error(hint, vm));
        };
        defer_drop!(pairs, vm);
        let mut fields = DropGuard::new(Vec::with_capacity(pairs.len()), vm);
        for (key, field) in pairs {
            let (fields, vm) = fields.as_parts_mut();
            // Field names come from the class's annotations, which are always interned.
            let Value::InternString(field_name) = key else {
                return Err(schema_error(hint, vm));
            };
            let field_hint = dict_of(hints, vm)
                .get_by_str(vm.interns.get_str(*field_name), vm.heap, vm.interns)
                .map(|value| value.clone_with_heap(vm.heap));
            let validator = match field_hint {
                Some(field_hint) => {
                    defer_drop!(field_hint, vm);
                    self.walk(field_hint, vm)?
                }
                None => self.arena.push(Validator::Any, vm)?,
            };
            fields.push(FieldValidator {
                name: *field_name,
                validator,
                default: field_default(field, vm),
            });
        }
        let (fields, vm) = fields.into_parts();
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

/// The builtin container a `typing` alias stands for, as pydantic maps them:
/// `List`, `Set`, `FrozenSet`, `Tuple`, `Dict`, and `Mapping`, whose schema is
/// a plain `dict` one.
fn typing_container(form: Marker) -> Option<Type> {
    match form.0 {
        StaticStrings::ListType => Some(Type::List),
        StaticStrings::SetType => Some(Type::Set),
        StaticStrings::FrozenSet => Some(Type::FrozenSet),
        StaticStrings::TupleType => Some(Type::Tuple),
        StaticStrings::DictType | StaticStrings::Mapping => Some(Type::Dict),
        _ => None,
    }
}

/// The class whose own namespace holds the `__dataclass_fields__` that
/// `class_id` resolves: itself, or the first dataclass among its bases.
fn fields_owner(class_id: HeapId, vm: &VM<'_>) -> HeapId {
    let name: &str = StaticStrings::DataclassFields.into();
    let declares = |id: HeapId| namespace_member(id, name, vm.heap, vm.interns).is_some();
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("is_dataclass_class checked a class")
    };
    if declares(class_id) {
        class_id
    } else {
        class
            .ancestors()
            .iter()
            .find_map(|ancestor| match ancestor {
                TypeRef::Class(id) if declares(*id) => Some(*id),
                _ => None,
            })
            .expect("is_dataclass_class found __dataclass_fields__")
    }
}

/// What a dataclass field gets when the input omits it, read from its `Field`.
fn field_default(field: &Value, vm: &VM<'_>) -> FieldDefault {
    match field {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::DataclassField(field) => match (field.default(), field.default_factory()) {
                (Some(value), _) => FieldDefault::Value(value.clone_with_heap(vm.heap)),
                (None, Some(factory)) => FieldDefault::Factory(factory.clone_with_heap(vm.heap)),
                (None, None) => FieldDefault::Required,
            },
            _ => FieldDefault::Required,
        },
        _ => FieldDefault::Required,
    }
}

/// `typing.get_type_hints(cls, include_extras=True)` for the class `class_id`
/// (borrowed), run to completion: the evaluated field hints keyed by name.
fn type_hints(class_id: HeapId, vm: &mut VM<'_>) -> RunResult<Value> {
    let get_type_hints = Value::ModuleFunction(ModuleFunctions::Typing(TypingFunctions::GetTypeHints));
    vm.heap.inc_ref(class_id);
    let args = ArgValues::ArgsKargs {
        args: vec![Value::Ref(class_id)],
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
/// `tuple[]` for `tuple[()]`, a function's name, and the class of any other
/// value (`Annotated[int, 'x']` shows `Annotated[int, str]`).
///
/// Written straight into `f`, polling the limits per hint and guarding the
/// recursion, since metadata can nest arbitrarily deep or name an alias twice.
pub(crate) fn display_as_type(
    hint: &Value,
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    let mut steps = 0;
    display(hint, f, &mut steps, vm, heap_ids)
}

/// One level of [`display_as_type`]; `steps` counts the hints written so far.
fn display(
    hint: &Value,
    f: &mut impl Write,
    steps: &mut usize,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    *steps = steps.wrapping_add(1);
    vm.heap.tracker.check_memory_time_every(*steps)?;
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    match hint {
        Value::Ellipsis => f.write_str("...")?,
        Value::Marker(Marker(name)) => f.write_str(name.into())?,
        Value::Builtin(Builtins::Type(ty)) => f.write_str(ty.dunder_name())?,
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::GenericAlias(alias) => {
                let (origin, args) = (alias.origin(), alias.args(vm.heap));
                defer_drop!(args, vm);
                let items = tuple_items(args, vm)?.expect("GenericAlias::args is a tuple");
                defer_drop!(items, vm);
                match origin {
                    AliasOrigin::Builtin(origin) => write!(f, "{}[", origin.dunder_name())?,
                    AliasOrigin::Special(Marker(form)) => write!(f, "{}[", <&'static str>::from(form))?,
                }
                let literal = origin == AliasOrigin::Special(Marker(StaticStrings::Literal));
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    if literal {
                        item.py_repr_fmt(f, vm, heap_ids)?;
                    } else {
                        display(item, f, steps, vm, heap_ids)?;
                    }
                }
                f.write_char(']')?;
            }
            HeapData::Union(union) => {
                let args = union.args(vm.heap);
                defer_drop!(args, vm);
                let members = tuple_items(args, vm)?.expect("Union::args is a tuple");
                defer_drop!(members, vm);
                f.write_str("Union[")?;
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    display(member, f, steps, vm, heap_ids)?;
                }
                f.write_char(']')?;
            }
            HeapData::Class(class) => f.write_str(class.name().as_str(vm.interns))?,
            HeapData::NamedTupleClass(class) => f.write_str(class.name(vm.interns))?,
            _ => display_value(hint, f, vm)?,
        },
        _ => display_value(hint, f, vm)?,
    }
    Ok(())
}

/// A hint that is a plain value: a function by its name, anything else by its
/// class, as pydantic replaces a non-type with `obj.__class__`.
fn display_value(value: &Value, f: &mut impl Write, vm: &VM<'_>) -> RunResult<()> {
    let function = match value {
        Value::DefFunction(id) => Some(*id),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Closure(closure) => Some(closure.func_id),
            HeapData::FunctionDefaults(defaults) => Some(defaults.func_id),
            _ => None,
        },
        _ => None,
    };
    match (function, value) {
        (Some(id), _) => f.write_str(vm.interns.get_str(vm.interns.get_function(id).name.name_id))?,
        (None, Value::Builtin(Builtins::Function(function))) => write!(f, "{function}")?,
        (None, Value::ModuleFunction(function)) => write!(f, "{function}")?,
        (None, _) => f.write_str(&type_name(value, vm))?,
    }
    Ok(())
}

/// pydantic's `PydanticSchemaGenerationError` for a hint with no validator.
fn schema_error(hint: &Value, vm: &mut VM<'_>) -> RunError {
    let repr = repr_string(hint, vm);
    let message = format!(
        "Unable to generate pydantic-core schema for {repr}. Set `arbitrary_types_allowed=True` in the model_config to ignore this error or implement `__get_pydantic_core_schema__` on your type to fully support it.\n\nIf you got this error by calling handler(<some type>) within `__get_pydantic_core_schema__` then you likely need to call `handler.generate_schema(<some type>)` since we do not call `__get_pydantic_core_schema__` on `<some type>` otherwise to avoid infinite recursion."
    );
    user_error(
        ExcType::PydanticSchemaGenerationError,
        message,
        Some(SCHEMA_GENERATION_CODE.to_owned()),
    )
    .into()
}
