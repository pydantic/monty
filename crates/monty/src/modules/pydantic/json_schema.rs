//! `TypeAdapter.json_schema()`: pydantic's `GenerateJsonSchema` output for the
//! types Monty validates, read off the arena.
//!
//! A dataclass is written inline when it is the root and does not refer to
//! itself; every other occurrence is a `$ref` into `$defs`, keyed by class
//! name as pydantic keys it. The schema is built as Python dicts and lists so
//! the result is what pydantic returns, not JSON text, and its keys are sorted
//! the way `GenerateJsonSchema.sort` leaves them.

use std::mem;

use ahash::AHashSet;

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId},
    modules::pydantic::{
        arena::{Arena, Constraints, DataclassValidator, FieldDefault, NamedTupleValidator, Validator, ValidatorId},
        serialize::{self, Mode, Options},
    },
    types::{Dict, List, allocate_string},
    value::Value,
};

/// pydantic's default `ref_template`.
pub(crate) const DEFAULT_REF_TEMPLATE: &str = "#/$defs/{model}";

/// The `string` branch pydantic writes for a `Decimal`.
const DECIMAL_PATTERN: &str = r"^(?!^[-+.]*$)[+-]?0*\d*\.?\d*$";

/// Builds the schema of `root`; `by_alias` defaults to true, as pydantic's does.
pub(crate) fn generate(
    arena: &Arena,
    root: ValidatorId,
    by_alias: Option<bool>,
    ref_template: &str,
    serialization: bool,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let mut generator = Generator {
        arena,
        by_alias: by_alias.unwrap_or(true),
        ref_template,
        serialization,
        defs: Vec::new(),
    };
    let schema = generator.schema(root, true, vm);
    let defs = mem::take(&mut generator.defs);
    let mut schema = match schema {
        Ok(schema) => schema,
        Err(err) => {
            defs.drop_with(vm);
            return Err(err);
        }
    };
    if !defs.is_empty() {
        let pairs = defs
            .into_iter()
            .map(|def| {
                (
                    def.name,
                    def.schema.expect("a definition is filled once generation succeeds"),
                )
            })
            .collect();
        let defs = match Object::from_pairs(pairs).into_value(vm) {
            Ok(defs) => defs,
            Err(err) => {
                schema.drop_with(vm);
                return Err(err);
            }
        };
        schema.0.insert(0, ("$defs".to_owned(), defs));
    }
    let schema = schema.into_value(vm)?;
    sorted(schema, None, vm)
}

/// pydantic's `GenerateJsonSchema.sort`: dict keys sorted recursively, except
/// the children of `properties` and `default`, which keep their order; a list
/// passes its parent key on to its items.
fn sorted(value: Value, parent_key: Option<&str>, vm: &mut VM<'_>) -> RunResult<Value> {
    let Value::Ref(id) = &value else {
        return Ok(value);
    };
    match vm.heap.get(*id) {
        HeapData::Dict(dict) => {
            let mut pairs: Vec<(Value, Value)> = dict
                .iter()
                .map(|(key, item)| (key.clone_with_heap(vm.heap), item.clone_with_heap(vm.heap)))
                .collect();
            value.drop_with(vm);
            if !matches!(parent_key, Some("properties" | "default")) {
                pairs.sort_by(|(a, _), (b, _)| a.as_str(vm).cmp(&b.as_str(vm)));
            }
            let mut out = Vec::with_capacity(pairs.len());
            let mut pending = pairs.into_iter();
            while let Some((key, item)) = pending.next() {
                let key_text = key.as_str(vm).map(str::to_owned);
                match sorted(item, key_text.as_deref(), vm) {
                    Ok(item) => out.push((key, item)),
                    Err(err) => {
                        key.drop_with(vm);
                        out.drop_with(vm);
                        pending.drop_with(vm);
                        return Err(err);
                    }
                }
            }
            let dict = Dict::from_pairs(out, vm)?;
            Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
        }
        HeapData::List(list) => {
            let items: Vec<Value> = list
                .as_slice()
                .iter()
                .map(|item| item.clone_with_heap(vm.heap))
                .collect();
            value.drop_with(vm);
            let mut out = Vec::with_capacity(items.len());
            let mut pending = items.into_iter();
            while let Some(item) = pending.next() {
                match sorted(item, parent_key, vm) {
                    Ok(item) => out.push(item),
                    Err(err) => {
                        out.drop_with(vm);
                        pending.drop_with(vm);
                        return Err(err);
                    }
                }
            }
            Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(out)))))
        }
        _ => Ok(value),
    }
}

/// A schema under construction: its keys in insertion order, values OWNED.
struct Object(Vec<(String, Value)>);

impl Object {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn from_pairs(pairs: Vec<(String, Value)>) -> Self {
        Self(pairs)
    }

    /// `{"type": name}`.
    fn typed(name: &str, vm: &mut VM<'_>) -> Self {
        let mut object = Self::new();
        object.set_str("type", name, vm);
        object
    }

    /// Adds `key`, replacing an earlier value of the same key.
    fn set(&mut self, key: &str, value: Value, vm: &mut VM<'_>) {
        if let Some(slot) = self.0.iter_mut().find(|(existing, _)| existing == key) {
            let old = mem::replace(&mut slot.1, value);
            old.drop_with(vm);
        } else {
            self.0.push((key.to_owned(), value));
        }
    }

    fn set_str(&mut self, key: &str, text: &str, vm: &mut VM<'_>) {
        self.set(key, allocate_string(text, vm.heap), vm);
    }

    fn set_usize(&mut self, key: &str, count: usize, vm: &mut VM<'_>) {
        self.set(key, Value::Int(i64::try_from(count).expect("lengths fit i64")), vm);
    }

    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(existing, _)| existing == key)
    }

    /// The schema as a Python dict.
    fn into_value(self, vm: &mut VM<'_>) -> RunResult<Value> {
        let pairs = self
            .0
            .into_iter()
            .map(|(key, value)| (allocate_string(key, vm.heap), value))
            .collect();
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Object {
    fn drop_with(self, ctx: &mut C) {
        for (_, value) in self.0 {
            value.drop_with(ctx);
        }
    }
}

/// The walk's state: the `$defs` collected so far, in order of first reference.
struct Generator<'a> {
    arena: &'a Arena,
    by_alias: bool,
    ref_template: &'a str,
    serialization: bool,
    defs: Vec<Definition>,
}

/// One `$defs` entry. `schema` is `None` while the class is being defined, so
/// a recursive reference becomes a `$ref` to the name rather than a loop.
struct Definition {
    /// The dataclass, by identity: one definition however many validators it has.
    class: HeapId,
    name: String,
    /// OWNED once filled.
    schema: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for Definition {
    fn drop_with(self, ctx: &mut C) {
        self.schema.drop_with(ctx);
    }
}

impl Generator<'_> {
    /// The schema of validator `id`; `root` allows a dataclass to be written inline.
    fn schema(&mut self, id: ValidatorId, root: bool, vm: &mut VM<'_>) -> RunResult<Object> {
        let schema = match self.arena.get(id) {
            Validator::Any => Object::new(),
            Validator::None => Object::typed("null", vm),
            Validator::Bool => Object::typed("boolean", vm),
            Validator::Int => Object::typed("integer", vm),
            Validator::Float => Object::typed("number", vm),
            Validator::Str => Object::typed("string", vm),
            Validator::Bytes => {
                let mut schema = Object::typed("string", vm);
                schema.set_str("format", "binary", vm);
                schema
            }
            Validator::DateTime => Self::formatted("date-time", vm),
            Validator::Date => Self::formatted("date", vm),
            Validator::Time => Self::formatted("time", vm),
            Validator::TimeDelta => Self::formatted("duration", vm),
            Validator::Decimal => Self::decimal(None, vm)?,
            Validator::List(item) => self.array(*item, false, vm)?,
            Validator::Set(item) | Validator::FrozenSet(item) => self.array(*item, true, vm)?,
            Validator::Tuple { items, variadic: true } => self.array(items[0], false, vm)?,
            Validator::Tuple { items, .. } => {
                let mut schema = Object::typed("array", vm);
                if !items.is_empty() {
                    let mut prefix = Vec::with_capacity(items.len());
                    for item in items {
                        let item = self.schema(*item, false, vm);
                        match item {
                            Ok(item) => prefix.push(item),
                            Err(err) => {
                                schema.drop_with(vm);
                                prefix.drop_with(vm);
                                return Err(err);
                            }
                        }
                    }
                    let prefix = objects_to_list(prefix, vm)?;
                    schema.set("prefixItems", prefix, vm);
                }
                schema.set_usize("minItems", items.len(), vm);
                schema.set_usize("maxItems", items.len(), vm);
                schema
            }
            Validator::Dict { value, .. } => {
                let mut schema = Object::typed("object", vm);
                let additional = if matches!(self.arena.get(*value), Validator::Any) {
                    Value::Bool(true)
                } else {
                    let inner = self.schema(*value, false, vm);
                    match inner {
                        Ok(inner) => inner.into_value(vm)?,
                        Err(err) => {
                            schema.drop_with(vm);
                            return Err(err);
                        }
                    }
                };
                schema.set("additionalProperties", additional, vm);
                schema
            }
            Validator::Union(choices) => self.any_of(choices, false, vm)?,
            Validator::Nullable(inner) => match self.arena.get(*inner) {
                Validator::Union(choices) => self.any_of(choices, true, vm)?,
                _ => self.any_of(&[*inner], true, vm)?,
            },
            Validator::Literal(literal) => Self::literal(&literal.choices, vm)?,
            Validator::Dataclass(dataclass) => self.dataclass(id, dataclass, root, vm)?,
            Validator::NamedTuple(named_tuple) => Self::named_tuple(named_tuple, vm)?,
            Validator::Strict { inner, .. } => self.schema(*inner, root, vm)?,
            Validator::Constrained { inner, constraints } => self.constrained(*inner, constraints, vm)?,
            Validator::Meta { inner, meta } => {
                let mut schema = self.schema(*inner, root, vm)?;
                let written = (|| {
                    if let Some(title) = &meta.title {
                        schema.set_str("title", title, vm);
                    }
                    if let Some(description) = &meta.description {
                        schema.set_str("description", description, vm);
                    }
                    if let Some(default) = &meta.default {
                        let default = jsonable(default, vm)?;
                        schema.set("default", default, vm);
                    }
                    if let Some(examples) = &meta.examples {
                        let examples = jsonable(examples, vm)?;
                        schema.set("examples", examples, vm);
                    }
                    if let Some(Value::Ref(extra)) = &meta.json_schema_extra
                        && let HeapData::Dict(extra) = vm.heap.get(*extra)
                    {
                        let pairs: Vec<(Value, Value)> = extra
                            .iter()
                            .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                            .collect();
                        for (key, value) in pairs {
                            let text = key.as_str(vm).map(str::to_owned);
                            key.drop_with(vm);
                            match text {
                                Some(text) => match jsonable(&value, vm) {
                                    Ok(json) => {
                                        value.drop_with(vm);
                                        schema.set(&text, json, vm);
                                    }
                                    Err(err) => {
                                        value.drop_with(vm);
                                        return Err(err);
                                    }
                                },
                                None => value.drop_with(vm),
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(err) = written {
                    schema.drop_with(vm);
                    return Err(err);
                }
                schema
            }
        };
        Ok(schema)
    }

    /// `{"type": "string", "format": name}`.
    fn formatted(format: &str, vm: &mut VM<'_>) -> Object {
        let mut schema = Object::typed("string", vm);
        schema.set_str("format", format, vm);
        schema
    }

    /// pydantic's `Decimal`: a number, or a string in decimal notation; numeric
    /// bounds go on the number branch as floats.
    fn decimal(bounds: Option<&Constraints>, vm: &mut VM<'_>) -> RunResult<Object> {
        let mut number = Object::typed("number", vm);
        if let Some(Constraints::Number(bounds)) = bounds {
            for (key, bound) in [
                ("multipleOf", &bounds.multiple_of),
                ("maximum", &bounds.le),
                ("exclusiveMaximum", &bounds.lt),
                ("minimum", &bounds.ge),
                ("exclusiveMinimum", &bounds.gt),
            ] {
                if let Some(bound) = bound {
                    let as_float = bound.py_float(vm);
                    match as_float {
                        Ok(Some(float)) => number.set(key, Value::Float(float), vm),
                        Ok(None) => {}
                        Err(err) => {
                            number.drop_with(vm);
                            return Err(err);
                        }
                    }
                }
            }
        }
        let mut text = Object::typed("string", vm);
        text.set_str("pattern", DECIMAL_PATTERN, vm);
        let branches = objects_to_list(vec![number, text], vm)?;
        let mut schema = Object::new();
        schema.set("anyOf", branches, vm);
        Ok(schema)
    }

    /// `{"type": "array", "items": ...}`, with `uniqueItems` for the sets.
    fn array(&mut self, item: ValidatorId, unique: bool, vm: &mut VM<'_>) -> RunResult<Object> {
        let item = self.schema(item, false, vm)?;
        let item = item.into_value(vm)?;
        let mut schema = Object::typed("array", vm);
        schema.set("items", item, vm);
        if unique {
            schema.set("uniqueItems", Value::Bool(true), vm);
        }
        Ok(schema)
    }

    /// `{"anyOf": [...]}`, with `{"type": "null"}` last for a nullable.
    fn any_of(&mut self, choices: &[ValidatorId], nullable: bool, vm: &mut VM<'_>) -> RunResult<Object> {
        let mut members = Vec::with_capacity(choices.len() + 1);
        for choice in choices {
            match self.schema(*choice, false, vm) {
                Ok(member) => members.push(member),
                Err(err) => {
                    members.drop_with(vm);
                    return Err(err);
                }
            }
        }
        if nullable {
            members.push(Object::typed("null", vm));
        }
        let members = objects_to_list(members, vm)?;
        let mut schema = Object::new();
        schema.set("anyOf", members, vm);
        Ok(schema)
    }

    /// `Literal[...]`: `const` for one choice, `enum` otherwise, plus `type` when
    /// the choices share a JSON type.
    fn literal(choices: &[Value], vm: &mut VM<'_>) -> RunResult<Object> {
        let mut schema = Object::new();
        let json_type = choices
            .iter()
            .map(json_type_of)
            .reduce(|a, b| if a == b { a } else { None });
        if let [choice] = choices {
            let value = jsonable(choice, vm)?;
            schema.set("const", value, vm);
        } else {
            let mut values = Vec::with_capacity(choices.len());
            for choice in choices {
                match jsonable(choice, vm) {
                    Ok(value) => values.push(value),
                    Err(err) => {
                        values.drop_with(vm);
                        return Err(err);
                    }
                }
            }
            let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(values))));
            schema.set("enum", list, vm);
        }
        if let Some(Some(json_type)) = json_type {
            schema.set_str("type", json_type, vm);
        }
        Ok(schema)
    }

    /// A dataclass: inline when it is the root and does not refer to itself,
    /// otherwise a `$ref` to its definition in `$defs`.
    fn dataclass(
        &mut self,
        id: ValidatorId,
        dataclass: &DataclassValidator,
        root: bool,
        vm: &mut VM<'_>,
    ) -> RunResult<Object> {
        if root && !self.reaches(id, id, &mut AHashSet::new()) {
            return self.object_schema(dataclass, vm);
        }
        let Value::Ref(class) = &dataclass.class else {
            unreachable!("a dataclass validator holds its class")
        };
        let class = *class;
        let name = if let Some(def) = self.defs.iter().find(|def| def.class == class) {
            def.name.clone()
        } else {
            let name = self.unique_name(&dataclass.name);
            self.defs.push(Definition {
                class,
                name: name.clone(),
                schema: None,
            });
            // On an error the caller drops `defs`, placeholder included.
            let definition = self.object_schema(dataclass, vm)?.into_value(vm)?;
            let slot = self.defs.iter_mut().find(|def| def.class == class);
            slot.expect("pushed above").schema = Some(definition);
            name
        };
        let mut schema = Object::new();
        schema.set_str("$ref", &self.ref_template.replace("{model}", &name), vm);
        Ok(schema)
    }

    /// `base`, or `base_2`, `base_3`, ... when another class already holds it.
    /// pydantic qualifies colliding names with their module path instead, which
    /// sandbox classes do not have.
    fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.defs.iter().any(|def| def.name == name);
        let mut name = base.to_owned();
        let mut suffix = 2;
        while taken(&name) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        name
    }

    /// pydantic's `field_title_should_be_set`: a field whose schema is a `$ref`,
    /// or a nullable of one, gets no generated title.
    fn auto_title(&self, id: ValidatorId) -> bool {
        match self.arena.get(id) {
            Validator::Dataclass(_) => false,
            Validator::Nullable(inner)
            | Validator::Strict { inner, .. }
            | Validator::Constrained { inner, .. }
            | Validator::Meta { inner, .. } => self.auto_title(*inner),
            _ => true,
        }
    }

    /// Whether validator `from` leads back to `target`.
    fn reaches(&self, from: ValidatorId, target: ValidatorId, seen: &mut AHashSet<ValidatorId>) -> bool {
        let children: Vec<ValidatorId> = match self.arena.get(from) {
            Validator::List(item) | Validator::Set(item) | Validator::FrozenSet(item) => vec![*item],
            Validator::Tuple { items, .. } => items.clone(),
            Validator::Dict { key, value } => vec![*key, *value],
            Validator::Union(choices) => choices.clone(),
            Validator::Nullable(inner)
            | Validator::Strict { inner, .. }
            | Validator::Constrained { inner, .. }
            | Validator::Meta { inner, .. } => vec![*inner],
            Validator::Dataclass(dataclass) => dataclass.fields.iter().map(|field| field.validator).collect(),
            _ => Vec::new(),
        };
        children
            .into_iter()
            .any(|child| child == target || (seen.insert(child) && self.reaches(child, target, seen)))
    }

    /// The `object` schema of a dataclass: one property per field, the required
    /// ones listed, `additionalProperties: false` under `extra='forbid'`.
    fn object_schema(&mut self, dataclass: &DataclassValidator, vm: &mut VM<'_>) -> RunResult<Object> {
        let mut properties = Object::new();
        let mut required: Vec<Value> = Vec::new();
        for field in &dataclass.fields {
            if self.serialization && field.exclude {
                continue;
            }
            let name = vm.interns.get_str(field.name).to_owned();
            let key = match (self.by_alias, self.serialization) {
                (true, true) => field.serialization_alias.clone().unwrap_or_else(|| name.clone()),
                (true, false) => field.alias.clone().unwrap_or_else(|| name.clone()),
                (false, _) => name.clone(),
            };
            let property = self.schema(field.validator, false, vm);
            let mut property = match property {
                Ok(property) => property,
                Err(err) => {
                    properties.drop_with(vm);
                    required.drop_with(vm);
                    return Err(err);
                }
            };
            if !property.has("title") && self.auto_title(field.validator) {
                property.set_str("title", &title_case(&name), vm);
            }
            match &field.default {
                FieldDefault::Value(default) if !property.has("default") => match jsonable(default, vm) {
                    Ok(default) => property.set("default", default, vm),
                    Err(err) => {
                        property.drop_with(vm);
                        properties.drop_with(vm);
                        required.drop_with(vm);
                        return Err(err);
                    }
                },
                FieldDefault::Required => required.push(allocate_string(key.as_str(), vm.heap)),
                _ => {}
            }
            match property.into_value(vm) {
                Ok(property) => properties.set(&key, property, vm),
                Err(err) => {
                    properties.drop_with(vm);
                    required.drop_with(vm);
                    return Err(err);
                }
            }
        }
        let mut schema = Object::new();
        if dataclass.extra_forbid {
            schema.set("additionalProperties", Value::Bool(false), vm);
        }
        match properties.into_value(vm) {
            Ok(properties) => schema.set("properties", properties, vm),
            Err(err) => {
                required.drop_with(vm);
                schema.drop_with(vm);
                return Err(err);
            }
        }
        if !required.is_empty() {
            let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(required))));
            schema.set("required", list, vm);
        }
        schema.set_str("title", dataclass.title.as_deref().unwrap_or(&dataclass.name), vm);
        schema.set_str("type", "object", vm);
        Ok(schema)
    }

    /// A namedtuple: an array of its fields, each titled, the defaulted ones optional.
    fn named_tuple(named_tuple: &NamedTupleValidator, vm: &mut VM<'_>) -> RunResult<Object> {
        let defaults: Vec<Value> = match &named_tuple.class {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::NamedTupleClass(class) => {
                    class.defaults().iter().map(|v| v.clone_with_heap(vm.heap)).collect()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let count = named_tuple.field_names.len();
        let first_default = count - named_tuple.defaults.min(count);
        let mut items = Vec::with_capacity(count);
        let mut defaults = defaults.into_iter();
        for (index, name) in named_tuple.field_names.iter().enumerate() {
            let mut item = Object::new();
            item.set_str("title", &title_case(name), vm);
            if index >= first_default
                && let Some(default) = defaults.next()
            {
                let json = jsonable(&default, vm);
                default.drop_with(vm);
                match json {
                    Ok(json) => item.set("default", json, vm),
                    Err(err) => {
                        item.drop_with(vm);
                        items.drop_with(vm);
                        defaults.collect::<Vec<Value>>().drop_with(vm);
                        return Err(err);
                    }
                }
            }
            items.push(item);
        }
        let mut schema = Object::typed("array", vm);
        let prefix = objects_to_list(items, vm)?;
        schema.set("prefixItems", prefix, vm);
        schema.set_usize("minItems", first_default, vm);
        schema.set_usize("maxItems", count, vm);
        Ok(schema)
    }

    /// `inner`'s schema with its constraints as JSON Schema keywords.
    fn constrained(&mut self, inner: ValidatorId, constraints: &Constraints, vm: &mut VM<'_>) -> RunResult<Object> {
        if matches!(self.arena.get(inner), Validator::Decimal) {
            return Self::decimal(Some(constraints), vm);
        }
        let mut schema = self.schema(inner, false, vm)?;
        match constraints {
            Constraints::Number(bounds) => {
                for (key, bound) in [
                    ("multipleOf", &bounds.multiple_of),
                    ("maximum", &bounds.le),
                    ("exclusiveMaximum", &bounds.lt),
                    ("minimum", &bounds.ge),
                    ("exclusiveMinimum", &bounds.gt),
                ] {
                    if let Some(bound) = bound {
                        schema.set(key, bound.clone_with_heap(vm.heap), vm);
                    }
                }
            }
            Constraints::Str(rules) => {
                if let Some(min) = rules.min_length {
                    schema.set_usize("minLength", min, vm);
                }
                if let Some(max) = rules.max_length {
                    schema.set_usize("maxLength", max, vm);
                }
                if let Some(pattern) = &rules.pattern {
                    schema.set_str("pattern", &pattern.source, vm);
                }
            }
            Constraints::Bytes { min_length, max_length } => {
                if let Some(min) = min_length {
                    schema.set_usize("minLength", *min, vm);
                }
                if let Some(max) = max_length {
                    schema.set_usize("maxLength", *max, vm);
                }
            }
            Constraints::Length { min_length, max_length } => {
                let (min_key, max_key) = if matches!(self.arena.get(inner), Validator::Dict { .. }) {
                    ("minProperties", "maxProperties")
                } else {
                    ("minItems", "maxItems")
                };
                if let Some(min) = min_length {
                    schema.set_usize(min_key, *min, vm);
                }
                if let Some(max) = max_length {
                    schema.set_usize(max_key, *max, vm);
                }
            }
        }
        Ok(schema)
    }
}

/// The JSON type name shared by `Literal` choices of one Python type.
fn json_type_of(value: &Value) -> Option<&'static str> {
    match value {
        Value::None => Some("null"),
        Value::Bool(_) => Some("boolean"),
        Value::Int(_) | Value::InternLongInt(_) => Some("integer"),
        Value::Float(_) => Some("number"),
        Value::InternString(_) | Value::InlineString(_) => Some("string"),
        _ => None,
    }
}

/// `name` as pydantic titles a field: words capitalised, underscores as spaces.
fn title_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// `value` as `dump_python(mode='json')` writes it, pydantic's `to_jsonable_python`.
fn jsonable(value: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut arena = Arena::default();
    let any = arena.push(Validator::Any);
    let options = Options {
        mode: Mode::Json,
        exclude_none: false,
        by_alias: false,
        capitals: false,
    };
    serialize::dump_python(&arena, any, value, options, vm)
}

/// A list of schema dicts.
fn objects_to_list(objects: Vec<Object>, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut values = Vec::with_capacity(objects.len());
    for object in objects {
        match object.into_value(vm) {
            Ok(value) => values.push(value),
            Err(err) => {
                values.drop_with(vm);
                return Err(err);
            }
        }
    }
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(values)))))
}
