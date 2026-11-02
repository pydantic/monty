//! The validator arena a `TypeAdapter` runs: one flat `Vec<Validator>` built
//! from a type hint, with validators referring to each other by [`ValidatorId`].
//!
//! A recursive type (a dataclass whose field names the class again) is an id
//! pointing at an earlier slot; there are no definitions or late binding. The
//! arena is owned by its `TypeAdapter` heap object, so the `Value`s it holds
//! (literal choices, dataclass defaults, class objects) are reference-counted
//! and cycle-collected with it, and the whole arena persists in dumps.

use std::cell::OnceCell;

use serde::{Deserialize, Serialize};

use crate::{
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, HeapId},
    intern::{Interns, StringId},
    types::re_pattern::RePattern,
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
    /// `Field(strict=)` or a config's `strict` around `inner`: with `scope`
    /// the whole subtree (a dataclass config), otherwise the next validator
    /// that reads strictness (one field).
    Strict {
        inner: ValidatorId,
        strict: bool,
        scope: bool,
    },
    /// `Field(...)` constraints checked on `inner`'s output.
    Constrained {
        inner: ValidatorId,
        constraints: Box<Constraints>,
    },
    /// `Field(...)` settings that only `json_schema()` reads; validation passes through.
    Meta {
        inner: ValidatorId,
        meta: Box<SchemaMeta>,
    },
}

/// The `Field(...)` settings a JSON schema shows; every `Value` is an OWNED reference.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SchemaMeta {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) examples: Option<Value>,
    pub(crate) json_schema_extra: Option<Value>,
    /// `Field(default=)`, shown as the schema's `default`.
    pub(crate) default: Option<Value>,
}

impl SchemaMeta {
    /// Whether anything is set.
    pub(crate) fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.examples.is_none()
            && self.json_schema_extra.is_none()
            && self.default.is_none()
    }

    fn values(&self) -> impl Iterator<Item = &Value> {
        [&self.examples, &self.json_schema_extra, &self.default]
            .into_iter()
            .flatten()
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        [&mut self.examples, &mut self.json_schema_extra, &mut self.default]
            .into_iter()
            .flatten()
    }
}

impl<C: ContainsHeap> DropWithContext<C> for SchemaMeta {
    fn drop_with(self, ctx: &mut C) {
        for value in [self.examples, self.json_schema_extra, self.default]
            .into_iter()
            .flatten()
        {
            value.drop_with(ctx);
        }
    }
}

/// The constraints a `Field(...)` or a config puts on one validator, grouped by
/// the kind of value they apply to; a constraint pydantic ignores for the type
/// is dropped at build time, as pydantic drops it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Constraints {
    /// `gt`, `ge`, `lt`, `le` and `multiple_of` on an `int`, `float` or `Decimal`.
    Number(NumberConstraints),
    /// Length, pattern and case rules on a `str`.
    Str(Box<StrConstraints>),
    /// Length bounds on `bytes`.
    Bytes {
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
    /// Length bounds on a container, checked after its items validate.
    Length {
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
}

/// Numeric bounds, each an OWNED value already coerced to the validator's type,
/// so a bound compares as pydantic-core's does and appears in `ctx` as pydantic shows it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct NumberConstraints {
    pub(crate) multiple_of: Option<Value>,
    pub(crate) le: Option<Value>,
    pub(crate) lt: Option<Value>,
    pub(crate) ge: Option<Value>,
    pub(crate) gt: Option<Value>,
}

impl NumberConstraints {
    /// The bounds that are set.
    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        [&self.multiple_of, &self.le, &self.lt, &self.ge, &self.gt]
            .into_iter()
            .flatten()
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        [
            &mut self.multiple_of,
            &mut self.le,
            &mut self.lt,
            &mut self.ge,
            &mut self.gt,
        ]
        .into_iter()
        .flatten()
    }
}

/// String rules in the order pydantic-core applies them: numbers coerced to
/// text, whitespace stripped, length checked, the pattern searched, then case changed.
#[derive(Debug, Default, Serialize, Deserialize)]
#[expect(clippy::struct_excessive_bools, reason = "one bool per string rule")]
pub(crate) struct StrConstraints {
    pub(crate) coerce_numbers_to_str: bool,
    pub(crate) strip_whitespace: bool,
    pub(crate) to_lower: bool,
    pub(crate) to_upper: bool,
    pub(crate) min_length: Option<usize>,
    pub(crate) max_length: Option<usize>,
    pub(crate) pattern: Option<Pattern>,
}

/// A `pattern=` regex, compiled on first use and again after a dump is loaded;
/// the source is what the arena persists and the error reports.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Pattern {
    pub(crate) source: String,
    #[serde(skip)]
    compiled: OnceCell<RePattern>,
}

impl Pattern {
    /// Compiles `source` now, so an invalid pattern fails while the adapter is built.
    pub(crate) fn new(source: String) -> RunResult<Self> {
        let compiled = RePattern::compile(source.clone(), 0)?;
        Ok(Self {
            source,
            compiled: OnceCell::from(compiled),
        })
    }

    /// Whether the pattern matches anywhere in `text`, as pydantic-core's `search` does.
    pub(crate) fn is_match(&self, text: &str) -> RunResult<bool> {
        if self.compiled.get().is_none() {
            let compiled = RePattern::compile(self.source.clone(), 0)?;
            self.compiled.get_or_init(|| compiled);
        }
        self.compiled.get().expect("set above").is_match(text)
    }
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
    /// `__pydantic_config__['populate_by_name']`: an aliased field is also read by its name.
    #[serde(default)]
    pub(crate) populate_by_name: bool,
    /// `__pydantic_config__['extra'] == 'forbid'`: an unknown key is an error.
    #[serde(default)]
    pub(crate) extra_forbid: bool,
    /// `__pydantic_config__['title']`: the JSON schema's title in place of the class name.
    #[serde(default)]
    pub(crate) title: Option<String>,
}

/// One dataclass field.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FieldValidator {
    pub(crate) name: StringId,
    pub(crate) validator: ValidatorId,
    pub(crate) default: FieldDefault,
    /// `Field(alias=)` or `validation_alias=`: the input key looked up instead
    /// of the name, and the location errors report. Text, as the VM cannot
    /// intern a string while it runs.
    #[serde(default)]
    pub(crate) alias: Option<String>,
    /// `Field(serialization_alias=)` or `alias=`: the key `dump_*(by_alias=True)` writes.
    #[serde(default)]
    pub(crate) serialization_alias: Option<String>,
    /// `Field(exclude=True)`: left out of `dump_*`.
    #[serde(default)]
    pub(crate) exclude: bool,
}

impl FieldValidator {
    /// The input key the field is read from: its alias, else its name.
    pub(crate) fn lookup_name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.alias.as_deref().unwrap_or_else(|| interns.get_str(self.name))
    }
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
            Validator::Strict { inner, .. } | Validator::Meta { inner, .. } => self.name(*inner),
            // pydantic-core names a constrained scalar; a bounded container keeps its name.
            Validator::Constrained { inner, constraints } => match (self.get(*inner), constraints.as_ref()) {
                (Validator::Decimal, _) | (_, Constraints::Length { .. }) => self.name(*inner),
                _ => format!("constrained-{}", self.name(*inner)),
            },
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
            Self::Constrained { constraints, .. } => {
                if let Constraints::Number(number) = constraints.as_mut() {
                    number.values_mut().for_each(&mut *visit);
                }
            }
            Self::Meta { meta, .. } => meta.values_mut().for_each(&mut *visit),
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
            Self::Constrained { constraints, .. } => {
                if let Constraints::Number(number) = constraints.as_ref() {
                    number.values().for_each(&mut *visit);
                }
            }
            Self::Meta { meta, .. } => meta.values().for_each(&mut *visit),
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
            Self::Constrained { constraints, .. } => {
                if let Constraints::Number(number) = *constraints {
                    number.drop_with(ctx);
                }
            }
            Self::Meta { meta, .. } => meta.drop_with(ctx),
            _ => {}
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for NumberConstraints {
    fn drop_with(self, ctx: &mut C) {
        for value in [self.multiple_of, self.le, self.lt, self.ge, self.gt]
            .into_iter()
            .flatten()
        {
            value.drop_with(ctx);
        }
    }
}

impl FieldDefault {
    /// A copy holding its own reference.
    pub(crate) fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
        match self {
            Self::Required => Self::Required,
            Self::Value(value) => Self::Value(value.clone_with_heap(heap)),
            Self::Factory(factory) => Self::Factory(factory.clone_with_heap(heap)),
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
