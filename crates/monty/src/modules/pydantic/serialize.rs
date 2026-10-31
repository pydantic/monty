//! `TypeAdapter.dump_python` and `dump_json`: pydantic-core's serializers for
//! the types an arena knows.
//!
//! The arena is the schema. A value is written by its validator's shape when
//! its runtime type fits, so a dataclass is a mapping of its fields with
//! `exclude_none` honoured, and by its runtime type when it does not, as
//! pydantic falls back to inference (after a warning Monty does not emit).
//! Python mode keeps values and container types; JSON mode writes temporal
//! values as ISO text, bytes as text, sets and tuples as arrays and keys as
//! strings, whether the output is Python objects (`dump_python(mode='json')`)
//! or a document (`dump_json`).
//!
//! The walk reads the heap without mutating it, so a document can be written
//! through a tracker-backed [`JsonWriter`] while the heap is borrowed; Python
//! output is gathered into an [`Out`] tree first and allocated at the end.

use std::str;

use ahash::AHashSet;
use monty_types::{MontyDate, MontyTime, MontyTimeDelta};
use num_bigint::BigInt;
use smallvec::SmallVec;

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{
        dataclasses::is_dataclass_class,
        pydantic::{
            arena::{Arena, DataclassValidator, Validator, ValidatorId},
            input::SeqKind,
            iso,
            json_writer::{InfNan, JsonWriter, non_finite_text},
        },
    },
    types::{
        Dict, List, Set, allocate_string, allocate_tuple, bytes::allocate_bytes, date as date_type,
        datetime as datetime_type, set::FrozenSet, time as time_type, timedelta as timedelta_type,
    },
    value::Value,
};

/// `dump_python(mode=...)`: Python objects as they are, or JSON-compatible ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Python,
    Json,
}

/// The options a dump honours.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Options {
    pub(crate) mode: Mode,
    /// Skip a dataclass field whose value is `None`.
    pub(crate) exclude_none: bool,
    /// The `decimal` context's `capitals`, which `str(Decimal)` follows.
    pub(crate) capitals: bool,
}

/// Nesting pydantic's serializers allow before `Circular reference detected (depth exceeded)`.
const RECURSION_LIMIT: usize = 255;

/// `dump_python(instance, mode=, exclude_none=)`.
pub(crate) fn dump_python(
    arena: &Arena,
    root: ValidatorId,
    value: &Value,
    options: Options,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let mut sink = PySink {
        mode: options.mode,
        open: Vec::new(),
        pending_keys: Vec::new(),
        result: None,
    };
    let walked = Serializer::new(arena, options, false).value(root, value, &mut sink, vm);
    match walked {
        Ok(()) => sink.result.take().expect("a walk emits one value").build(vm),
        Err(err) => {
            sink.abandon(vm);
            Err(err)
        }
    }
}

/// `dump_json(instance, indent=, ensure_ascii=, exclude_none=)`: the document as `bytes`.
pub(crate) fn dump_json(
    arena: &Arena,
    root: ValidatorId,
    value: &Value,
    options: Options,
    indent: Option<usize>,
    ensure_ascii: bool,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let text = {
        let mut sink = JsonSink {
            writer: JsonWriter::new(indent, InfNan::Null, &vm.heap.tracker).with_ensure_ascii(ensure_ascii),
        };
        Serializer::new(arena, options, true).value(root, value, &mut sink, vm)?;
        sink.writer.finish()?
    };
    Ok(allocate_bytes(text.into_bytes(), vm.heap))
}

/// Where serialized values go: Python objects or JSON text.
trait Sink {
    fn none(&mut self) -> RunResult<()>;
    fn bool(&mut self, value: bool) -> RunResult<()>;
    fn int(&mut self, value: i64) -> RunResult<()>;
    /// `value` is the big integer itself, for a sink that keeps values.
    fn bigint(&mut self, value: &Value, big: &BigInt, vm: &VM<'_>) -> RunResult<()>;
    fn float(&mut self, value: f64) -> RunResult<()>;
    /// `value` is the `str` itself.
    fn str(&mut self, value: &Value, text: &str, vm: &VM<'_>) -> RunResult<()>;
    /// Bytes stay bytes in Python mode and decode as UTF-8 in JSON mode.
    fn bytes(&mut self, value: &Value, bytes: &[u8], vm: &VM<'_>) -> RunResult<()>;
    /// A temporal value or decimal: kept in Python mode, `text` in JSON mode.
    fn text_form(&mut self, value: &Value, text: String, vm: &VM<'_>) -> RunResult<()>;
    /// A value with no JSON form, kept as it is: Python mode only.
    fn keep(&mut self, value: &Value, vm: &VM<'_>) -> RunResult<()>;
    fn begin_seq(&mut self, kind: SeqKind) -> RunResult<()>;
    fn element(&mut self) -> RunResult<()>;
    fn end_seq(&mut self) -> RunResult<()>;
    fn begin_map(&mut self) -> RunResult<()>;
    fn key(&mut self, key: Key<'_>, vm: &VM<'_>) -> RunResult<()>;
    fn end_map(&mut self) -> RunResult<()>;
}

/// A mapping key: text (JSON mode, and field names) or the key value itself.
enum Key<'v> {
    Text(&'v str),
    Value(&'v Value),
}

/// The walk over one value.
struct Serializer<'a> {
    arena: &'a Arena,
    options: Options,
    /// Whether errors are `dump_json`'s `PydanticSerializationError`s.
    json_text: bool,
    depth: usize,
    /// The containers being written, so one that holds itself is reported.
    in_progress: AHashSet<HeapId>,
}

impl<'a> Serializer<'a> {
    fn new(arena: &'a Arena, options: Options, json_text: bool) -> Self {
        Self {
            arena,
            options,
            json_text,
            depth: 0,
            in_progress: AHashSet::new(),
        }
    }

    /// Writes `value` by its validator when its type fits, else by inference.
    fn value<S: Sink>(&mut self, id: ValidatorId, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        match self.arena.get(id) {
            Validator::Nullable(inner) => {
                if matches!(value, Value::None) {
                    sink.none()
                } else {
                    self.value(*inner, value, sink, vm)
                }
            }
            Validator::Union(choices) => match choices.iter().find(|choice| self.fits(**choice, value, vm)) {
                Some(choice) => self.value(*choice, value, sink, vm),
                None => self.infer(value, sink, vm),
            },
            // A typed float stays a float in JSON mode even when it is not
            // finite; inference writes `None` for it.
            Validator::Float => match value {
                Value::Float(f) => sink.float(*f),
                _ => self.infer(value, sink, vm),
            },
            Validator::List(item) => match heap_data(value, vm) {
                Some((id, HeapData::List(list))) => self.seq(id, SeqKind::List, list.as_slice(), Some(*item), sink, vm),
                _ => self.infer(value, sink, vm),
            },
            Validator::Set(item) => match heap_data(value, vm) {
                Some((id, HeapData::Set(set))) => {
                    let items: Vec<&Value> = set.iter().collect();
                    self.seq_refs(id, SeqKind::Set, &items, Some(*item), sink, vm)
                }
                _ => self.infer(value, sink, vm),
            },
            Validator::FrozenSet(item) => match heap_data(value, vm) {
                Some((id, HeapData::FrozenSet(set))) => {
                    let items: Vec<&Value> = set.storage().iter().collect();
                    self.seq_refs(id, SeqKind::FrozenSet, &items, Some(*item), sink, vm)
                }
                _ => self.infer(value, sink, vm),
            },
            Validator::Tuple { items, variadic } => match heap_data(value, vm) {
                Some((id, HeapData::Tuple(tuple))) => self.tuple(id, items, *variadic, tuple.as_slice(), sink, vm),
                Some((id, HeapData::NamedTuple(nt))) => self.tuple(id, items, *variadic, nt.items(), sink, vm),
                _ => self.infer(value, sink, vm),
            },
            Validator::Dict { key, value: item } => match heap_data(value, vm) {
                Some((id, HeapData::Dict(dict))) => self.map(id, dict, Some((*key, *item)), sink, vm),
                _ => self.infer(value, sink, vm),
            },
            Validator::Dataclass(dataclass) => match heap_data(value, vm) {
                Some((id, HeapData::Instance(instance))) if is_class(dataclass, instance.class()) => {
                    self.dataclass(id, dataclass, sink, vm)
                }
                _ => self.infer(value, sink, vm),
            },
            // Every scalar validator writes what inference writes for a value
            // of its type, and inference is the fallback for any other.
            _ => self.infer(value, sink, vm),
        }
    }

    /// Whether `value` is what the validator `id` describes, for choosing a
    /// union member; scalars by type, containers by kind, a dataclass by class.
    fn fits(&self, id: ValidatorId, value: &Value, vm: &VM<'_>) -> bool {
        let data = heap_data(value, vm).map(|(_, data)| data);
        match self.arena.get(id) {
            Validator::Any | Validator::Literal(_) => true,
            Validator::None => matches!(value, Value::None),
            Validator::Bool => matches!(value, Value::Bool(_)),
            Validator::Int => {
                matches!(value, Value::Int(_) | Value::InternLongInt(_)) || matches!(data, Some(HeapData::LongInt(_)))
            }
            Validator::Float => matches!(value, Value::Float(_)),
            Validator::Str => value.as_str(vm).is_some(),
            Validator::Bytes => matches!(value, Value::InternBytes(_)) || matches!(data, Some(HeapData::Bytes(_))),
            Validator::DateTime => matches!(data, Some(HeapData::DateTime(_))),
            Validator::Date => matches!(data, Some(HeapData::Date(_))),
            Validator::Time => matches!(data, Some(HeapData::Time(_))),
            Validator::TimeDelta => matches!(data, Some(HeapData::TimeDelta(_))),
            Validator::Decimal => matches!(data, Some(HeapData::Decimal(_))),
            Validator::List(_) => matches!(data, Some(HeapData::List(_))),
            Validator::Set(_) => matches!(data, Some(HeapData::Set(_))),
            Validator::FrozenSet(_) => matches!(data, Some(HeapData::FrozenSet(_))),
            Validator::Tuple { .. } | Validator::NamedTuple(_) => {
                matches!(data, Some(HeapData::Tuple(_) | HeapData::NamedTuple(_)))
            }
            Validator::Dict { .. } => matches!(data, Some(HeapData::Dict(_))),
            Validator::Union(choices) => choices.iter().any(|choice| self.fits(*choice, value, vm)),
            Validator::Nullable(inner) => matches!(value, Value::None) || self.fits(*inner, value, vm),
            Validator::Dataclass(dataclass) => match data {
                Some(HeapData::Instance(instance)) => is_class(dataclass, instance.class()),
                _ => false,
            },
        }
    }

    /// Writes `value` by its runtime type, as pydantic's `Any` serializer does.
    fn infer<S: Sink>(&mut self, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        if let Some(text) = value.as_str(vm) {
            return sink.str(value, text, vm);
        }
        match value {
            Value::None => sink.none(),
            Value::Bool(b) => sink.bool(*b),
            Value::Int(i) => sink.int(*i),
            Value::Float(f) if f.is_finite() || self.options.mode == Mode::Python => sink.float(*f),
            Value::Float(_) => sink.none(),
            Value::InternBytes(id) => sink.bytes(value, vm.interns.get_bytes(*id), vm),
            Value::InternLongInt(id) => sink.bigint(value, vm.interns.get_long_int(*id), vm),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Bytes(bytes) => sink.bytes(value, bytes.as_slice(), vm),
                HeapData::LongInt(big) => sink.bigint(value, big.inner(), vm),
                HeapData::List(list) => self.seq(*id, SeqKind::List, list.as_slice(), None, sink, vm),
                HeapData::Tuple(tuple) => self.seq(*id, SeqKind::Tuple, tuple.as_slice(), None, sink, vm),
                HeapData::NamedTuple(nt) => self.seq(*id, SeqKind::Tuple, nt.items(), None, sink, vm),
                HeapData::Set(set) => {
                    let items: Vec<&Value> = set.iter().collect();
                    self.seq_refs(*id, SeqKind::Set, &items, None, sink, vm)
                }
                HeapData::FrozenSet(set) => {
                    let items: Vec<&Value> = set.storage().iter().collect();
                    self.seq_refs(*id, SeqKind::FrozenSet, &items, None, sink, vm)
                }
                HeapData::Dict(dict) => self.map(*id, dict, None, sink, vm),
                HeapData::Instance(instance) if is_dataclass_class(instance.class(), vm) => {
                    self.inferred_dataclass(*id, instance.class(), sink, vm)
                }
                HeapData::Date(_) | HeapData::DateTime(_) | HeapData::Time(_) | HeapData::TimeDelta(_) => {
                    match (self.options.mode, temporal_text(*id, vm)) {
                        (Mode::Python, _) => sink.keep(value, vm),
                        (Mode::Json, Some(text)) => sink.text_form(value, text, vm),
                        (Mode::Json, None) => Err(Self::unknown_type(value, vm)),
                    }
                }
                HeapData::Decimal(decimal) => match self.options.mode {
                    Mode::Python => sink.keep(value, vm),
                    Mode::Json => sink.text_form(value, decimal.to_sci_string(false, self.options.capitals), vm),
                },
                _ => self.unknown(value, sink, vm),
            },
            _ => self.unknown(value, sink, vm),
        }
    }

    /// A value with no JSON form: kept in Python mode, an error in JSON mode.
    fn unknown<S: Sink>(&mut self, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        match self.options.mode {
            Mode::Python => sink.keep(value, vm),
            Mode::Json => Err(Self::unknown_type(value, vm)),
        }
    }

    /// pydantic's error for a type its inference does not know.
    fn unknown_type(value: &Value, vm: &VM<'_>) -> RunError {
        serialization_error(format!(
            "Unable to serialize unknown type: <class '{}'>",
            value.py_type_name(vm)
        ))
    }

    /// Enters container `id`, failing on a reference back or past the depth limit.
    fn enter(&mut self, id: HeapId) -> RunResult<()> {
        if self.depth >= RECURSION_LIMIT {
            return Err(self.circular("depth exceeded"));
        }
        if !self.in_progress.insert(id) {
            return Err(self.circular("id repeated"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self, id: HeapId) {
        self.depth -= 1;
        self.in_progress.remove(&id);
    }

    /// pydantic's cycle error, a `ValueError` that `dump_json` wraps.
    fn circular(&self, reason: &str) -> RunError {
        let message = format!("Circular reference detected ({reason})");
        if self.json_text {
            serialization_error(format!("Error serializing to JSON: ValueError: {message}"))
        } else {
            ExcType::value_error(message)
        }
    }

    /// A sequence of `kind` from a slice of items.
    fn seq<S: Sink>(
        &mut self,
        id: HeapId,
        kind: SeqKind,
        items: &[Value],
        item: Option<ValidatorId>,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        let refs: Vec<&Value> = items.iter().collect();
        self.seq_refs(id, kind, &refs, item, sink, vm)
    }

    /// A sequence of `kind`, each item by `item` or by inference.
    fn seq_refs<S: Sink>(
        &mut self,
        id: HeapId,
        kind: SeqKind,
        items: &[&Value],
        item: Option<ValidatorId>,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        self.enter(id)?;
        let written = (|| {
            sink.begin_seq(kind)?;
            for value in items {
                sink.element()?;
                self.item(item, value, sink, vm)?;
            }
            sink.end_seq()
        })();
        self.leave(id);
        written
    }

    /// A tuple against `tuple[A, B]` or `tuple[T, ...]`: positions past the
    /// fixed items take the variadic validator, or inference.
    fn tuple<S: Sink>(
        &mut self,
        id: HeapId,
        validators: &[ValidatorId],
        variadic: bool,
        items: &[Value],
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        let fixed = if variadic {
            validators.len() - 1
        } else {
            validators.len()
        };
        self.enter(id)?;
        let written = (|| {
            sink.begin_seq(SeqKind::Tuple)?;
            for (index, value) in items.iter().enumerate() {
                sink.element()?;
                let validator = if index < fixed {
                    Some(validators[index])
                } else if variadic {
                    Some(validators[fixed])
                } else {
                    None
                };
                self.item(validator, value, sink, vm)?;
            }
            sink.end_seq()
        })();
        self.leave(id);
        written
    }

    /// A mapping, keys and values by their validators or by inference.
    fn map<S: Sink>(
        &mut self,
        id: HeapId,
        dict: &Dict,
        validators: Option<(ValidatorId, ValidatorId)>,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        self.enter(id)?;
        let written = (|| {
            sink.begin_map()?;
            for (key, value) in dict {
                self.key(validators.map(|(key, _)| key), key, sink, vm)?;
                self.item(validators.map(|(_, value)| value), value, sink, vm)?;
            }
            sink.end_map()
        })();
        self.leave(id);
        written
    }

    /// A dataclass instance by its validator: fields in declaration order,
    /// `None` values skipped under `exclude_none`.
    fn dataclass<S: Sink>(
        &mut self,
        id: HeapId,
        dataclass: &DataclassValidator,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        let HeapData::Instance(instance) = vm.heap.get(id) else {
            unreachable!("a dataclass validator matched an instance")
        };
        self.enter(id)?;
        let written = (|| {
            sink.begin_map()?;
            for field in &dataclass.fields {
                let name = vm.interns.get_str(field.name);
                let Some(value) = instance.attrs().get_by_str(name, vm.heap, vm.interns) else {
                    continue;
                };
                if self.options.exclude_none && matches!(value, Value::None) {
                    continue;
                }
                sink.key(Key::Text(name), vm)?;
                self.value(field.validator, value, sink, vm)?;
            }
            sink.end_map()
        })();
        self.leave(id);
        written
    }

    /// A dataclass instance met by inference: its class's fields, values inferred.
    fn inferred_dataclass<S: Sink>(
        &mut self,
        id: HeapId,
        class_id: HeapId,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        let HeapData::Instance(instance) = vm.heap.get(id) else {
            unreachable!("inference matched an instance")
        };
        let fields_attr: &'static str = StaticStrings::DataclassFields.into();
        let fields = match vm.heap.get(class_id) {
            HeapData::Class(class) => class.namespace().get_by_str(fields_attr, vm.heap, vm.interns),
            _ => None,
        };
        let Some((_, HeapData::Dict(fields))) = fields.and_then(|fields| heap_data(fields, vm)) else {
            return self.unknown(&Value::Ref(id), sink, vm);
        };
        self.enter(id)?;
        let written = (|| {
            sink.begin_map()?;
            for (name, _) in fields {
                let Some(name) = name.as_str(vm) else { continue };
                let Some(value) = instance.attrs().get_by_str(name, vm.heap, vm.interns) else {
                    continue;
                };
                sink.key(Key::Text(name), vm)?;
                self.infer(value, sink, vm)?;
            }
            sink.end_map()
        })();
        self.leave(id);
        written
    }

    /// One container item, by `validator` when there is one.
    fn item<S: Sink>(
        &mut self,
        validator: Option<ValidatorId>,
        value: &Value,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        match validator {
            Some(validator) => self.value(validator, value, sink, vm),
            None => self.infer(value, sink, vm),
        }
    }

    /// Whether `id` types `value` as a `float`, looking through the member of
    /// a union or `Optional` that the value fits, as pydantic's key serializer does.
    fn is_float(&self, id: ValidatorId, value: &Value, vm: &VM<'_>) -> bool {
        match self.arena.get(id) {
            Validator::Float => true,
            Validator::Nullable(inner) => self.is_float(*inner, value, vm),
            Validator::Union(choices) => choices
                .iter()
                .find(|choice| self.fits(**choice, value, vm))
                .is_some_and(|choice| self.is_float(*choice, value, vm)),
            _ => false,
        }
    }

    /// A dict key: the value itself in Python mode, pydantic's key text in
    /// JSON mode, where a key typed `float` writes a non-finite value as `inf`
    /// / `nan` and an inferred one as `None`.
    fn key<S: Sink>(
        &mut self,
        validator: Option<ValidatorId>,
        key: &Value,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        match (self.options.mode, key) {
            (Mode::Python, _) => sink.key(Key::Value(key), vm),
            (Mode::Json, Value::Float(f))
                if !f.is_finite() && validator.is_some_and(|id| self.is_float(id, key, vm)) =>
            {
                sink.key(Key::Text(non_finite_text(*f)), vm)
            }
            (Mode::Json, _) => {
                let text = self.key_text(key, vm)?;
                sink.key(Key::Text(&text), vm)
            }
        }
    }

    /// pydantic's JSON object key for a value: strings as they are, `None`
    /// for `None` and non-finite floats, `true` / `false`, a number's digits,
    /// ISO text for temporal values, a tuple's keys joined by commas.
    fn key_text(&self, key: &Value, vm: &VM<'_>) -> RunResult<String> {
        if let Some(text) = key.as_str(vm) {
            return Ok(text.to_owned());
        }
        match key {
            Value::None => Ok("None".to_owned()),
            Value::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
            Value::Int(i) => Ok(i.to_string()),
            Value::Float(f) if f.is_finite() => Ok(zmij::Buffer::new().format_finite(*f).to_owned()),
            Value::Float(_) => Ok("None".to_owned()),
            Value::InternBytes(id) => self.bytes_text(vm.interns.get_bytes(*id)),
            Value::InternLongInt(id) => Ok(vm.interns.get_long_int(*id).to_string()),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Bytes(bytes) => self.bytes_text(bytes.as_slice()),
                HeapData::LongInt(big) => Ok(big.inner().to_string()),
                HeapData::Decimal(decimal) => Ok(decimal.to_sci_string(false, self.options.capitals)),
                HeapData::Date(_) | HeapData::DateTime(_) | HeapData::Time(_) | HeapData::TimeDelta(_) => {
                    temporal_text(*id, vm).ok_or_else(|| Self::unknown_type(key, vm))
                }
                HeapData::Tuple(tuple) => self.joined_keys(tuple.as_slice(), vm),
                HeapData::NamedTuple(nt) => self.joined_keys(nt.items(), vm),
                _ => Err(Self::unknown_type(key, vm)),
            },
            _ => Err(Self::unknown_type(key, vm)),
        }
    }

    /// A tuple key's items as keys, comma-separated.
    fn joined_keys(&self, items: &[Value], vm: &VM<'_>) -> RunResult<String> {
        let parts: Vec<String> = items
            .iter()
            .map(|item| self.key_text(item, vm))
            .collect::<RunResult<_>>()?;
        Ok(parts.join(","))
    }

    /// Bytes as UTF-8 text, with the error for the mode.
    fn bytes_text(&self, bytes: &[u8]) -> RunResult<String> {
        decode_utf8(bytes, self.json_text).map(str::to_owned)
    }
}

/// Bytes as UTF-8 text: `dump_json` reports the Rust error, `mode='json'` a
/// `UnicodeDecodeError`, as pydantic's two paths do.
fn decode_utf8(bytes: &[u8], json_text: bool) -> RunResult<&str> {
    str::from_utf8(bytes).map_err(|err| {
        if json_text {
            serialization_error(format!("Error serializing to JSON: {err}"))
        } else {
            let start = err.valid_up_to();
            ExcType::unicode_decode_error("utf-8", bytes, start, start + 1, "invalid utf-8")
        }
    })
}

/// A `PydanticSerializationError` with `message`.
fn serialization_error(message: String) -> RunError {
    SimpleException::new_msg(ExcType::PydanticSerializationError, message).into()
}

/// Whether `class_id` is the class a dataclass validator was built for.
fn is_class(dataclass: &DataclassValidator, class_id: HeapId) -> bool {
    matches!(dataclass.class, Value::Ref(id) if id == class_id)
}

/// The heap entry `value` names, with its id.
fn heap_data<'v>(value: &Value, vm: &'v VM<'_>) -> Option<(HeapId, &'v HeapData)> {
    match value {
        Value::Ref(id) => Some((*id, vm.heap.get(*id))),
        _ => None,
    }
}

/// The ISO text of a temporal heap value; `None` for a datetime whose zone
/// has no fixed offset to write.
fn temporal_text(id: HeapId, vm: &VM<'_>) -> Option<String> {
    match vm.heap.read(id) {
        HeapReadOutput::Date(d) => {
            let (year, month, day) = date_type::to_ymd(*d.get(vm.heap));
            Some(iso::date(&MontyDate {
                year,
                month: u8::try_from(month).expect("month is always 1..=12"),
                day: u8::try_from(day).expect("day is always 1..=31"),
            }))
        }
        HeapReadOutput::DateTime(dt) => datetime_type::to_monty_datetime(dt.get(vm.heap)).map(|dt| iso::datetime(&dt)),
        HeapReadOutput::Time(t) => {
            let time = t.get(vm.heap);
            let (hour, minute, second, microsecond, fold) = time.to_components();
            let tz = time_type::attached_timezone(time, vm.heap);
            Some(iso::time(&MontyTime {
                hour,
                minute,
                second,
                microsecond,
                offset_seconds: tz.as_ref().map(|tz| tz.offset_seconds),
                timezone_name: tz.and_then(|tz| tz.name),
                fold,
            }))
        }
        HeapReadOutput::TimeDelta(td) => {
            let (days, seconds, microseconds) = timedelta_type::components(td.get(vm.heap));
            Some(iso::timedelta(&MontyTimeDelta {
                days,
                seconds,
                microseconds,
            }))
        }
        _ => None,
    }
}

/// A Python-mode result under construction: scalars are values already,
/// containers are built once the walk is complete.
enum Out {
    Value(Value),
    Seq(SeqKind, Vec<Self>),
    Map(Vec<(Self, Self)>),
}

impl Out {
    /// Allocates the tree as values.
    fn build(self, vm: &mut VM<'_>) -> RunResult<Value> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Seq(kind, items) => {
                let values = build_all(items, vm)?;
                match kind {
                    SeqKind::List => Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(values))))),
                    SeqKind::Tuple => Ok(allocate_tuple(SmallVec::from_vec(values), vm.heap)),
                    SeqKind::Set | SeqKind::FrozenSet => build_set(values, kind == SeqKind::FrozenSet, vm),
                }
            }
            Self::Map(pairs) => {
                let mut building = DropGuard::new(Partial::new(Dict::new(), pairs), vm);
                loop {
                    let (partial, vm) = building.as_parts_mut();
                    let Some((key, value)) = partial.pending.pop() else {
                        break;
                    };
                    let key = key.build(vm)?;
                    let value = match value.build(vm) {
                        Ok(value) => value,
                        Err(err) => {
                            key.drop_with(vm);
                            return Err(err);
                        }
                    };
                    if let Some(replaced) = partial.built.set(key, value, vm)? {
                        replaced.drop_with(vm);
                    }
                }
                let (partial, vm) = building.into_parts();
                Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(partial.built))))
            }
        }
    }
}

/// Builds every item, releasing the rest when one fails.
fn build_all(items: Vec<Out>, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let mut building = DropGuard::new(Partial::new(Vec::with_capacity(items.len()), items), vm);
    loop {
        let (partial, vm) = building.as_parts_mut();
        let Some(item) = partial.pending.pop() else { break };
        let value = item.build(vm)?;
        partial.built.push(value);
    }
    Ok(building.into_inner().built)
}

/// A set or frozenset from its items, in order.
fn build_set(values: Vec<Value>, frozen: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut building = DropGuard::new(Partial::new(Set::with_capacity(values.len()), values), vm);
    loop {
        let (partial, vm) = building.as_parts_mut();
        let Some(value) = partial.pending.pop() else { break };
        partial.built.add(value, vm)?;
    }
    let (partial, vm) = building.into_parts();
    Ok(Value::Ref(vm.heap.allocate(if frozen {
        HeapData::FrozenSet(FrozenSet::from_set(partial.built))
    } else {
        HeapData::Set(partial.built)
    })))
}

/// A container being built from its pending items, both released together
/// when building fails. `pending` is popped from the back, so it is reversed
/// on construction to keep the items' order.
struct Partial<B, P> {
    built: B,
    pending: Vec<P>,
}

impl<B, P> Partial<B, P> {
    fn new(built: B, mut pending: Vec<P>) -> Self {
        pending.reverse();
        Self { built, pending }
    }
}

impl<C: ContainsHeap, B: DropWithContext<C>, P: DropWithContext<C>> DropWithContext<C> for Partial<B, P> {
    fn drop_with(self, ctx: &mut C) {
        self.built.drop_with(ctx);
        self.pending.drop_with(ctx);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Out {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Value(value) => value.drop_with(ctx),
            Self::Seq(_, items) => items.drop_with(ctx),
            Self::Map(pairs) => pairs.drop_with(ctx),
        }
    }
}

/// Gathers Python objects: in JSON mode every sequence is a list.
struct PySink {
    mode: Mode,
    /// The containers being filled, innermost last.
    open: Vec<Out>,
    /// The map keys waiting for their values, one per open map at most.
    pending_keys: Vec<Out>,
    result: Option<Out>,
}

impl PySink {
    /// Places a finished value: in the open container, or as the result.
    fn emit(&mut self, out: Out) {
        match self.open.last_mut() {
            Some(Out::Seq(_, items)) => items.push(out),
            Some(Out::Map(pairs)) => {
                let key = self.pending_keys.pop().expect("a key precedes each map value");
                pairs.push((key, out));
            }
            Some(Out::Value(_)) => unreachable!("only containers are open"),
            None => self.result = Some(out),
        }
    }

    /// Closes the innermost container.
    fn close(&mut self) {
        let out = self.open.pop().expect("a container is open");
        self.emit(out);
    }

    /// Releases everything gathered so far.
    fn abandon(self, vm: &mut VM<'_>) {
        self.open.drop_with(vm);
        self.pending_keys.drop_with(vm);
        if let Some(result) = self.result {
            result.drop_with(vm);
        }
    }
}

impl Sink for PySink {
    fn none(&mut self) -> RunResult<()> {
        self.emit(Out::Value(Value::None));
        Ok(())
    }

    fn bool(&mut self, value: bool) -> RunResult<()> {
        self.emit(Out::Value(Value::Bool(value)));
        Ok(())
    }

    fn int(&mut self, value: i64) -> RunResult<()> {
        self.emit(Out::Value(Value::Int(value)));
        Ok(())
    }

    fn bigint(&mut self, value: &Value, _big: &BigInt, vm: &VM<'_>) -> RunResult<()> {
        self.emit(Out::Value(value.clone_with_heap(vm.heap)));
        Ok(())
    }

    fn float(&mut self, value: f64) -> RunResult<()> {
        self.emit(Out::Value(Value::Float(value)));
        Ok(())
    }

    fn str(&mut self, value: &Value, _text: &str, vm: &VM<'_>) -> RunResult<()> {
        self.emit(Out::Value(value.clone_with_heap(vm.heap)));
        Ok(())
    }

    fn bytes(&mut self, value: &Value, bytes: &[u8], vm: &VM<'_>) -> RunResult<()> {
        let out = match self.mode {
            Mode::Python => value.clone_with_heap(vm.heap),
            Mode::Json => allocate_string(decode_utf8(bytes, false)?, vm.heap),
        };
        self.emit(Out::Value(out));
        Ok(())
    }

    fn text_form(&mut self, value: &Value, text: String, vm: &VM<'_>) -> RunResult<()> {
        let out = match self.mode {
            Mode::Python => value.clone_with_heap(vm.heap),
            Mode::Json => allocate_string(text, vm.heap),
        };
        self.emit(Out::Value(out));
        Ok(())
    }

    fn keep(&mut self, value: &Value, vm: &VM<'_>) -> RunResult<()> {
        self.emit(Out::Value(value.clone_with_heap(vm.heap)));
        Ok(())
    }

    fn begin_seq(&mut self, kind: SeqKind) -> RunResult<()> {
        let kind = match self.mode {
            Mode::Python => kind,
            Mode::Json => SeqKind::List,
        };
        self.open.push(Out::Seq(kind, Vec::new()));
        Ok(())
    }

    fn element(&mut self) -> RunResult<()> {
        Ok(())
    }

    fn end_seq(&mut self) -> RunResult<()> {
        self.close();
        Ok(())
    }

    fn begin_map(&mut self) -> RunResult<()> {
        self.open.push(Out::Map(Vec::new()));
        Ok(())
    }

    fn key(&mut self, key: Key<'_>, vm: &VM<'_>) -> RunResult<()> {
        let key = match key {
            Key::Text(text) => allocate_string(text, vm.heap),
            Key::Value(value) => value.clone_with_heap(vm.heap),
        };
        self.pending_keys.push(Out::Value(key));
        Ok(())
    }

    fn end_map(&mut self) -> RunResult<()> {
        self.close();
        Ok(())
    }
}

/// Writes the JSON document.
struct JsonSink<'t> {
    writer: JsonWriter<'t>,
}

impl Sink for JsonSink<'_> {
    fn none(&mut self) -> RunResult<()> {
        self.writer.null()
    }

    fn bool(&mut self, value: bool) -> RunResult<()> {
        self.writer.bool(value)
    }

    fn int(&mut self, value: i64) -> RunResult<()> {
        self.writer.int(value)
    }

    fn bigint(&mut self, _value: &Value, big: &BigInt, _vm: &VM<'_>) -> RunResult<()> {
        self.writer.bigint(big)
    }

    fn float(&mut self, value: f64) -> RunResult<()> {
        self.writer.float(value)
    }

    fn str(&mut self, _value: &Value, text: &str, _vm: &VM<'_>) -> RunResult<()> {
        self.writer.string(text)
    }

    fn bytes(&mut self, _value: &Value, bytes: &[u8], _vm: &VM<'_>) -> RunResult<()> {
        self.writer.string(decode_utf8(bytes, true)?)
    }

    fn text_form(&mut self, _value: &Value, text: String, _vm: &VM<'_>) -> RunResult<()> {
        self.writer.string(&text)
    }

    fn keep(&mut self, _value: &Value, _vm: &VM<'_>) -> RunResult<()> {
        unreachable!("JSON mode writes every value or refuses it")
    }

    fn begin_seq(&mut self, _kind: SeqKind) -> RunResult<()> {
        self.writer.begin_array()
    }

    fn element(&mut self) -> RunResult<()> {
        self.writer.element()
    }

    fn end_seq(&mut self) -> RunResult<()> {
        self.writer.end_array()
    }

    fn begin_map(&mut self) -> RunResult<()> {
        self.writer.begin_object()
    }

    fn key(&mut self, key: Key<'_>, _vm: &VM<'_>) -> RunResult<()> {
        match key {
            Key::Text(text) => self.writer.key(text),
            Key::Value(_) => unreachable!("JSON mode renders keys as text"),
        }
    }

    fn end_map(&mut self) -> RunResult<()> {
        self.writer.end_object()
    }
}
