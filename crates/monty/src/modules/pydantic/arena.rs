//! The validator arena a `TypeAdapter` runs: one flat `Vec<Validator>` built
//! from a type hint, with validators referring to each other by [`ValidatorId`].
//!
//! A recursive type (a dataclass whose field names the class again) is an id
//! pointing at an earlier slot; there are no definitions or late binding. The
//! arena is owned by its `TypeAdapter` heap object, so the `Value`s it holds
//! (literal choices, dataclass defaults, class objects) are reference-counted
//! and cycle-collected with it, and the whole arena persists in dumps.

use serde::{Deserialize, Serialize};

use crate::{
    heap::{ContainsHeap, DropWithContext, HeapId},
    intern::StringId,
    value::Value,
};

/// Index of a validator in its arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ValidatorId(u32);

impl ValidatorId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// One node of the validator tree. Containers hold the ids of their item
/// validators; the leaves carry the coercion rules for one type.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Validator {
    Any,
    None,
    Bool,
    Int,
    Float,
    Str,
    Bytes,
    DateTime,
    Date,
    Time,
    TimeDelta,
    Decimal,
    List(ValidatorId),
    Set(ValidatorId),
    FrozenSet(ValidatorId),
    /// `tuple[A, B]` has fixed `items`; for `tuple[A, ...]` the last item repeats.
    Tuple {
        items: Vec<ValidatorId>,
        variadic: bool,
    },
    Dict {
        key: ValidatorId,
        value: ValidatorId,
    },
    /// A smart-mode union: every member is tried and the most exact match wins.
    Union(Vec<ValidatorId>),
    /// `X | None`: `None` passes, anything else validates as `X`.
    Nullable(ValidatorId),
    Literal(LiteralValidator),
    Dataclass(DataclassValidator),
    NamedTuple(NamedTupleValidator),
}

/// `Literal[...]`: the accepted values, matched by value and type.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LiteralValidator {
    /// The choices in declaration order; OWNED references.
    pub(crate) choices: Vec<Value>,
    /// `'a' or 'b'`: the `expected` context of a `literal_error`.
    pub(crate) expected: String,
    /// `literal['a','b']`: the validator's name.
    pub(crate) name: String,
}

/// A stdlib `@dataclass`: its fields in signature order, validated from a dict
/// or passed through when the input is already an instance.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DataclassValidator {
    /// The class object; an OWNED reference, so the arena keeps the class alive.
    pub(crate) class: Value,
    /// The class name, which is also the validator's name.
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldValidator>,
    /// Whether the class defined `__post_init__`, called after the fields are set.
    pub(crate) post_init: bool,
}

/// One dataclass field.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FieldValidator {
    pub(crate) name: StringId,
    pub(crate) validator: ValidatorId,
    pub(crate) default: FieldDefault,
}

/// What a dataclass field gets when the input omits it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum FieldDefault {
    Required,
    /// A default value; an OWNED reference.
    Value(Value),
    /// A `default_factory` callable; an OWNED reference.
    Factory(Value),
}

/// A `collections.namedtuple` class: positional or keyword arguments bound
/// to its fields, as pydantic's `call[Point]` validator does.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct NamedTupleValidator {
    /// The class object; an OWNED reference.
    pub(crate) class: Value,
    pub(crate) name: String,
    pub(crate) field_names: Vec<String>,
    /// How many trailing fields have defaults; the class supplies the values.
    pub(crate) defaults: usize,
}

/// The validators of one `TypeAdapter`, with the root last built.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Arena {
    validators: Vec<Validator>,
}

impl Arena {
    pub(crate) fn get(&self, id: ValidatorId) -> &Validator {
        &self.validators[id.index()]
    }

    /// Appends `validator`, returning its id.
    pub(crate) fn push(&mut self, validator: Validator) -> ValidatorId {
        let id = ValidatorId(u32::try_from(self.validators.len()).expect("arena ids fit u32"));
        self.validators.push(validator);
        id
    }

    /// Reserves a slot a recursive type can refer back to while its fields are
    /// walked; [`fill`](Self::fill) replaces the placeholder.
    pub(crate) fn reserve(&mut self) -> ValidatorId {
        self.push(Validator::Any)
    }

    pub(crate) fn fill(&mut self, id: ValidatorId, validator: Validator) {
        self.validators[id.index()] = validator;
    }

    /// pydantic's name for a validator: the type for a leaf, `list[int]` and
    /// the like for containers, the class name for a dataclass. Names are the
    /// `ValidationError` title and the location of a union member's errors.
    pub(crate) fn name(&self, id: ValidatorId) -> String {
        match self.get(id) {
            Validator::Any => "any".to_owned(),
            Validator::None => "none".to_owned(),
            Validator::Bool => "bool".to_owned(),
            Validator::Int => "int".to_owned(),
            Validator::Float => "float".to_owned(),
            Validator::Str => "str".to_owned(),
            Validator::Bytes => "bytes".to_owned(),
            Validator::DateTime => "datetime".to_owned(),
            Validator::Date => "date".to_owned(),
            Validator::Time => "time".to_owned(),
            Validator::TimeDelta => "timedelta".to_owned(),
            Validator::Decimal => "decimal".to_owned(),
            Validator::List(item) => format!("list[{}]", self.name(*item)),
            Validator::Set(item) => format!("set[{}]", self.name(*item)),
            Validator::FrozenSet(item) => format!("frozenset[{}]", self.name(*item)),
            Validator::Tuple { items, variadic } => {
                let mut names: Vec<String> = items.iter().map(|item| self.name(*item)).collect();
                if *variadic {
                    names.push("...".to_owned());
                }
                format!("tuple[{}]", names.join(", "))
            }
            Validator::Dict { key, value } => {
                format!("dict[{},{}]", self.name(*key), self.name(*value))
            }
            Validator::Union(choices) => {
                let names: Vec<String> = choices.iter().map(|choice| self.name(*choice)).collect();
                format!("union[{}]", names.join(","))
            }
            Validator::Nullable(inner) => format!("nullable[{}]", self.name(*inner)),
            Validator::Literal(literal) => literal.name.clone(),
            Validator::Dataclass(dataclass) => dataclass.name.clone(),
            Validator::NamedTuple(named_tuple) => format!("call[{}]", named_tuple.name),
        }
    }

    /// Invokes `on_child` for every heap id the arena owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for validator in &self.validators {
            validator.for_each_value(&mut |value| {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            });
        }
    }

    /// Hands every owned value to the heap's cleanup stack.
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for validator in &mut self.validators {
            validator.for_each_value_mut(&mut |value| value.py_dec_ref_ids(stack));
        }
    }
}

impl Validator {
    /// Visits every `Value` the validator owns, mutably.
    fn for_each_value_mut(&mut self, visit: &mut impl FnMut(&mut Value)) {
        match self {
            Self::Literal(literal) => literal.choices.iter_mut().for_each(&mut *visit),
            Self::Dataclass(dataclass) => {
                visit(&mut dataclass.class);
                for field in &mut dataclass.fields {
                    if let FieldDefault::Value(value) | FieldDefault::Factory(value) = &mut field.default {
                        visit(value);
                    }
                }
            }
            Self::NamedTuple(named_tuple) => visit(&mut named_tuple.class),
            _ => {}
        }
    }

    /// Visits every `Value` the validator owns.
    fn for_each_value(&self, visit: &mut impl FnMut(&Value)) {
        match self {
            Self::Literal(literal) => literal.choices.iter().for_each(&mut *visit),
            Self::Dataclass(dataclass) => {
                visit(&dataclass.class);
                for field in &dataclass.fields {
                    if let FieldDefault::Value(value) | FieldDefault::Factory(value) = &field.default {
                        visit(value);
                    }
                }
            }
            Self::NamedTuple(named_tuple) => visit(&named_tuple.class),
            _ => {}
        }
    }
}

/// Releases a validator abandoned before it reached the heap; one stored in a
/// `TypeAdapter` is freed through [`Arena::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for Validator {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Literal(literal) => literal.choices.drop_with(ctx),
            Self::Dataclass(dataclass) => {
                dataclass.class.drop_with(ctx);
                dataclass.fields.drop_with(ctx);
            }
            Self::NamedTuple(named_tuple) => named_tuple.class.drop_with(ctx),
            _ => {}
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FieldDefault {
    fn drop_with(self, ctx: &mut C) {
        if let Self::Value(value) | Self::Factory(value) = self {
            value.drop_with(ctx);
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FieldValidator {
    fn drop_with(self, ctx: &mut C) {
        self.default.drop_with(ctx);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Arena {
    fn drop_with(self, ctx: &mut C) {
        self.validators.drop_with(ctx);
    }
}
