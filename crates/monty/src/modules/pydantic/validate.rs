//! Validation of an [`Input`] against an [`Arena`] validator, producing the
//! coerced value or the line errors, with pydantic-core's semantics for each
//! validator kind (`validators/*.rs`).
//!
//! Written once over the [`Input`] trait, so the Python and JSON modes share
//! every rule here and differ only in how a value is read. Errors are
//! collected rather than raised at the first failure: a list reports every
//! bad item, a dataclass every bad field.

use monty_types::LocItem;
use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::pydantic::{
        arena::{
            Arena, DataclassValidator, FieldDefault, FieldValidator, LiteralValidator, NamedTupleValidator, Validator,
            ValidatorId,
        },
        catalogue::ErrorType,
        coerce::datetime_is_midnight,
        errors::{LineError, ValError, ValResult},
        input::{Exactness, Input, InputKind, Mapping, SeqKind, Sequence, State},
        snapshot::repr_string,
    },
    types::{
        Dict, Instance, List, NamedTuple, PyTrait, Set, allocate_tuple, date as date_type, datetime as datetime_type,
        decimal::{Decimal, py as py_decimal},
        set::FrozenSet,
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// Validates `input` against the validator `id`.
pub(crate) fn validate<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    // Nesting recurses on the native stack, so a deep input is cut off at the
    // recursion limit rather than overflowing it.
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    match arena.get(id) {
        Validator::Any => {
            // In a union, `Any` is preferred to a lax coercion.
            state.floor_exactness(Exactness::Strict);
            input.to_value(vm)
        }
        Validator::None => {
            // `to_value` consumes a JSON `null`; a Python `None` is free to copy.
            if input.is_none() {
                input.to_value(vm)
            } else {
                input.error(ErrorType::NoneRequired {}, vm)
            }
        }
        Validator::Bool => Ok(Value::Bool(input.validate_bool(state.strict, vm)?.unpack(state))),
        Validator::Int => Ok(input.validate_int(state.strict, vm)?.unpack(state)),
        Validator::Float => Ok(Value::Float(input.validate_float(state.strict, vm)?.unpack(state))),
        Validator::Str => Ok(input.validate_str(state.strict, vm)?.unpack(state)),
        Validator::Bytes => Ok(input.validate_bytes(state.strict, vm)?.unpack(state)),
        Validator::DateTime => validate_datetime(input, state, vm),
        Validator::Date => validate_date(input, state, vm),
        Validator::Time => Ok(input.validate_time(state.strict, vm)?.unpack(state)),
        Validator::TimeDelta => Ok(input.validate_timedelta(state.strict, vm)?.unpack(state)),
        Validator::Decimal => validate_decimal(input, state, vm),
        Validator::List(item) => validate_list(arena, id, *item, input, state, vm),
        Validator::Set(item) => validate_set(arena, id, *item, false, input, state, vm),
        Validator::FrozenSet(item) => validate_set(arena, id, *item, true, input, state, vm),
        Validator::Tuple { items, variadic } => validate_tuple(arena, id, items, *variadic, input, state, vm),
        Validator::Dict { key, value } => validate_dict(arena, id, *key, *value, input, state, vm),
        Validator::Union(choices) => validate_union(arena, choices, input, state, vm),
        Validator::Nullable(inner) => {
            if input.is_none() {
                input.to_value(vm)
            } else {
                validate(arena, *inner, input, state, vm)
            }
        }
        Validator::Literal(literal) => validate_literal(literal, input, vm),
        Validator::Dataclass(dataclass) => validate_dataclass(arena, id, dataclass, input, state, vm),
        Validator::NamedTuple(named_tuple) => validate_named_tuple(named_tuple, input, state, vm),
    }
}

/// A `Decimal`; pydantic refuses a non-finite one unless `allow_inf_nan` is set.
fn validate_decimal<I: Input>(input: &mut I, state: &mut State, vm: &mut VM<'_>) -> ValResult<Value> {
    let decimal = input.validate_decimal(state.strict, vm)?.unpack(state);
    let finite = py_decimal::as_decimal(&decimal, vm).is_some_and(Decimal::is_finite);
    if finite {
        Ok(decimal)
    } else {
        decimal.drop_with(vm);
        input.error(ErrorType::FiniteNumber {}, vm)
    }
}

/// A `datetime`; in lax mode a date (text or value) is accepted at midnight,
/// and a date that fails to parse reports `datetime_from_date_parsing`.
fn validate_datetime<I: Input>(input: &mut I, state: &mut State, vm: &mut VM<'_>) -> ValResult<Value> {
    match input.validate_datetime(state.strict, vm) {
        Ok(matched) => Ok(matched.unpack(state)),
        Err(ValError::Lines(datetime_errors)) if !state.strict && is_parsing_error(&datetime_errors) => {
            state.floor_exactness(Exactness::Lax);
            match input.validate_date(false, vm) {
                Ok(matched) => {
                    datetime_errors.drop_with(vm);
                    let date = matched.value;
                    defer_drop!(date, vm);
                    Ok(midnight(date, vm))
                }
                Err(ValError::Lines(lines))
                    if lines
                        .iter()
                        .any(|line| matches!(line.error_type, ErrorType::DateParsing { .. })) =>
                {
                    datetime_errors.drop_with(vm);
                    Err(ValError::Lines(
                        lines
                            .into_iter()
                            .map(|line| match line.error_type.clone() {
                                ErrorType::DateParsing { error } => {
                                    line.with_type(ErrorType::DatetimeFromDateParsing { error })
                                }
                                _ => line,
                            })
                            .collect(),
                    ))
                }
                Err(ValError::Lines(lines)) => {
                    lines.drop_with(vm);
                    Err(ValError::Lines(datetime_errors))
                }
                Err(other) => {
                    datetime_errors.drop_with(vm);
                    Err(other)
                }
            }
        }
        Err(err) => Err(err),
    }
}

/// A `date`; in lax mode a datetime (text or value) at midnight is accepted
/// and one with a time of day reports `date_from_datetime_inexact`.
fn validate_date<I: Input>(input: &mut I, state: &mut State, vm: &mut VM<'_>) -> ValResult<Value> {
    match input.validate_date(state.strict, vm) {
        Ok(matched) => Ok(matched.unpack(state)),
        Err(ValError::Lines(lines)) if !state.strict => {
            let date_errors = lines;
            state.floor_exactness(Exactness::Lax);
            match input.validate_datetime(false, vm) {
                Ok(matched) => {
                    date_errors.drop_with(vm);
                    let datetime = matched.value;
                    defer_drop!(datetime, vm);
                    if datetime_is_midnight(datetime, vm) {
                        Ok(date_of(datetime, vm))
                    } else {
                        input.error(ErrorType::DateFromDatetimeInexact {}, vm)
                    }
                }
                Err(ValError::Lines(lines)) => {
                    // A datetime parse failure is reported as the date failure it also is.
                    let parsing = lines
                        .iter()
                        .any(|line| matches!(line.error_type, ErrorType::DatetimeParsing { .. }));
                    if parsing {
                        date_errors.drop_with(vm);
                        Err(ValError::Lines(
                            lines
                                .into_iter()
                                .map(|line| match line.error_type.clone() {
                                    ErrorType::DatetimeParsing { error } => {
                                        line.with_type(ErrorType::DateFromDatetimeParsing { error })
                                    }
                                    _ => line,
                                })
                                .collect(),
                        ))
                    } else {
                        lines.drop_with(vm);
                        Err(ValError::Lines(date_errors))
                    }
                }
                Err(other) => {
                    date_errors.drop_with(vm);
                    Err(other)
                }
            }
        }
        Err(err) => Err(err),
    }
}

/// Whether `lines` is a single text-parsing failure, which the lax temporal
/// fallbacks retry as another type.
fn is_parsing_error(lines: &[LineError]) -> bool {
    lines.iter().all(|line| {
        matches!(
            line.error_type,
            ErrorType::DatetimeParsing { .. } | ErrorType::DateParsing { .. }
        )
    })
}

/// `date` as the naive `datetime` at midnight.
fn midnight(date: &Value, vm: &mut VM<'_>) -> Value {
    let Value::Ref(id) = date else {
        unreachable!("validate_date yields a date")
    };
    let HeapData::Date(date) = vm.heap.get(*id) else {
        unreachable!("validate_date yields a date")
    };
    let (year, month, day) = date_type::to_ymd(*date);
    let (month, day) = (i32::try_from(month), i32::try_from(day));
    let (month, day) = (month.expect("month is 1..=12"), day.expect("day is 1..=31"));
    datetime_type::allocate_parsed(year, month, day, 0, 0, 0, 0, None, vm.heap)
        .expect("a date's components are in range")
}

/// A sequence index as a location step.
fn index_loc(index: usize) -> LocItem {
    LocItem::Index(i64::try_from(index).unwrap_or(i64::MAX))
}

/// The `date` of a `datetime` value.
fn date_of(datetime: &Value, vm: &mut VM<'_>) -> Value {
    let Value::Ref(id) = datetime else {
        unreachable!("validate_datetime yields a datetime")
    };
    let HeapData::DateTime(dt) = vm.heap.get(*id) else {
        unreachable!("validate_datetime yields a datetime")
    };
    let (year, month, day, ..) = datetime_type::to_components(dt).expect("a parsed datetime is in range");
    date_type::allocate_ymd(year, i32::from(month), i32::from(day), vm.heap)
}

/// Runs `body` with `(input object, validator)` on the recursion guard, so a
/// container that holds itself reports `recursion_loop` rather than looping.
fn guarded<I: Input, R>(
    id: ValidatorId,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
    body: impl FnOnce(&mut I, &mut State, &mut VM<'_>) -> ValResult<R>,
) -> ValResult<R> {
    let Some(heap_id) = input.heap_id() else {
        return body(input, state, vm);
    };
    if state.recursion.contains(&(heap_id, id)) {
        return input.error(ErrorType::RecursionLoop {}, vm);
    }
    state.recursion.push((heap_id, id));
    let result = body(input, state, vm);
    state.recursion.pop();
    result
}

/// `list[T]`: every item validated, errors located by index.
fn validate_list<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    item: ValidatorId,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    guarded(id, input, state, vm, |input, state, vm| {
        let seq = input.validate_sequence(SeqKind::List, state.strict, vm)?.unpack(state);
        let items = validate_items::<I>(arena, item, seq, state, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
    })
}

/// `set[T]` / `frozenset[T]`: every item validated, then added; an unhashable
/// result is `set_item_not_hashable`.
fn validate_set<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    item: ValidatorId,
    frozen: bool,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    guarded(id, input, state, vm, |input, state, vm| {
        let kind = if frozen { SeqKind::FrozenSet } else { SeqKind::Set };
        let seq = input.validate_sequence(kind, state.strict, vm)?.unpack(state);
        let items = validate_items::<I>(arena, item, seq, state, vm)?;
        defer_drop_mut!(items, vm);
        let mut set = Set::new();
        let mut errors = Vec::new();
        for (index, value) in items.drain(..).enumerate() {
            let probe = value.clone_with_heap(vm.heap);
            match set.add(value, vm) {
                Ok(_) => probe.drop_with(vm),
                Err(_) => errors
                    .push(LineError::owned(ErrorType::SetItemNotHashable {}, probe).with_outer_loc(index_loc(index))),
            }
        }
        if errors.is_empty() {
            let data = if frozen {
                HeapData::FrozenSet(FrozenSet::from_set(set))
            } else {
                HeapData::Set(set)
            };
            Ok(Value::Ref(vm.heap.allocate(data)))
        } else {
            set.drop_with(vm);
            Err(ValError::Lines(errors))
        }
    })
}

/// Validates each item of `seq` against `item`, collecting every error with
/// its index. The output buffer grows under the resource tracker.
fn validate_items<I: Input>(
    arena: &Arena,
    item: ValidatorId,
    mut seq: I::Seq,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Vec<Value>> {
    let mut output = DropGuard::new(Vec::new(), vm);
    let mut errors: Vec<LineError> = Vec::new();
    let mut index = 0usize;
    loop {
        let (output, vm) = output.as_parts_mut();
        let Some(mut element) = seq.next(vm)? else { break };
        vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
        match validate(arena, item, &mut element, state, vm) {
            Ok(value) => {
                let mut value = DropGuard::new(value, vm);
                if let Err(err) = value
                    .ctx()
                    .heap
                    .tracker
                    .check_growth(output.len(), output.capacity(), VALUE_SIZE)
                {
                    let (_, vm) = value.into_parts();
                    seq.finish_item(element, vm);
                    errors.drop_with(vm);
                    return Err(ValError::Run(err.into()));
                }
                output.push(value.into_inner());
            }
            Err(ValError::Lines(lines)) => {
                errors.extend(lines.into_iter().map(|line| line.with_outer_loc(index_loc(index))));
            }
            Err(err @ (ValError::Run(_) | ValError::Json(_))) => {
                seq.finish_item(element, vm);
                errors.drop_with(vm);
                return Err(err);
            }
        }
        seq.finish_item(element, vm);
        index += 1;
    }
    let (output, vm) = output.into_parts();
    if errors.is_empty() {
        Ok(output)
    } else {
        output.drop_with(vm);
        Err(ValError::Lines(errors))
    }
}

/// `tuple[A, B]` and `tuple[A, ...]`: each item validated positionally, the
/// last validator repeating when variadic; `missing` for too few items and
/// `too_long` for too many.
fn validate_tuple<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    items: &[ValidatorId],
    variadic: bool,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    guarded(id, input, state, vm, |input, state, vm| {
        let mut seq = input.validate_sequence(SeqKind::Tuple, state.strict, vm)?.unpack(state);
        let actual_length = seq.len();
        let fixed = if variadic { items.len() - 1 } else { items.len() };
        let mut output = DropGuard::new(Vec::new(), vm);
        let mut errors: Vec<LineError> = Vec::new();
        let mut index = 0usize;
        let mut too_long = false;
        loop {
            let (output, vm) = output.as_parts_mut();
            let Some(mut element) = seq.next(vm)? else { break };
            vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
            let validator = if index < fixed {
                items[index]
            } else if variadic {
                items[fixed]
            } else {
                seq.finish_item(element, vm);
                too_long = true;
                break;
            };
            match validate(arena, validator, &mut element, state, vm) {
                Ok(value) => output.push(value),
                Err(ValError::Lines(lines)) => {
                    errors.extend(lines.into_iter().map(|line| line.with_outer_loc(index_loc(index))));
                }
                Err(err @ (ValError::Run(_) | ValError::Json(_))) => {
                    seq.finish_item(element, vm);
                    errors.drop_with(vm);
                    return Err(err);
                }
            }
            seq.finish_item(element, vm);
            index += 1;
        }
        if too_long {
            let (output, vm) = output.into_parts();
            output.drop_with(vm);
            errors.drop_with(vm);
            // A streamed input reports its length too: the rest is walked for the count.
            let actual_length = match actual_length {
                Some(len) => len,
                None => drain_count::<I>(&mut seq, index + 1, vm)?,
            };
            return input.error(
                ErrorType::TooLong {
                    field_type: "Tuple".to_owned(),
                    max_length: items.len(),
                    actual_length: Some(actual_length),
                },
                vm,
            );
        }
        let (output, vm) = output.into_parts();
        for missing in index..fixed {
            match missing_error(input, missing, vm) {
                Ok(line) => errors.push(line),
                Err(err) => {
                    output.drop_with(vm);
                    errors.drop_with(vm);
                    return Err(err);
                }
            }
        }
        if errors.is_empty() {
            Ok(allocate_tuple(SmallVec::from_vec(output), vm.heap))
        } else {
            output.drop_with(vm);
            Err(ValError::Lines(errors))
        }
    })
}

/// The length of a sequence with `seen` items already read, walking the rest.
fn drain_count<I: Input>(seq: &mut impl Sequence<I>, seen: usize, vm: &mut VM<'_>) -> ValResult<usize> {
    let mut count = seen;
    while let Some(item) = seq.next(vm)? {
        seq.finish_item(item, vm);
        count += 1;
    }
    Ok(count)
}

/// A `missing` error for the tuple position `index`, on the whole input.
fn missing_error<I: Input>(input: &mut I, index: usize, vm: &mut VM<'_>) -> ValResult<LineError> {
    let value = input.to_value(vm)?;
    Ok(LineError::owned(ErrorType::Missing {}, value).with_outer_loc(index_loc(index)))
}

/// `dict[K, V]`: keys and values validated; a key's errors are located at
/// `(key, '[key]')`, a value's at `(key,)`.
fn validate_dict<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    key: ValidatorId,
    value: ValidatorId,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    guarded(id, input, state, vm, |input, state, vm| {
        let mut map = input.validate_dict(state.strict, vm)?.unpack(state);
        let mut output = DropGuard::new(Dict::new(), vm);
        let mut errors: Vec<LineError> = Vec::new();
        let mut index = 0usize;
        loop {
            let (output, vm) = output.as_parts_mut();
            let Some((mut key_input, mut value_input)) = map.next(vm)? else {
                break;
            };
            vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
            index += 1;
            let loc = loc_item(&mut key_input, vm)?;
            let output_key = match validate(arena, key, &mut key_input, state, vm) {
                Ok(k) => Some(k),
                Err(ValError::Lines(lines)) => {
                    errors.extend(lines.into_iter().map(|line| {
                        line.with_outer_loc(LocItem::Key("[key]".to_owned()))
                            .with_outer_loc(loc.clone())
                    }));
                    None
                }
                Err(err @ (ValError::Run(_) | ValError::Json(_))) => {
                    map.finish_entry(key_input, value_input, vm);
                    errors.drop_with(vm);
                    return Err(err);
                }
            };
            let output_value = match validate(arena, value, &mut value_input, state, vm) {
                Ok(v) => Some(v),
                Err(ValError::Lines(lines)) => {
                    errors.extend(lines.into_iter().map(|line| line.with_outer_loc(loc.clone())));
                    None
                }
                Err(err @ (ValError::Run(_) | ValError::Json(_))) => {
                    output_key.drop_with(vm);
                    map.finish_entry(key_input, value_input, vm);
                    errors.drop_with(vm);
                    return Err(err);
                }
            };
            map.finish_entry(key_input, value_input, vm);
            match (output_key, output_value) {
                (Some(k), Some(v)) => {
                    if let Some(old) = output.set(k, v, vm)? {
                        old.drop_with(vm);
                    }
                }
                (k, v) => {
                    k.drop_with(vm);
                    v.drop_with(vm);
                }
            }
        }
        let (output, vm) = output.into_parts();
        if errors.is_empty() {
            Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(output))))
        } else {
            output.drop_with(vm);
            Err(ValError::Lines(errors))
        }
    })
}

/// A dict key as a location step: text and ints as themselves, anything else
/// by its repr, as pydantic's `LocItem::from` does.
fn loc_item<I: Input>(key: &mut I, vm: &mut VM<'_>) -> ValResult<LocItem> {
    let value = key.to_value(vm)?;
    defer_drop!(value, vm);
    Ok(match value {
        Value::Int(index) => LocItem::Index(*index),
        other => match other.as_str(vm) {
            Some(text) => LocItem::Key(text.to_owned()),
            None => LocItem::Key(repr_string(other, vm)),
        },
    })
}

/// pydantic's smart union: every member is tried; an exact match returns at
/// once, otherwise the most exact success wins (more dataclass fields set
/// breaking ties), and with no success every member's errors are reported
/// under the member's name.
fn validate_union<I: Input>(
    arena: &Arena,
    choices: &[ValidatorId],
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    let old_exactness = state.exactness;
    let old_fields_set = state.fields_set_count;
    let mut best: Option<(Value, Exactness, Option<usize>)> = None;
    let mut errors: Vec<LineError> = Vec::new();
    let kind = input.kind();
    for choice in choices {
        if !might_match(arena, *choice, kind) {
            // A member the token kind rules out still contributes its type error.
            let result = input.attempt(vm, |input, vm| {
                let mut probe = State::new(state.strict);
                validate(arena, *choice, input, &mut probe, vm)
            });
            match result {
                Ok(value) => value.drop_with(vm),
                Err(ValError::Lines(lines)) => {
                    if best.is_none() {
                        push_choice_errors(&mut errors, arena, *choice, lines);
                    } else {
                        lines.drop_with(vm);
                    }
                }
                Err(err) => {
                    errors.drop_with(vm);
                    drop_best(&mut best, vm);
                    return Err(err);
                }
            }
            continue;
        }
        state.exactness = Some(Exactness::Exact);
        state.fields_set_count = None;
        let result = input.attempt(vm, |input, vm| validate(arena, *choice, input, state, vm));
        match result {
            Ok(value) => {
                let exactness = state.exactness.unwrap_or(Exactness::Lax);
                let fields_set = state.fields_set_count;
                if exactness == Exactness::Exact && fields_set.is_none() {
                    state.exactness = old_exactness;
                    state.fields_set_count = old_fields_set;
                    errors.drop_with(vm);
                    drop_best(&mut best, vm);
                    return Ok(value);
                }
                let is_best =
                    best.as_ref()
                        .is_none_or(|(_, cur_exactness, cur_fields)| match (*cur_fields, fields_set) {
                            (Some(cur), Some(new)) if cur != new => cur < new,
                            _ => *cur_exactness < exactness,
                        });
                if is_best {
                    drop_best(&mut best, vm);
                    best = Some((value, exactness, fields_set));
                } else {
                    value.drop_with(vm);
                }
            }
            Err(ValError::Lines(lines)) => {
                if best.is_none() {
                    push_choice_errors(&mut errors, arena, *choice, lines);
                } else {
                    lines.drop_with(vm);
                }
            }
            Err(err) => {
                state.exactness = old_exactness;
                state.fields_set_count = old_fields_set;
                errors.drop_with(vm);
                drop_best(&mut best, vm);
                return Err(err);
            }
        }
    }
    state.exactness = old_exactness;
    state.fields_set_count = old_fields_set;
    if let Some((value, exactness, fields_set)) = best {
        errors.drop_with(vm);
        state.floor_exactness(exactness);
        if let Some(count) = fields_set {
            state.add_fields_set(count);
        }
        Ok(value)
    } else {
        Err(ValError::Lines(errors))
    }
}

/// Releases a union's provisional best match.
fn drop_best(best: &mut Option<(Value, Exactness, Option<usize>)>, vm: &mut VM<'_>) {
    if let Some((value, _, _)) = best.take() {
        value.drop_with(vm);
    }
}

/// Whether a validator could accept an input of `kind`, for skipping union
/// members whose token kind rules them out before they run.
fn might_match(arena: &Arena, id: ValidatorId, kind: InputKind) -> bool {
    match (arena.get(id), kind) {
        (_, InputKind::Other) => true,
        (Validator::None, kind) => kind == InputKind::Null,
        (Validator::Nullable(_) | Validator::Union(_) | Validator::Any | Validator::Literal(_), _) => true,
        (Validator::List(_) | Validator::Set(_) | Validator::FrozenSet(_) | Validator::Tuple { .. }, kind) => {
            kind == InputKind::Array
        }
        (Validator::Dict { .. } | Validator::Dataclass(_), kind) => kind == InputKind::Object,
        (Validator::NamedTuple(_), kind) => matches!(kind, InputKind::Array | InputKind::Object),
        (_, InputKind::Null | InputKind::Array | InputKind::Object) => false,
        _ => true,
    }
}

/// Files a failed member's errors under its name.
fn push_choice_errors(errors: &mut Vec<LineError>, arena: &Arena, choice: ValidatorId, lines: Vec<LineError>) {
    let name = arena.name(choice);
    errors.extend(
        lines
            .into_iter()
            .map(|line| line.with_outer_loc(LocItem::Key(name.clone()))),
    );
}

/// `Literal[...]`: the input matched by value and type against the choices,
/// yielding the choice itself.
fn validate_literal<I: Input>(literal: &LiteralValidator, input: &mut I, vm: &mut VM<'_>) -> ValResult<Value> {
    let value = input.to_value(vm)?;
    defer_drop!(value, vm);
    let input_type = value.py_type(vm);
    // An exact-type match first (`True` for `Literal[True]`, `1` for `Literal[1]`),
    // then plain equality, where the last equal choice wins as it does in the
    // dict pydantic looks `1.0` up in: `Literal[1, True]` maps it to `True`.
    for choice in &literal.choices {
        if choice.py_type(vm) == input_type && choice.py_eq(value, vm)? {
            return Ok(choice.clone_with_heap(vm.heap));
        }
    }
    let mut found = None;
    for choice in &literal.choices {
        if choice.py_eq(value, vm)? {
            found = Some(choice);
        }
    }
    if let Some(choice) = found {
        return Ok(choice.clone_with_heap(vm.heap));
    }
    input.error(
        ErrorType::LiteralError {
            expected: literal.expected.clone(),
        },
        vm,
    )
}

/// A dataclass: an instance passes through; a dict is validated field by
/// field into a new instance, then `__post_init__` runs if the class has one.
fn validate_dataclass<I: Input>(
    arena: &Arena,
    id: ValidatorId,
    dataclass: &DataclassValidator,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    let Value::Ref(class_id) = dataclass.class else {
        unreachable!("a dataclass validator holds its class")
    };
    if let Some(instance) = input.instance_of(class_id, vm) {
        return Ok(instance);
    }
    if state.strict && I::INPUT_TYPE == monty_types::ValidationInputType::Python {
        return input.error(
            ErrorType::DataclassExactType {
                class_name: dataclass.name.clone(),
            },
            vm,
        );
    }
    guarded(id, input, state, vm, |input, state, vm| {
        let mut map = input.validate_dataclass_args(&dataclass.name, vm)?;
        let fields = validate_fields(arena, &dataclass.fields, &mut map, input, state, vm)?;
        state.floor_exactness(Exactness::Strict);
        build_instance(class_id, dataclass, fields, vm).map_err(ValError::Run)
    })
}

/// Validates the entries of `map` into one slot per field, in field order:
/// unknown keys are ignored, a key given twice (JSON allows it) keeps its
/// last value, missing fields take their default or report `missing`, and a
/// field's errors are located by its name.
fn validate_fields<I: Input>(
    arena: &Arena,
    fields: &[FieldValidator],
    map: &mut I::Map,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Vec<Value>> {
    let mut slots: Vec<Option<Value>> = (0..fields.len()).map(|_| None).collect();
    let mut errors: Vec<Option<Vec<LineError>>> = (0..fields.len()).map(|_| None).collect();
    let mut index = 0usize;
    loop {
        let Some((mut key, mut value)) = map.next(vm)? else {
            break;
        };
        if let Err(err) = vm.heap.tracker.check_memory_time_every(index) {
            map.finish_entry(key, value, vm);
            release_slots(slots, errors, vm);
            return Err(ValError::Run(err.into()));
        }
        index += 1;
        let key_value = match key.to_value(vm) {
            Ok(k) => k,
            Err(err) => {
                map.finish_entry(key, value, vm);
                release_slots(slots, errors, vm);
                return Err(err);
            }
        };
        let field_index = key_value
            .as_str(vm)
            .and_then(|name| fields.iter().position(|field| vm.interns.get_str(field.name) == name));
        key_value.drop_with(vm);
        if let Some(field_index) = field_index {
            if let Some(earlier) = slots[field_index].take() {
                earlier.drop_with(vm);
            }
            if let Some(earlier) = errors[field_index].take() {
                earlier.drop_with(vm);
            }
            match validate(arena, fields[field_index].validator, &mut value, state, vm) {
                Ok(v) => slots[field_index] = Some(v),
                Err(ValError::Lines(lines)) => errors[field_index] = Some(lines),
                Err(err @ (ValError::Run(_) | ValError::Json(_))) => {
                    map.finish_entry(key, value, vm);
                    release_slots(slots, errors, vm);
                    return Err(err);
                }
            }
        } else if let Err(err) = value.to_value(vm).map(|v| v.drop_with(vm)) {
            // A JSON value must be consumed even when ignored.
            map.finish_entry(key, value, vm);
            release_slots(slots, errors, vm);
            return Err(err);
        }
        map.finish_entry(key, value, vm);
    }
    let fields_set = slots.iter().flatten().count();
    // Defaults for the unset fields, in field order; a factory may run Python.
    let mut line_errors: Vec<LineError> = Vec::new();
    for (field_index, field) in fields.iter().enumerate() {
        if let Some(lines) = errors[field_index].take() {
            let name = vm.interns.get_str(field.name).to_owned();
            line_errors.extend(
                lines
                    .into_iter()
                    .map(|line| line.with_outer_loc(LocItem::Key(name.clone()))),
            );
            continue;
        }
        if slots[field_index].is_some() {
            continue;
        }
        match &field.default {
            FieldDefault::Value(value) => slots[field_index] = Some(value.clone_with_heap(vm.heap)),
            FieldDefault::Factory(factory) => match vm.evaluate_function(factory, ArgValues::Empty) {
                Ok(value) => slots[field_index] = Some(value),
                Err(err) => {
                    line_errors.drop_with(vm);
                    release_slots(slots, errors, vm);
                    return Err(ValError::Run(err));
                }
            },
            FieldDefault::Required => {
                let whole = match input.to_value(vm) {
                    Ok(whole) => whole,
                    Err(err) => {
                        line_errors.drop_with(vm);
                        release_slots(slots, errors, vm);
                        return Err(err);
                    }
                };
                let name = vm.interns.get_str(field.name).to_owned();
                line_errors.push(LineError::owned(ErrorType::Missing {}, whole).with_outer_loc(LocItem::Key(name)));
            }
        }
    }
    state.add_fields_set(fields_set);
    if line_errors.is_empty() {
        Ok(slots
            .into_iter()
            .map(|slot| slot.expect("every field is set or errored"))
            .collect())
    } else {
        release_slots(slots, errors, vm);
        Err(ValError::Lines(line_errors))
    }
}

/// Releases the partial field results of a failed dataclass validation.
fn release_slots(slots: Vec<Option<Value>>, errors: Vec<Option<Vec<LineError>>>, vm: &mut VM<'_>) {
    for slot in slots.into_iter().flatten() {
        slot.drop_with(vm);
    }
    for lines in errors.into_iter().flatten() {
        lines.drop_with(vm);
    }
}

/// Allocates an instance of `class_id` with `values` stored as its fields,
/// then calls `__post_init__` when the class defines one.
fn build_instance(
    class_id: HeapId,
    dataclass: &DataclassValidator,
    values: Vec<Value>,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let mut attrs = DropGuard::new(Dict::new(), vm);
    {
        let (attrs, vm) = attrs.as_parts_mut();
        for (field, value) in dataclass.fields.iter().zip(values) {
            if let Some(old) = attrs.set(Value::InternString(field.name), value, vm)? {
                old.drop_with(vm);
            }
        }
    }
    let (attrs, vm) = attrs.into_parts();
    vm.heap.inc_ref(class_id);
    let instance = Value::Ref(
        vm.heap
            .allocate(HeapData::Instance(Box::new(Instance::new(class_id, attrs)))),
    );
    if !dataclass.post_init {
        return Ok(instance);
    }
    let mut guard = DropGuard::new(instance, vm);
    {
        let (instance, vm) = guard.as_parts();
        let hook = instance.py_getattr(
            &EitherStr::Interned(vm.interns.intern_static(StaticStrings::PostInit)),
            vm,
        )?;
        let CallResult::Value(hook) = hook else {
            unreachable!("an instance attribute lookup yields a value")
        };
        defer_drop!(hook, vm);
        let result = vm.evaluate_function(hook, ArgValues::Empty)?;
        result.drop_with(vm);
    }
    Ok(guard.into_inner())
}

/// A namedtuple built from positional (sequence) or keyword (dict)
/// arguments, with pydantic's `arguments` errors.
fn validate_named_tuple<I: Input>(
    named_tuple: &NamedTupleValidator,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    let field_count = named_tuple.field_names.len();
    let mut slots: Vec<Option<Value>> = (0..field_count).map(|_| None).collect();
    let mut errors: Vec<LineError> = Vec::new();
    if input.is_mapping(vm) {
        {
            let mut map = input.validate_dict(state.strict, vm)?.value;
            loop {
                let Some((mut key, mut value)) = map.next(vm)? else {
                    break;
                };
                let key_value = match key.to_value(vm) {
                    Ok(k) => k,
                    Err(err) => {
                        map.finish_entry(key, value, vm);
                        release_named(slots, errors, vm);
                        return Err(err);
                    }
                };
                let position = key_value
                    .as_str(vm)
                    .and_then(|name| named_tuple.field_names.iter().position(|field| field == name));
                let key_text = key_value.as_str(vm).map(str::to_owned);
                key_value.drop_with(vm);
                let item = match value.to_value(vm) {
                    Ok(v) => v,
                    Err(err) => {
                        map.finish_entry(key, value, vm);
                        release_named(slots, errors, vm);
                        return Err(err);
                    }
                };
                map.finish_entry(key, value, vm);
                match position {
                    Some(position) if slots[position].is_none() => slots[position] = Some(item),
                    _ => errors.push(
                        LineError::owned(ErrorType::UnexpectedKeywordArgument {}, item)
                            .with_outer_loc(LocItem::Key(key_text.unwrap_or_default())),
                    ),
                }
            }
        }
    } else {
        {
            let mut seq = match input.validate_sequence(SeqKind::Tuple, false, vm) {
                Ok(seq) => seq.value,
                Err(ValError::Lines(lines)) => {
                    lines.drop_with(vm);
                    return input.error(ErrorType::ArgumentsType {}, vm);
                }
                Err(err) => return Err(err),
            };
            let mut index = 0usize;
            loop {
                let Some(mut element) = seq.next(vm)? else { break };
                let item = match element.to_value(vm) {
                    Ok(v) => v,
                    Err(err) => {
                        seq.finish_item(element, vm);
                        release_named(slots, errors, vm);
                        return Err(err);
                    }
                };
                seq.finish_item(element, vm);
                if index < field_count {
                    slots[index] = Some(item);
                } else {
                    errors.push(
                        LineError::owned(ErrorType::UnexpectedPositionalArgument {}, item)
                            .with_outer_loc(index_loc(index)),
                    );
                }
                index += 1;
            }
        }
    }
    let Value::Ref(class_id) = named_tuple.class else {
        unreachable!("a namedtuple validator holds its class")
    };
    let defaults_start = field_count - named_tuple.defaults;
    // Missing arguments are reported before unexpected ones, as pydantic orders them.
    let mut missing: Vec<LineError> = Vec::new();
    for (position, slot) in slots.iter_mut().enumerate() {
        if slot.is_some() {
            continue;
        }
        if position >= defaults_start {
            let HeapData::NamedTupleClass(class) = vm.heap.get(class_id) else {
                unreachable!("a namedtuple validator holds a namedtuple class")
            };
            *slot = Some(class.defaults()[position - defaults_start].clone_with_heap(vm.heap));
        } else {
            let whole = match input.to_value(vm) {
                Ok(whole) => whole,
                Err(err) => {
                    missing.drop_with(vm);
                    release_named(slots, errors, vm);
                    return Err(err);
                }
            };
            missing.push(
                LineError::owned(ErrorType::MissingArgument {}, whole)
                    .with_outer_loc(LocItem::Key(named_tuple.field_names[position].clone())),
            );
        }
    }
    if !missing.is_empty() || !errors.is_empty() {
        release_named(slots, Vec::new(), vm);
        missing.append(&mut errors);
        return Err(ValError::Lines(missing));
    }
    state.floor_exactness(Exactness::Strict);
    let items: Vec<Value> = slots
        .into_iter()
        .map(|slot| slot.expect("every field is set"))
        .collect();
    let HeapData::NamedTupleClass(class) = vm.heap.get(class_id) else {
        unreachable!("a namedtuple validator holds a namedtuple class")
    };
    let name = class.name(vm.interns).to_owned();
    let field_names = class.field_names().to_vec();
    vm.heap.inc_ref(class_id);
    let tuple = NamedTuple::with_class(name, field_names, items, Some(class_id));
    Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(tuple)))))
}

/// Releases a failed namedtuple validation's partial results.
fn release_named(slots: Vec<Option<Value>>, errors: Vec<LineError>, vm: &mut VM<'_>) {
    for slot in slots.into_iter().flatten() {
        slot.drop_with(vm);
    }
    errors.drop_with(vm);
}
