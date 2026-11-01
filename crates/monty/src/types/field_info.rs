//! `pydantic.fields.FieldInfo`, what `pydantic.Field(...)` returns: the per-field
//! settings the arena builder reads from `Annotated` metadata and from a
//! dataclass field's default.
//!
//! A plain record rather than pydantic's merging `FieldInfo`: the builder keeps
//! the hint itself, so there is no `annotation` here, and only the arguments
//! `limitations/pydantic.md` lists are stored. Attribute reads cover the stored
//! settings; `default` raises when unset, since Monty has no `PydanticUndefined`.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    modules::pydantic::{build::display_as_type, snapshot::repr_string},
    types::{LazyHeapSet, PyTrait, Type, allocate_string},
    value::{EitherStr, Value},
};

/// The settings one `Field(...)` call gave; every `Value` is an OWNED reference.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct FieldInfo {
    /// `default=`; `None` is pydantic's `PydanticUndefined`, not a `None` default.
    pub(crate) default: Option<Value>,
    pub(crate) default_factory: Option<Value>,
    pub(crate) alias: Option<String>,
    pub(crate) validation_alias: Option<String>,
    pub(crate) serialization_alias: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) examples: Option<Value>,
    pub(crate) exclude: Option<bool>,
    pub(crate) frozen: Option<bool>,
    pub(crate) json_schema_extra: Option<Value>,
    pub(crate) strict: Option<bool>,
    pub(crate) coerce_numbers_to_str: Option<bool>,
    /// The bounds as given; the builder coerces each to the field's type.
    pub(crate) gt: Option<Value>,
    pub(crate) ge: Option<Value>,
    pub(crate) lt: Option<Value>,
    pub(crate) le: Option<Value>,
    pub(crate) multiple_of: Option<Value>,
    pub(crate) min_length: Option<usize>,
    pub(crate) max_length: Option<usize>,
    pub(crate) pattern: Option<String>,
}

impl FieldInfo {
    /// Whether the field has neither a default nor a factory, pydantic's `is_required()`.
    pub(crate) fn is_required(&self) -> bool {
        self.default.is_none() && self.default_factory.is_none()
    }

    /// The key a value is looked up by: the validation alias, else the alias.
    pub(crate) fn lookup_alias(&self) -> Option<&str> {
        self.validation_alias.as_deref().or(self.alias.as_deref())
    }

    /// The key a value is written under with `by_alias`: the serialization alias, else the alias.
    pub(crate) fn output_alias(&self) -> Option<&str> {
        self.serialization_alias.as_deref().or(self.alias.as_deref())
    }

    /// The owned values, for cloning and tracing.
    fn values(&self) -> impl Iterator<Item = &Value> {
        [
            &self.default,
            &self.default_factory,
            &self.examples,
            &self.json_schema_extra,
            &self.gt,
            &self.ge,
            &self.lt,
            &self.le,
            &self.multiple_of,
        ]
        .into_iter()
        .flatten()
    }

    /// A copy holding its own references, for the builder to read while it runs Python.
    /// A copy keeping only the value constraints: the strictness, defaults and
    /// schema metadata a `Field(...)` on a union applies to the union, not to
    /// each of its members.
    pub(crate) fn constraints_only(&self, heap: &impl ContainsHeap) -> Self {
        let clone = |value: &Option<Value>| value.as_ref().map(|value| value.clone_with_heap(heap));
        Self {
            gt: clone(&self.gt),
            ge: clone(&self.ge),
            lt: clone(&self.lt),
            le: clone(&self.le),
            multiple_of: clone(&self.multiple_of),
            min_length: self.min_length,
            max_length: self.max_length,
            pattern: self.pattern.clone(),
            coerce_numbers_to_str: self.coerce_numbers_to_str,
            ..Self::default()
        }
    }

    pub(crate) fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
        let clone = |value: &Option<Value>| value.as_ref().map(|value| value.clone_with_heap(heap));
        Self {
            default: clone(&self.default),
            default_factory: clone(&self.default_factory),
            alias: self.alias.clone(),
            validation_alias: self.validation_alias.clone(),
            serialization_alias: self.serialization_alias.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            examples: clone(&self.examples),
            exclude: self.exclude,
            frozen: self.frozen,
            json_schema_extra: clone(&self.json_schema_extra),
            strict: self.strict,
            coerce_numbers_to_str: self.coerce_numbers_to_str,
            gt: clone(&self.gt),
            ge: clone(&self.ge),
            lt: clone(&self.lt),
            le: clone(&self.le),
            multiple_of: clone(&self.multiple_of),
            min_length: self.min_length,
            max_length: self.max_length,
            pattern: self.pattern.clone(),
        }
    }

    /// This record with `other`'s set values on top, as pydantic merges the
    /// `Field()` of a dataclass default over those in the hint's `Annotated`.
    pub(crate) fn merged<C: ContainsHeap>(&self, other: &Self, ctx: &mut C) -> Self {
        let mut merged = self.clone_with_heap(ctx);
        for (mine, theirs) in [
            (&mut merged.default, &other.default),
            (&mut merged.default_factory, &other.default_factory),
            (&mut merged.examples, &other.examples),
            (&mut merged.json_schema_extra, &other.json_schema_extra),
            (&mut merged.gt, &other.gt),
            (&mut merged.ge, &other.ge),
            (&mut merged.lt, &other.lt),
            (&mut merged.le, &other.le),
            (&mut merged.multiple_of, &other.multiple_of),
        ] {
            if let Some(value) = theirs
                && let Some(old) = mine.replace(value.clone_with_heap(ctx))
            {
                old.drop_with(ctx);
            }
        }
        for (mine, theirs) in [
            (&mut merged.alias, &other.alias),
            (&mut merged.validation_alias, &other.validation_alias),
            (&mut merged.serialization_alias, &other.serialization_alias),
            (&mut merged.title, &other.title),
            (&mut merged.description, &other.description),
            (&mut merged.pattern, &other.pattern),
        ] {
            if theirs.is_some() {
                mine.clone_from(theirs);
            }
        }
        for (mine, theirs) in [
            (&mut merged.exclude, other.exclude),
            (&mut merged.frozen, other.frozen),
            (&mut merged.strict, other.strict),
            (&mut merged.coerce_numbers_to_str, other.coerce_numbers_to_str),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        for (mine, theirs) in [
            (&mut merged.min_length, other.min_length),
            (&mut merged.max_length, other.max_length),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        merged
    }

    /// The owned values, mutably, for releasing them.
    fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        [
            &mut self.default,
            &mut self.default_factory,
            &mut self.examples,
            &mut self.json_schema_extra,
            &mut self.gt,
            &mut self.ge,
            &mut self.lt,
            &mut self.le,
            &mut self.multiple_of,
        ]
        .into_iter()
        .flatten()
    }

    /// Every heap id this field info owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in self.values() {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// pydantic's repr: `required`, the settings that are set, then the
    /// constraints as `annotated_types` metadata.
    fn write_repr(&self, f: &mut impl Write, vm: &mut VM<'_>) -> RunResult<()> {
        write!(
            f,
            "FieldInfo(annotation=NoneType, required={}",
            python_bool(self.is_required())
        )?;
        if let Some(default) = &self.default {
            write!(f, ", default={}", repr_string(default, vm))?;
        }
        if let Some(factory) = &self.default_factory {
            write!(f, ", default_factory={}", display_as_type(factory, vm))?;
        }
        if let Some(alias) = &self.alias {
            write!(f, ", alias={}", str_repr(alias, vm))?;
        }
        if self.alias.is_some() || self.validation_alias.is_some() || self.serialization_alias.is_some() {
            f.write_str(", alias_priority=2")?;
        }
        for (name, text) in [
            ("validation_alias", &self.validation_alias),
            ("serialization_alias", &self.serialization_alias),
            ("title", &self.title),
            ("description", &self.description),
        ] {
            if let Some(text) = text {
                write!(f, ", {name}={}", str_repr(text, vm))?;
            }
        }
        if let Some(examples) = &self.examples {
            write!(f, ", examples={}", repr_string(examples, vm))?;
        }
        if let Some(exclude) = self.exclude {
            write!(f, ", exclude={}", python_bool(exclude))?;
        }
        if let Some(extra) = &self.json_schema_extra {
            write!(f, ", json_schema_extra={}", repr_string(extra, vm))?;
        }
        if let Some(frozen) = self.frozen {
            write!(f, ", frozen={}", python_bool(frozen))?;
        }
        let metadata = self.metadata_reprs(vm);
        if !metadata.is_empty() {
            write!(f, ", metadata=[{}]", metadata.join(", "))?;
        }
        f.write_char(')')?;
        Ok(())
    }

    /// The constraints as pydantic lists them: `annotated_types` objects in
    /// pydantic's order, then its `_PydanticGeneralMetadata` for the rest.
    fn metadata_reprs(&self, vm: &mut VM<'_>) -> Vec<String> {
        let mut items = Vec::new();
        if let Some(strict) = self.strict {
            items.push(format!("Strict(strict={})", python_bool(strict)));
        }
        for (name, value) in [
            ("Gt", "gt", &self.gt),
            ("Ge", "ge", &self.ge),
            ("Lt", "lt", &self.lt),
            ("Le", "le", &self.le),
            ("MultipleOf", "multiple_of", &self.multiple_of),
        ]
        .into_iter()
        .filter_map(|(class, name, value)| value.as_ref().map(|value| ((class, name), value)))
        {
            items.push(format!("{}({}={})", name.0, name.1, repr_string(value, vm)));
        }
        if let Some(min_length) = self.min_length {
            items.push(format!("MinLen(min_length={min_length})"));
        }
        if let Some(max_length) = self.max_length {
            items.push(format!("MaxLen(max_length={max_length})"));
        }
        let mut general = Vec::new();
        if let Some(pattern) = &self.pattern {
            general.push(format!("pattern={}", str_repr(pattern, vm)));
        }
        if let Some(coerce) = self.coerce_numbers_to_str {
            general.push(format!("coerce_numbers_to_str={}", python_bool(coerce)));
        }
        if !general.is_empty() {
            items.push(format!("_PydanticGeneralMetadata({})", general.join(", ")));
        }
        items
    }
}

/// Python's spelling of a bool.
fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// `repr()` of a Rust string as a Python `str`.
fn str_repr(text: &str, vm: &mut VM<'_>) -> String {
    let value = allocate_string(text, vm.heap);
    let repr = repr_string(&value, vm);
    value.drop_with(vm);
    repr
}

impl HeapItem for FieldInfo {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Each value hands its reference to the stack and is left inert for the entry's drop.
        for value in self.values_mut() {
            value.py_dec_ref_ids(stack);
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FieldInfo {
    fn drop_with(self, ctx: &mut C) {
        let Self {
            default,
            default_factory,
            examples,
            json_schema_extra,
            gt,
            ge,
            lt,
            le,
            multiple_of,
            ..
        } = self;
        for value in [
            default,
            default_factory,
            examples,
            json_schema_extra,
            gt,
            ge,
            lt,
            le,
            multiple_of,
        ]
        .into_iter()
        .flatten()
        {
            value.drop_with(ctx);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, FieldInfo> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::FieldInfo
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Hashable by identity, as pydantic's is, so a stdlib `dataclass` accepts
    /// `Field(...)` as a default where it rejects a mutable one.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        // The reprs of stored values run Python, so a copy is read while they do.
        let info = self.get(vm.heap).clone_with_heap(vm.heap);
        defer_drop!(info, vm);
        let mut text = String::new();
        info.write_repr(&mut text, vm)?;
        Ok(f.write_str(&text)?)
    }

    /// The stored settings by name; an unset `default` raises, since Monty has
    /// no `PydanticUndefined` to return.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let info = self.get(vm.heap);
        let text = |text: &Option<String>| match text {
            Some(text) => allocate_string(text.as_str(), vm.heap),
            None => Value::None,
        };
        let flag = |flag: Option<bool>| flag.map_or(Value::None, Value::Bool);
        let owned = |value: &Option<Value>| value.as_ref().map_or(Value::None, |v| v.clone_with_heap(vm.heap));
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::Default) => match &info.default {
                Some(default) => default.clone_with_heap(vm.heap),
                None => return Err(ExcType::attribute_error("FieldInfo", "default")),
            },
            Some(StaticStrings::DefaultFactory) => owned(&info.default_factory),
            Some(StaticStrings::Alias) => text(&info.alias),
            Some(StaticStrings::ValidationAlias) => text(&info.validation_alias),
            Some(StaticStrings::SerializationAlias) => text(&info.serialization_alias),
            Some(StaticStrings::Title) => text(&info.title),
            Some(StaticStrings::Description) => text(&info.description),
            Some(StaticStrings::Examples) => owned(&info.examples),
            Some(StaticStrings::Exclude) => flag(info.exclude),
            Some(StaticStrings::Frozen) => flag(info.frozen),
            Some(StaticStrings::JsonSchemaExtra) => owned(&info.json_schema_extra),
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::IsRequired) => {
                args.check_zero_args("FieldInfo.is_required", vm.heap)?;
                Ok(CallResult::Value(Value::Bool(self.get(vm.heap).is_required())))
            }
            _ => Err(ExcType::attribute_error_method("FieldInfo", attr, args, vm)),
        }
    }
}
