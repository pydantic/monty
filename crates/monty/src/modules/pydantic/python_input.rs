//! [`Input`] for a live sandbox value: pydantic-core's Python-mode coercions
//! (`input/input_python.rs`) over Monty's heap types.
//!
//! A `PyInput` owns one reference to its value, so a sequence's items and a
//! mapping's entries are taken as new references and released after each
//! is validated; the containers themselves are snapshotted before iteration,
//! since a validator may run Python (a `default_factory`, `__post_init__`)
//! that mutates them.

use std::{mem, str};

use monty_types::{FormatFloat, ValidationInputType};
use num_traits::ToPrimitive;

use crate::{
    args::ArgValues,
    bytecode::VM,
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId},
    modules::pydantic::{
        catalogue::ErrorType,
        coerce::{
            bigint_value, bytes_as_date, bytes_as_datetime, bytes_as_time, bytes_as_timedelta, float_as_datetime,
            float_as_duration, float_as_int, float_as_time, int_as_bool, int_as_datetime, int_as_duration, int_as_time,
            str_as_bool, str_as_float, str_as_int,
        },
        errors::{ValError, ValResult},
        input::{Input, InputKind, Mapping, Match, SeqKind, Sequence},
    },
    types::{
        allocate_string,
        bytes::allocate_bytes,
        date as date_type, datetime as datetime_type,
        decimal::{self as decimal_type, py as py_decimal},
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

    /// The text of a `str` input, or the decoded text of a `bytes` input;
    /// `unicode_error` is raised for bytes that are not UTF-8.
    fn maybe_text(&mut self, unicode_error: ErrorType, vm: &mut VM<'_>) -> ValResult<Option<String>> {
        if let Some(text) = self.value.as_str(vm) {
            return Ok(Some(text.to_owned()));
        }
        match self.bytes_slice(vm) {
            Some(bytes) => match str::from_utf8(bytes) {
                Ok(text) => Ok(Some(text.to_owned())),
                Err(_) => self.error(unicode_error, vm),
            },
            None => Ok(None),
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
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => li.inner().to_f64(),
                _ => None,
            },
            _ => None,
        }
    }

    /// The bytes of a `str` or `bytes` input, which the temporal parsers read.
    fn temporal_bytes(&self, vm: &VM<'_>) -> Option<Vec<u8>> {
        match self.value.as_str(vm) {
            Some(text) => Some(text.as_bytes().to_vec()),
            None => self.bytes_slice(vm).map(<[u8]>::to_vec),
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
            if let Some(text) = self.maybe_text(ErrorType::BoolParsing {}, vm)? {
                return match str_as_bool(&text) {
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
            if let Some(text) = self.maybe_text(ErrorType::IntParsing {}, vm)? {
                return match str_as_int(&text, vm.heap) {
                    Ok(int) => Ok(Match::lax(int)),
                    Err(err) => self.error(err, vm),
                };
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
        if !strict && let Some(text) = self.maybe_text(ErrorType::FloatParsing {}, vm)? {
            return match str_as_float(&text) {
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
                return match bytes_as_datetime(&bytes, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if matches!(self.value, Value::Bool(_)) {
                return self.error(ErrorType::DatetimeType {}, vm);
            }
            if let Some(int) = self.as_int(vm) {
                return match int_as_datetime(int, 0, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Value::Float(f) = self.value {
                return match float_as_datetime(f, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
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
            return match bytes_as_date(&bytes, vm.heap) {
                Ok(value) => Ok(Match::lax(value)),
                Err(err) => self.error(err, vm),
            };
        }
        self.error(ErrorType::DateType {}, vm)
    }

    fn validate_time(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        if matches!(self.heap_data(vm), Some(HeapData::Time(_))) {
            return Ok(Match::exact(self.clone_value(vm)));
        }
        if !strict {
            if let Some(bytes) = self.temporal_bytes(vm) {
                return match bytes_as_time(&bytes, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if matches!(self.value, Value::Bool(_)) {
                return self.error(ErrorType::TimeType {}, vm);
            }
            if let Some(int) = self.as_int(vm) {
                return match int_as_time(int, 0, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Value::Float(f) = self.value {
                return match float_as_time(f, vm) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
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
                return match bytes_as_timedelta(&bytes, vm.heap) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Some(int) = self.as_int(vm) {
                return match int_as_duration(int, vm.heap) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
            }
            if let Value::Float(f) = self.value {
                return match float_as_duration(f, vm.heap) {
                    Ok(value) => Ok(Match::lax(value)),
                    Err(err) => self.error(err, vm),
                };
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
        let (actual, items) = match self.heap_data(vm) {
            Some(HeapData::List(list)) => (SeqKind::List, clone_all(list.as_slice(), vm)),
            Some(HeapData::Tuple(tuple)) => (SeqKind::Tuple, clone_all(tuple.as_slice(), vm)),
            Some(HeapData::NamedTuple(tuple)) => (SeqKind::Tuple, clone_all(tuple.items(), vm)),
            Some(HeapData::Set(set)) => (SeqKind::Set, set.iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
            Some(HeapData::FrozenSet(set)) => (
                SeqKind::FrozenSet,
                set.storage().iter().map(|v| v.clone_with_heap(vm.heap)).collect(),
            ),
            Some(HeapData::Deque(deque)) => (
                SeqKind::List,
                deque.iter().map(|v| v.clone_with_heap(vm.heap)).collect(),
            ),
            _ => return self.error(sequence_error(kind), vm),
        };
        let exact = actual == kind || (kind == SeqKind::List && matches!(self.heap_data(vm), Some(HeapData::List(_))));
        if !exact && strict {
            items.drop_with(vm);
            return self.error(sequence_error(kind), vm);
        }
        let seq = PySequence { items, index: 0 };
        Ok(if exact { Match::exact(seq) } else { Match::lax(seq) })
    }

    fn validate_dict(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<PyMapping>> {
        match self.heap_data(vm) {
            Some(HeapData::Dict(dict)) => {
                let pairs = dict
                    .iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect();
                Ok(Match::exact(PyMapping { pairs, index: 0 }))
            }
            _ => self.error(ErrorType::DictType {}, vm),
        }
    }

    fn validate_dataclass_args(&mut self, class_name: &str, vm: &mut VM<'_>) -> ValResult<PyMapping> {
        match self.heap_data(vm) {
            Some(HeapData::Dict(dict)) => {
                let pairs = dict
                    .iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect();
                Ok(PyMapping { pairs, index: 0 })
            }
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

    fn is_mapping(&self, vm: &VM<'_>) -> bool {
        matches!(self.heap_data(vm), Some(HeapData::Dict(_)))
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

/// New references to every item.
fn clone_all(items: &[Value], vm: &VM<'_>) -> Vec<Value> {
    items.iter().map(|item| item.clone_with_heap(vm.heap)).collect()
}

/// A snapshot of a sequence input's items, handed out one at a time.
pub(crate) struct PySequence {
    items: Vec<Value>,
    index: usize,
}

impl<C: ContainsHeap> DropWithContext<C> for PySequence {
    fn drop_with(self, ctx: &mut C) {
        self.items.drop_with(ctx);
    }
}

impl Sequence<PyInput> for PySequence {
    fn len(&self) -> Option<usize> {
        Some(self.items.len())
    }

    fn next(&mut self, _vm: &mut VM<'_>) -> ValResult<Option<PyInput>> {
        if self.index < self.items.len() {
            let value = mem::replace(&mut self.items[self.index], Value::None);
            self.index += 1;
            Ok(Some(PyInput::new(value)))
        } else {
            Ok(None)
        }
    }

    fn finish_item(&mut self, item: PyInput, vm: &mut VM<'_>) {
        item.drop_with(vm);
    }
}

/// A snapshot of a dict input's entries, handed out one at a time.
pub(crate) struct PyMapping {
    pairs: Vec<(Value, Value)>,
    index: usize,
}

impl<C: ContainsHeap> DropWithContext<C> for PyMapping {
    fn drop_with(self, ctx: &mut C) {
        for (key, value) in self.pairs {
            key.drop_with(ctx);
            value.drop_with(ctx);
        }
    }
}

impl Mapping<PyInput> for PyMapping {
    fn next(&mut self, _vm: &mut VM<'_>) -> ValResult<Option<(PyInput, PyInput)>> {
        if self.index < self.pairs.len() {
            let (key, value) = mem::replace(&mut self.pairs[self.index], (Value::None, Value::None));
            self.index += 1;
            Ok(Some((PyInput::new(key), PyInput::new(value))))
        } else {
            Ok(None)
        }
    }

    fn finish_entry(&mut self, key: PyInput, value: PyInput, vm: &mut VM<'_>) {
        key.drop_with(vm);
        value.drop_with(vm);
    }
}
