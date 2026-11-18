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

use std::{mem::ManuallyDrop, str};

use ahash::AHashSet;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
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
            json_writer::{InfNan, JsonWriter, RECURSION_LIMIT, Surface, bool_text, complex_text, float_key},
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
    let walked = Serializer::new(arena, options, Surface::Python).value(root, value, &mut sink, vm);
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
        Serializer::new(arena, options, Surface::DumpJson).value(root, value, &mut sink, vm)?;
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
    /// How errors are raised: `dump_json` wraps them in a `PydanticSerializationError`.
    surface: Surface,
    /// Inferred values and typed dataclasses being written, for pydantic's depth guard.
    depth: usize,
    /// The inferred containers being written, so one that holds itself is caught.
    inferring: AHashSet<HeapId>,
    /// The dataclasses being written by their validators, guarded apart from
    /// inference as pydantic guards a recursive type's definition.
    defining: AHashSet<HeapId>,
    /// Values written so far, keying the clock poll: a container shared
    /// many times is written once per reference, far beyond the heap's size.
    steps: usize,
}

impl<'a> Serializer<'a> {
    fn new(arena: &'a Arena, options: Options, surface: Surface) -> Self {
        Self {
            arena,
            options,
            surface,
            depth: 0,
            inferring: AHashSet::new(),
            defining: AHashSet::new(),
            steps: 0,
        }
    }

    /// Writes `value` by its validator when its type fits, else by inference.
    fn value<S: Sink>(&mut self, id: ValidatorId, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        self.step(vm)?;
        match self.arena.get(id) {
            Validator::Nullable(inner) => {
                if matches!(value, Value::None) {
                    sink.none()
                } else {
                    self.value(*inner, value, sink, vm)
                }
            }
            Validator::Union(choices) => match self.choose(choices, value, vm) {
                Some(choice) => self.value(choice, value, sink, vm),
                None => self.infer(value, sink, vm),
            },
            // A typed float stays a float in JSON mode even when it is not
            // finite (inference writes `None`), and an int or bool becomes one.
            Validator::Float => match value {
                Value::Float(f) => sink.float(*f),
                _ if self.options.mode == Mode::Json => match int_as_float(value, vm) {
                    Some(Ok(float)) => sink.float(float),
                    // `mode='json'` overflows on an int past `f64`; `dump_json` writes it as an int.
                    Some(Err(err)) if self.surface == Surface::Python => Err(err),
                    _ => self.infer(value, sink, vm),
                },
                _ => self.infer(value, sink, vm),
            },
            // JSON mode writes a bool under an `int` hint as its number.
            Validator::Int if self.options.mode == Mode::Json => match value {
                Value::Bool(b) => sink.int(i64::from(*b)),
                _ => self.infer(value, sink, vm),
            },
            Validator::List(item) => match heap_data(value, vm) {
                Some((id, HeapData::List(list))) => self.seq(id, SeqKind::List, list.as_slice(), Some(*item), sink, vm),
                _ => self.infer(value, sink, vm),
            },
            Validator::Set(item) => match heap_data(value, vm) {
                Some((id, HeapData::Set(set))) => self.seq(id, SeqKind::Set, set.iter(), Some(*item), sink, vm),
                _ => self.infer(value, sink, vm),
            },
            Validator::FrozenSet(item) => match heap_data(value, vm) {
                Some((id, HeapData::FrozenSet(set))) => {
                    self.seq(id, SeqKind::FrozenSet, set.storage().iter(), Some(*item), sink, vm)
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
            // Every other scalar validator writes what inference writes for a
            // value of its type, and inference is the fallback for any other.
            _ => self.infer(value, sink, vm),
        }
    }

    /// The union member that writes `value`, as pydantic's union serializer
    /// picks it: the first exact fit, else, when a member asks for the lax
    /// retry, the first that takes `value` laxly.
    fn choose(&self, choices: &[ValidatorId], value: &Value, vm: &VM<'_>) -> Option<ValidatorId> {
        choices
            .iter()
            .copied()
            .find(|choice| self.fits(*choice, value, vm))
            .or_else(|| {
                choices
                    .iter()
                    .any(|choice| self.retries_lax(*choice))
                    .then(|| choices.iter().find_map(|choice| self.lax_fit(*choice, value, vm)))
                    .flatten()
            })
    }

    /// Whether pydantic retries a union laxly for member `id`: `int`, tuples
    /// and dataclasses ask for it, lists and `Optional`s through their item.
    fn retries_lax(&self, id: ValidatorId) -> bool {
        match self.arena.get(id) {
            Validator::Int | Validator::Tuple { .. } | Validator::Dataclass(_) => true,
            Validator::List(item) | Validator::Nullable(item) => self.retries_lax(*item),
            Validator::Union(choices) => choices.iter().any(|choice| self.retries_lax(*choice)),
            _ => false,
        }
    }

    /// The member under `id` that takes `value` in the lax retry, which only
    /// a number reaches: an `int` takes a bool, a `float` a bool or an int it can hold.
    fn lax_fit(&self, id: ValidatorId, value: &Value, vm: &VM<'_>) -> Option<ValidatorId> {
        match self.arena.get(id) {
            Validator::Int => matches!(value, Value::Bool(_)).then_some(id),
            Validator::Float => matches!(int_as_float(value, vm), Some(Ok(_))).then_some(id),
            Validator::Nullable(inner) => self.lax_fit(*inner, value, vm),
            Validator::Union(choices) => choices.iter().find_map(|choice| self.lax_fit(*choice, value, vm)),
            _ => None,
        }
    }

    /// Whether `value` is what the validator `id` describes, for choosing a
    /// union member; scalars by type, containers by kind, a dataclass by class.
    fn fits(&self, id: ValidatorId, value: &Value, vm: &VM<'_>) -> bool {
        let data = heap_data(value, vm).map(|(_, data)| data);
        match self.arena.get(id) {
            Validator::Any => true,
            Validator::Literal(literal) => literal.choices.iter().any(|choice| literal_equal(choice, value, vm)),
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

    /// Writes `value` by its runtime type, as pydantic's `Any` serializer does,
    /// under its recursion guard: past [`RECURSION_LIMIT`] inferred values
    /// down, the value is kept in Python mode and an error in JSON mode.
    fn infer<S: Sink>(&mut self, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        self.step(vm)?;
        if self.depth >= RECURSION_LIMIT {
            // The guard checks for a cycle before the depth.
            let reason = match value {
                Value::Ref(id) if self.inferring.contains(id) => "id repeated",
                _ => "depth exceeded",
            };
            self.recursion(value, reason, sink, vm)
        } else {
            self.depth += 1;
            let written = self.infer_type(value, sink, vm);
            self.depth -= 1;
            written
        }
    }

    /// [`Self::infer`]'s dispatch on the runtime type.
    fn infer_type<S: Sink>(&mut self, value: &Value, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        match value.as_str(vm) {
            Some(text) => sink.str(value, text, vm),
            None => match value {
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
                    HeapData::Set(set) => self.seq(*id, SeqKind::Set, set.iter(), None, sink, vm),
                    HeapData::FrozenSet(set) => self.seq(*id, SeqKind::FrozenSet, set.storage().iter(), None, sink, vm),
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
                    HeapData::Complex(complex) => match self.options.mode {
                        Mode::Python => sink.keep(value, vm),
                        Mode::Json => sink.text_form(value, complex_text(complex.real, complex.imag), vm),
                    },
                    _ => self.unknown(value, sink, vm),
                },
                _ => self.unknown(value, sink, vm),
            },
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

    /// Counts one value written, polling the memory and time limits every so often.
    fn step(&mut self, vm: &VM<'_>) -> RunResult<()> {
        self.steps = self.steps.wrapping_add(1);
        Ok(vm.heap.tracker.check_memory_time_every(self.steps)?)
    }

    /// Writes container `id` through `body`. pydantic guards an inferred
    /// container, not a typed one, so only an inferred container met again
    /// inside itself stops, as [`Self::recursion`] does.
    fn container<S: Sink>(
        &mut self,
        id: HeapId,
        inferred: bool,
        sink: &mut S,
        vm: &VM<'_>,
        body: impl FnOnce(&mut Self, &mut S) -> RunResult<()>,
    ) -> RunResult<()> {
        if !inferred {
            body(self, sink)
        } else if self.inferring.insert(id) {
            let written = body(self, sink);
            self.inferring.remove(&id);
            written
        } else {
            self.recursion(&borrowed_ref(id), "id repeated", sink, vm)
        }
    }

    /// pydantic's inference guard stopping at `value`: Python mode keeps the
    /// value as it is, JSON mode fails.
    fn recursion<S: Sink>(&self, value: &Value, reason: &str, sink: &mut S, vm: &VM<'_>) -> RunResult<()> {
        match self.options.mode {
            Mode::Python => sink.keep(value, vm),
            Mode::Json => Err(self.circular(reason)),
        }
    }

    /// pydantic's recursion guard error, a `ValueError` that `dump_json` wraps.
    fn circular(&self, reason: &str) -> RunError {
        self.surface
            .wrap(ExcType::value_error(format!("Circular reference detected ({reason})")))
    }

    /// A sequence of `kind`, each item by `item` or by inference.
    fn seq<'v, S: Sink>(
        &mut self,
        id: HeapId,
        kind: SeqKind,
        items: impl IntoIterator<Item = &'v Value>,
        item: Option<ValidatorId>,
        sink: &mut S,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        self.container(id, item.is_none(), sink, vm, |this, sink| {
            sink.begin_seq(kind)?;
            for value in items {
                sink.element()?;
                this.item(item, value, sink, vm)?;
            }
            sink.end_seq()
        })
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
        self.container(id, false, sink, vm, |this, sink| {
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
                this.item(validator, value, sink, vm)?;
            }
            sink.end_seq()
        })
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
        self.container(id, validators.is_none(), sink, vm, |this, sink| {
            sink.begin_map()?;
            for (key, value) in dict {
                this.key(validators.map(|(key, _)| key), key, sink, vm)?;
                this.item(validators.map(|(_, value)| value), value, sink, vm)?;
            }
            sink.end_map()
        })
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
        self.defined(id, sink, |this, sink| {
            sink.begin_map()?;
            for field in &dataclass.fields {
                let name = vm.interns.get_str(field.name);
                let Some(value) = instance.attrs().get_by_str(name, vm.heap, vm.interns) else {
                    continue;
                };
                if this.options.exclude_none && matches!(value, Value::None) {
                    continue;
                }
                sink.key(Key::Text(name), vm)?;
                this.value(field.validator, value, sink, vm)?;
            }
            sink.end_map()
        })
    }

    /// Writes a dataclass by its validator through `body`, under the guard
    /// pydantic puts on a recursive type: it fails in either mode, and counts
    /// towards the depth inference shares.
    fn defined<S: Sink>(
        &mut self,
        id: HeapId,
        sink: &mut S,
        body: impl FnOnce(&mut Self, &mut S) -> RunResult<()>,
    ) -> RunResult<()> {
        if self.defining.contains(&id) {
            Err(self.circular("id repeated"))
        } else if self.depth >= RECURSION_LIMIT {
            Err(self.circular("depth exceeded"))
        } else {
            self.defining.insert(id);
            self.depth += 1;
            let written = body(self, sink);
            self.depth -= 1;
            self.defining.remove(&id);
            written
        }
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
            return self.unknown(&borrowed_ref(id), sink, vm);
        };
        self.container(id, true, sink, vm, |this, sink| {
            sink.begin_map()?;
            for (name, _) in fields {
                let Some(name) = name.as_str(vm) else { continue };
                let Some(value) = instance.attrs().get_by_str(name, vm.heap, vm.interns) else {
                    continue;
                };
                sink.key(Key::Text(name), vm)?;
                this.infer(value, sink, vm)?;
            }
            sink.end_map()
        })
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
                sink.key(Key::Text(&float_key(*f, InfNan::Constants)), vm)
            }
            (Mode::Json, _) => {
                let text = self.key_text(key, vm)?;
                sink.key(Key::Text(&text), vm)
            }
        }
    }

    /// pydantic's JSON object key for a value: strings as they are, `None`
    /// for `None` and non-finite floats, `true` / `false`, a number's text,
    /// ISO text for temporal values, a tuple's keys joined by commas.
    fn key_text(&mut self, key: &Value, vm: &VM<'_>) -> RunResult<String> {
        // A tuple key shared within itself expands once per reference, so keys count as steps too.
        self.step(vm)?;
        match key.as_str(vm) {
            Some(text) => Ok(text.to_owned()),
            None => match key {
                Value::None => Ok("None".to_owned()),
                Value::Bool(b) => Ok(bool_text(*b).to_owned()),
                Value::Int(i) => Ok(i.to_string()),
                Value::Float(f) => Ok(float_key(*f, InfNan::Null)),
                Value::InternBytes(id) => self.surface.utf8_key(vm.interns.get_bytes(*id)).map(str::to_owned),
                Value::InternLongInt(id) => self.surface.bigint_key(vm.interns.get_long_int(*id)),
                Value::Ref(id) => match vm.heap.get(*id) {
                    HeapData::Bytes(bytes) => self.surface.utf8_key(bytes.as_slice()).map(str::to_owned),
                    HeapData::LongInt(big) => self.surface.bigint_key(big.inner()),
                    HeapData::Decimal(decimal) => Ok(decimal.to_sci_string(false, self.options.capitals)),
                    HeapData::Complex(complex) => Ok(complex_text(complex.real, complex.imag)),
                    HeapData::Date(_) | HeapData::DateTime(_) | HeapData::Time(_) | HeapData::TimeDelta(_) => {
                        temporal_text(*id, vm).ok_or_else(|| Self::unknown_type(key, vm))
                    }
                    HeapData::Tuple(tuple) => self.joined_keys(tuple.as_slice(), vm),
                    HeapData::NamedTuple(nt) => self.joined_keys(nt.items(), vm),
                    HeapData::FrozenSet(_) => Err(self.surface.invalid_key("frozenset")),
                    _ => Err(Self::unknown_type(key, vm)),
                },
                _ => Err(Self::unknown_type(key, vm)),
            },
        }
    }

    /// A tuple key's items as keys, comma-separated.
    fn joined_keys(&mut self, items: &[Value], vm: &VM<'_>) -> RunResult<String> {
        let parts: Vec<String> = items
            .iter()
            .map(|item| self.key_text(item, vm))
            .collect::<RunResult<_>>()?;
        Ok(parts.join(","))
    }
}

/// A `PydanticSerializationError` with `message`.
fn serialization_error(message: String) -> RunError {
    SimpleException::new_msg(ExcType::PydanticSerializationError, message).into()
}

/// A `Value` naming heap entry `id` without owning a reference, for calls
/// that only read it; never dropped, so it releases nothing.
fn borrowed_ref(id: HeapId) -> ManuallyDrop<Value> {
    ManuallyDrop::new(Value::Ref(id))
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

/// The float JSON mode writes for an int or bool under a `float` hint; `None`
/// for any other value, and an `OverflowError` for an int past `f64`'s range.
fn int_as_float(value: &Value, vm: &VM<'_>) -> Option<RunResult<f64>> {
    match value {
        Value::Bool(b) => Some(Ok(f64::from(u8::from(*b)))),
        Value::Int(i) => Some(Ok(*i as f64)),
        Value::InternLongInt(id) => Some(big_as_float(vm.interns.get_long_int(*id))),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::LongInt(big) => Some(big_as_float(big.inner())),
            _ => None,
        },
        _ => None,
    }
}

fn big_as_float(big: &BigInt) -> RunResult<f64> {
    big.to_f64()
        .filter(|float| float.is_finite())
        .ok_or_else(ExcType::overflow_int_to_float)
}

/// Whether `value` is the `Literal` choice `choice`: the same type and value,
/// as the literal validator matches exactly.
fn literal_equal(choice: &Value, value: &Value, vm: &VM<'_>) -> bool {
    match (choice, value) {
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::None, Value::None) => true,
        (Value::Ref(a), Value::Ref(b)) if a == b => true,
        _ => match (choice.as_str(vm), value.as_str(vm)) {
            (Some(a), Some(b)) => a == b,
            _ => bytes_of(choice, vm)
                .zip(bytes_of(value, vm))
                .is_some_and(|(a, b)| a == b),
        },
    }
}

/// The contents of a `bytes` value.
fn bytes_of<'v>(value: &Value, vm: &'v VM<'_>) -> Option<&'v [u8]> {
    match value {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        },
        _ => None,
    }
}

/// The ISO text of a temporal heap value; `None` for a datetime whose
/// components are out of range for a snapshot.
fn temporal_text(id: HeapId, vm: &VM<'_>) -> Option<String> {
    match vm.heap.read(id) {
        HeapReadOutput::Date(d) => Some(iso::date(&date_type::to_monty_date(*d.get(vm.heap)))),
        HeapReadOutput::DateTime(dt) => datetime_type::to_monty_datetime(dt.get(vm.heap)).map(|dt| iso::datetime(&dt)),
        HeapReadOutput::Time(t) => Some(iso::time(&time_type::to_monty_time(t.get(vm.heap), vm.heap))),
        HeapReadOutput::TimeDelta(td) => Some(iso::timedelta(&timedelta_type::to_monty_timedelta(td.get(vm.heap)))),
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
                    // Each half is guarded while the other builds, so a failure releases both.
                    let mut value = DropGuard::new(value, vm);
                    let key = key.build(value.ctx())?;
                    let (value, vm) = value.into_parts();
                    let mut key = DropGuard::new(key, vm);
                    let value = value.build(key.ctx())?;
                    let (key, vm) = key.into_parts();
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
            Mode::Json => allocate_string(Surface::Python.utf8_value(bytes)?, vm.heap),
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
        self.writer.string(Surface::DumpJson.utf8_value(bytes)?)
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
