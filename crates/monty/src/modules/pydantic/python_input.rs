//! [`Input`] for a live sandbox value: pydantic-core's Python-mode coercions
//! (`input/input_python.rs`) over Monty's heap types.
//!
//! A `PyInput` owns one reference to its value, so a sequence's items and a
//! mapping's entries are taken as new references and released after each
//! is validated. Lists, tuples and dicts are read in place, one step at a
//! time, since a validator may run Python (a `default_factory`,
//! `__post_init__`) that mutates them; [`PySequence`] and [`PyMapping`] say
//! what each tolerates.

use std::{
    mem,
    str::{self, Utf8Error},
};

use monty_types::{FormatFloat, ValidationInputType};
use num_traits::ToPrimitive;

use crate::{
    args::ArgValues,
    bytecode::VM,
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId},
    modules::pydantic::{
        catalogue::ErrorType,
        coerce::{
            bigint_value, bytes_as_date, bytes_as_timedelta, datetime_value, float_as_datetime, float_as_duration,
            float_as_int, float_as_time, int_as_bool, int_as_datetime, int_as_duration, int_as_time, parse_datetime,
            parse_time, str_as_bool, str_as_float, str_as_int, time_value,
        },
        errors::{ValError, ValResult},
        input::{ArgumentsShape, Input, InputKind, Mapping, Match, SeqKind, Sequence},
    },
    types::{
        allocate_string,
        bytes::allocate_bytes,
        date as date_type, datetime as datetime_type,
        decimal::{self as decimal_type, py as py_decimal},
        hash_table::{TableCursor, TableScan},
    },
    value::Value,
};

/// One Python value under validation; owns its reference.
pub(crate) struct PyInput {
    value: Value,
}

impl PyInput {
    /// Takes ownership of `value`.
    pub(crate) fn new(value: Value) -> Self {
        Self { value }
    }

    /// The text of a `str` input, or of a `bytes` input decoded as UTF-8:
    /// `Some(Err(_))` for bytes that are not UTF-8, `None` for any other type.
    fn text<'s>(&'s self, vm: &'s VM<'_>) -> Option<Result<&'s str, Utf8Error>> {
        match self.value.as_str(vm) {
            Some(text) => Some(Ok(text)),
            None => self.bytes_slice(vm).map(str::from_utf8),
        }
    }

    /// A lax coercion's result as a match, or its error on this input.
    fn lax(&mut self, result: Result<Value, ErrorType>, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        match result {
            Ok(value) => Ok(Match::lax(value)),
            Err(err) => self.error(err, vm),
        }
    }

    /// The bytes of a `bytes` input.
    fn bytes_slice<'a>(&'a self, vm: &'a VM<'_>) -> Option<&'a [u8]> {
        match &self.value {
            Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Bytes(bytes) => Some(bytes.as_slice()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The heap entry this input names, if it is a heap value.
    fn heap_data<'a>(&self, vm: &'a VM<'_>) -> Option<&'a HeapData> {
        match &self.value {
            Value::Ref(id) => Some(vm.heap.get(*id)),
            _ => None,
        }
    }

    /// A new reference to the value, for passing it through unchanged.
    fn clone_value(&self, vm: &VM<'_>) -> Value {
        self.value.clone_with_heap(vm.heap)
    }

    /// An `int` or `bool` as `i64`, when the input is one.
    fn as_int(&self, vm: &VM<'_>) -> Option<i64> {
        match &self.value {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::InternLongInt(id) => vm.interns.get_long_int(*id).try_into().ok(),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => li.inner().try_into().ok(),
                _ => None,
            },
            _ => None,
        }
    }

    /// A number as `f64` when the input is an int, bool or float.
    fn as_f64(&self, vm: &VM<'_>) -> Option<f64> {
        match &self.value {
            Value::Int(i) => Some(*i as f64),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Float(f) => Some(*f),
            Value::InternLongInt(id) => vm.interns.get_long_int(*id).to_f64(),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => li.inner().to_f64(),
                _ => None,
            },
            _ => None,
        }
    }

    /// The bytes of a `str` or `bytes` input, which the temporal parsers read.
    fn temporal_bytes<'s>(&'s self, vm: &'s VM<'_>) -> Option<&'s [u8]> {
        match self.value.as_str(vm) {
            Some(text) => Some(text.as_bytes()),
            None => self.bytes_slice(vm),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for PyInput {
    fn drop_with(self, ctx: &mut C) {
        self.value.drop_with(ctx);
    }
}

impl Input for PyInput {
    type Seq = PySequence;
    type Map = PyMapping;

    const INPUT_TYPE: ValidationInputType = ValidationInputType::Python;

    fn to_value(&mut self, vm: &mut VM<'_>) -> ValResult<Value> {
        Ok(self.clone_value(vm))
    }

    fn is_none(&self) -> bool {
        matches!(self.value, Value::None)
    }

    fn validate_bool(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<bool>> {
        if let Value::Bool(b) = self.value {
            return Ok(Match::exact(b));
        }
        if !strict {
            if let Some(text) = self.text(vm) {
                return match text.map_or(Err(ErrorType::BoolParsing {}), str_as_bool) {
                    Ok(b) => Ok(Match::lax(b)),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Some(int) = self.as_int(vm) {
                return match int_as_bool(int) {
                    Ok(b) => Ok(Match::lax(b)),
                    Err(err) => self.error(err, vm),
                };
            }
            // A whole float is read as its integer; a fraction is not a boolean at all.
            if let Value::Float(f) = self.value
                && let Ok(int) = float_as_int(f)
            {
                return match int_as_bool(int) {
                    Ok(b) => Ok(Match::lax(b)),
                    Err(err) => self.error(err, vm),
                };
            }
        }
        self.error(ErrorType::BoolType {}, vm)
    }

    fn validate_int(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        match &self.value {
            Value::Int(_) | Value::InternLongInt(_) => return Ok(Match::exact(self.clone_value(vm))),
            Value::Bool(b) => {
                return if strict {
                    self.error(ErrorType::IntType {}, vm)
                } else {
                    Ok(Match::lax(Value::Int(i64::from(*b))))
                };
            }
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => {
                return Ok(Match::exact(self.clone_value(vm)));
            }
            _ => {}
        }
        if !strict {
            if let Some(text) = self.text(vm) {
                let parsed = text.map_or(Err(ErrorType::IntParsing {}), |text| str_as_int(text, vm.heap));
                return self.lax(parsed, vm);
            }
            if let Value::Float(f) = self.value {
                return match float_as_int(f) {
                    Ok(int) => Ok(Match::lax(Value::Int(int))),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Some(decimal) = py_decimal::as_decimal(&self.value, vm) {
                // A finite, integral decimal is its integer; anything else is a fraction.
                return if !decimal.is_finite() {
                    self.error(ErrorType::FiniteNumber {}, vm)
                } else if decimal.is_integer() {
                    let int = decimal.int();
                    Ok(Match::lax(bigint_value(int, vm.heap)))
                } else {
                    self.error(ErrorType::IntFromFloat {}, vm)
                };
            }
        }
        self.error(ErrorType::IntType {}, vm)
    }

    fn validate_float(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<f64>> {
        if let Value::Float(f) = self.value {
            return Ok(Match::exact(f));
        }
        if !strict && let Some(text) = self.text(vm) {
            return match text.map_or(Err(ErrorType::FloatParsing {}), str_as_float) {
                Ok(f) => Ok(Match::lax(f)),
                Err(err) => self.error(err, vm),
            };
        }
        if let Some(f) = self.as_f64(vm) {
            return if matches!(self.value, Value::Bool(_)) {
                if strict {
                    self.error(ErrorType::FloatType {}, vm)
                } else {
                    Ok(Match::lax(f))
                }
            } else {
                Ok(Match::strict(f))
            };
        }
        self.error(ErrorType::FloatType {}, vm)
    }

    fn validate_str(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if self.value.as_str(vm).is_some() {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict && let Some(bytes) = self.bytes_slice(vm) {
            return match str::from_utf8(bytes) {
                Ok(text) => Ok(Match::lax(allocate_string(text.to_owned(), vm.heap))),
                Err(_) => self.error(ErrorType::StringUnicode {}, vm),
            };
        }
        self.error(ErrorType::StringType {}, vm)
    }

    fn validate_bytes(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if self.bytes_slice(vm).is_some() {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict && let Some(text) = self.value.as_str(vm) {
            return Ok(Match::lax(allocate_bytes(text.as_bytes().to_vec(), vm.heap)));
        }
        self.error(ErrorType::BytesType {}, vm)
    }

    fn validate_datetime(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if matches!(self.heap_data(vm), Some(HeapData::DateTime(_))) {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict {
            if let Some(bytes) = self.temporal_bytes(vm) {
                let parsed = parse_datetime(bytes);
                let value = parsed.and_then(|dt| datetime_value(&dt, vm));
                return self.lax(value, vm);
            }
            if matches!(self.value, Value::Bool(_)) {
                return self.error(ErrorType::DatetimeType {}, vm);
            }
            if let Some(int) = self.as_int(vm) {
                return self.lax(int_as_datetime(int, 0, vm), vm);
            }
            if let Value::Float(f) = self.value {
                return self.lax(float_as_datetime(f, vm), vm);
            }
            if let Some(HeapData::Date(date)) = self.heap_data(vm) {
                let (year, month, day) = date_type::to_ymd(*date);
                let (month, day) = (i32::try_from(month), i32::try_from(day));
                let (month, day) = (month.expect("month is 1..=12"), day.expect("day is 1..=31"));
                let value = datetime_type::allocate_parsed(year, month, day, 0, 0, 0, 0, None, vm.heap)
                    .expect("a date's components are in range");
                return Ok(Match::lax(value));
            }
        }
        self.error(ErrorType::DatetimeType {}, vm)
    }

    fn validate_date(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        match self.heap_data(vm) {
            Some(HeapData::Date(_)) => return Ok(Match::exact(self.clone_value(vm))),
            Some(HeapData::DateTime(_)) => return self.error(ErrorType::DateType {}, vm),
            _ => {}
        }
        if !strict && let Some(bytes) = self.temporal_bytes(vm) {
            let parsed = bytes_as_date(bytes, vm.heap);
            return self.lax(parsed, vm);
        }
        self.error(ErrorType::DateType {}, vm)
    }

    fn validate_time(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if matches!(self.heap_data(vm), Some(HeapData::Time(_))) {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict {
            if let Some(bytes) = self.temporal_bytes(vm) {
                let parsed = parse_time(bytes);
                let value = parsed.map(|time| time_value(&time, vm));
                return self.lax(value, vm);
            }
            if matches!(self.value, Value::Bool(_)) {
                return self.error(ErrorType::TimeType {}, vm);
            }
            if let Some(int) = self.as_int(vm) {
                return self.lax(int_as_time(int, 0, vm), vm);
            }
            if let Value::Float(f) = self.value {
                return self.lax(float_as_time(f, vm), vm);
            }
        }
        self.error(ErrorType::TimeType {}, vm)
    }

    fn validate_timedelta(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if matches!(self.heap_data(vm), Some(HeapData::TimeDelta(_))) {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict {
            if let Some(bytes) = self.temporal_bytes(vm) {
                let parsed = bytes_as_timedelta(bytes, vm.heap);
                return self.lax(parsed, vm);
            }
            if let Some(int) = self.as_int(vm) {
                return self.lax(int_as_duration(int, vm.heap), vm);
            }
            if let Value::Float(f) = self.value {
                return self.lax(float_as_duration(f, vm.heap), vm);
            }
        }
        self.error(ErrorType::TimeDeltaType {}, vm)
    }

    fn validate_decimal(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if py_decimal::as_decimal(&self.value, vm).is_some() {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if strict {
            return self.error(
                ErrorType::IsInstanceOf {
                    class: "Decimal".to_owned(),
                },
                vm,
            );
        }
        let parsed = if let Some(text) = self.value.as_str(vm) {
            Some(decimal_type::parse::parse_str(text).map(Result::ok))
        } else {
            match &self.value {
                Value::Bool(_) => None,
                Value::Int(i) => Some(Ok(Some(decimal_type::Decimal::from_i64(*i)))),
                // Through the float's repr, as pydantic's `Decimal(str(value))` does.
                Value::Float(f) => Some(decimal_type::parse::parse_str(&FormatFloat(*f).to_string()).map(Result::ok)),
                Value::InternLongInt(id) => Some(decimal_type::parse::from_int(vm.interns.get_long_int(*id)).map(Some)),
                Value::Ref(id) => match vm.heap.get(*id) {
                    HeapData::LongInt(li) => Some(decimal_type::parse::from_int(li.inner()).map(Some)),
                    // `Decimal((sign, digits, exponent))`, through the constructor's own checks.
                    HeapData::Tuple(tuple) if tuple.as_slice().len() == 3 => {
                        let arg = self.clone_value(vm);
                        Some(match py_decimal::init(vm, ArgValues::One(arg)) {
                            Ok(value) => return Ok(Match::lax(value)),
                            Err(_) => Ok(None),
                        })
                    }
                    _ => None,
                },
                _ => None,
            }
        };
        match parsed {
            Some(Ok(Some(decimal))) => Ok(Match::lax(py_decimal::decimal_value(decimal, vm))),
            Some(Ok(None)) => self.error(ErrorType::DecimalParsing {}, vm),
            Some(Err(err)) => Err(ValError::Run(err)),
            None => self.error(ErrorType::DecimalType {}, vm),
        }
    }

    fn validate_sequence(&mut self, kind: SeqKind, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<PySequence>> {
        let (actual, seq) = match self.heap_data(vm) {
            Some(HeapData::List(list)) => (SeqKind::List, PySequence::indexed(self, list.as_slice().len(), vm)),
            Some(HeapData::Tuple(tuple)) => (SeqKind::Tuple, PySequence::indexed(self, tuple.as_slice().len(), vm)),
            Some(HeapData::NamedTuple(tuple)) => (SeqKind::Tuple, PySequence::indexed(self, tuple.items().len(), vm)),
            Some(HeapData::Set(set)) => (SeqKind::Set, PySequence::items(set.iter(), vm)),
            Some(HeapData::FrozenSet(set)) => (SeqKind::FrozenSet, PySequence::items(set.storage().iter(), vm)),
            Some(HeapData::Deque(deque)) => (SeqKind::List, PySequence::items(deque.iter(), vm)),
            _ => return self.error(sequence_error(kind), vm),
        };
        let exact = actual == kind || (kind == SeqKind::List && matches!(self.heap_data(vm), Some(HeapData::List(_))));
        if !exact && strict {
            seq.drop_with(vm);
            return self.error(sequence_error(kind), vm);
        }
        Ok(if exact { Match::exact(seq) } else { Match::lax(seq) })
    }

    fn validate_dict(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<PyMapping>> {
        match self.heap_data(vm) {
            Some(HeapData::Dict(dict)) => Ok(Match::exact(PyMapping::guarded(self, dict.len(), vm))),
            _ => self.error(ErrorType::DictType {}, vm),
        }
    }

    fn validate_dataclass_args(&mut self, class_name: &str, vm: &mut VM<'_>) -> ValResult<PyMapping> {
        match self.heap_data(vm) {
            Some(HeapData::Dict(_)) => Ok(PyMapping::tolerant(self, vm)),
            _ => self.error(
                ErrorType::DataclassType {
                    class_name: class_name.to_owned(),
                },
                vm,
            ),
        }
    }

    fn instance_of(&self, class_id: HeapId, vm: &VM<'_>) -> Option<Value> {
        match self.heap_data(vm) {
            Some(HeapData::Instance(instance)) if instance.class() == class_id => Some(self.clone_value(vm)),
            _ => None,
        }
    }

    fn attempt<R>(
        &mut self,
        vm: &mut VM<'_>,
        attempt: impl FnOnce(&mut Self, &mut VM<'_>) -> ValResult<R>,
    ) -> ValResult<R> {
        attempt(self, vm)
    }

    fn kind(&self) -> InputKind {
        match &self.value {
            Value::None => InputKind::Null,
            Value::Bool(_) => InputKind::Bool,
            Value::Int(_) | Value::InternLongInt(_) => InputKind::Int,
            Value::Float(_) => InputKind::Float,
            Value::InternString(_) | Value::InlineString(_) => InputKind::Str,
            _ => InputKind::Other,
        }
    }

    fn arguments_shape(&self, vm: &VM<'_>) -> ArgumentsShape {
        match self.heap_data(vm) {
            Some(HeapData::Dict(_)) => ArgumentsShape::Keywords,
            Some(HeapData::List(_) | HeapData::Tuple(_) | HeapData::NamedTuple(_)) => ArgumentsShape::Positional,
            _ => ArgumentsShape::Other,
        }
    }

    fn key_value(&self) -> Option<&Value> {
        Some(&self.value)
    }

    fn heap_id(&self) -> Option<HeapId> {
        match &self.value {
            Value::Ref(id) => Some(*id),
            _ => None,
        }
    }
}

/// The type error for a sequence of `kind`.
fn sequence_error(kind: SeqKind) -> ErrorType {
    match kind {
        SeqKind::List => ErrorType::ListType {},
        SeqKind::Tuple => ErrorType::TupleType {},
        SeqKind::Set => ErrorType::SetType {},
        SeqKind::FrozenSet => ErrorType::FrozenSetType {},
    }
}

/// The items of a sequence input, handed out one at a time.
pub(crate) enum PySequence {
    /// A list or tuple read in place by index, so its items are not copied up
    /// front; `len`, the length when validation began, bounds the walk as
    /// pydantic-core's does, so a validator that appends cannot prolong it.
    Indexed { container: Value, len: usize, index: usize },
    /// A set, frozenset or deque, which has no index to read by: a snapshot.
    Items { items: Vec<Value>, index: usize },
}

impl PySequence {
    /// Reads `input`'s container in place, holding a reference to it.
    fn indexed(input: &PyInput, len: usize, vm: &VM<'_>) -> Self {
        Self::Indexed {
            container: input.clone_value(vm),
            len,
            index: 0,
        }
    }

    /// Snapshots `items`, taking a reference to each.
    fn items<'v>(items: impl Iterator<Item = &'v Value>, vm: &VM<'_>) -> Self {
        Self::Items {
            items: items.map(|item| item.clone_with_heap(vm.heap)).collect(),
            index: 0,
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for PySequence {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Indexed { container, .. } => container.drop_with(ctx),
            Self::Items { items, .. } => items.drop_with(ctx),
        }
    }
}

impl Sequence<PyInput> for PySequence {
    fn len(&self) -> Option<usize> {
        Some(match self {
            Self::Indexed { len, .. } => *len,
            Self::Items { items, .. } => items.len(),
        })
    }

    fn next(&mut self, vm: &mut VM<'_>) -> ValResult<Option<PyInput>> {
        let item = match self {
            Self::Indexed { container, len, index } => {
                let Value::Ref(id) = container else {
                    unreachable!("an indexed sequence is a heap container")
                };
                // Also bounded by the current length: a validator may have run Python that shrank it.
                let item = match vm.heap.get(*id) {
                    _ if *index >= *len => None,
                    HeapData::List(list) => list.as_slice().get(*index),
                    HeapData::Tuple(tuple) => tuple.as_slice().get(*index),
                    HeapData::NamedTuple(tuple) => tuple.items().get(*index),
                    _ => unreachable!("an indexed sequence is a list or tuple"),
                };
                let item = item.map(|item| item.clone_with_heap(vm.heap));
                *index += usize::from(item.is_some());
                item
            }
            Self::Items { items, index } => {
                let item = items.get_mut(*index).map(|slot| mem::replace(slot, Value::None));
                *index += usize::from(item.is_some());
                item
            }
        };
        Ok(item.map(PyInput::new))
    }

    fn finish_item(&mut self, item: PyInput, vm: &mut VM<'_>) {
        item.drop_with(vm);
    }
}

/// The entries of a dict input, read in place one at a time.
pub(crate) struct PyMapping {
    /// The dict; an OWNED reference.
    dict: Value,
    walk: DictWalk,
}

/// How a [`PyMapping`] steps through a dict a validator may have mutated.
enum DictWalk {
    /// For `dict[K, V]`: a size change raises as `for` over the dict would.
    Guarded(TableCursor),
    /// For dataclass arguments, which pydantic-core looks up rather than
    /// iterates and so validates whatever a `default_factory` did to the dict.
    Tolerant(TableScan),
}

impl PyMapping {
    /// Reads `input`'s dict of `len` entries in place, raising if its size changes.
    fn guarded(input: &PyInput, len: usize, vm: &VM<'_>) -> Self {
        Self {
            dict: input.clone_value(vm),
            walk: DictWalk::Guarded(TableCursor::new(len)),
        }
    }

    /// Reads `input`'s dict in place, tolerating mutation between steps.
    fn tolerant(input: &PyInput, vm: &VM<'_>) -> Self {
        Self {
            dict: input.clone_value(vm),
            walk: DictWalk::Tolerant(TableScan::default()),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for PyMapping {
    fn drop_with(self, ctx: &mut C) {
        self.dict.drop_with(ctx);
    }
}

impl Mapping<PyInput> for PyMapping {
    fn next(&mut self, vm: &mut VM<'_>) -> ValResult<Option<(PyInput, PyInput)>> {
        let Value::Ref(id) = self.dict else {
            unreachable!("a mapping input is a dict")
        };
        let HeapData::Dict(dict) = vm.heap.get(id) else {
            unreachable!("a mapping input is a dict")
        };
        let pair = match &mut self.walk {
            DictWalk::Guarded(cursor) => dict.clone_next_pair(cursor, vm.heap).map_err(ValError::Run)?,
            DictWalk::Tolerant(scan) => dict
                .scan_next(scan)
                .map(|(key, value)| (key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap))),
        };
        Ok(pair.map(|(key, value)| (PyInput::new(key), PyInput::new(value))))
    }

    fn finish_entry(&mut self, key: PyInput, value: PyInput, vm: &mut VM<'_>) {
        key.drop_with(vm);
        value.drop_with(vm);
    }
}
