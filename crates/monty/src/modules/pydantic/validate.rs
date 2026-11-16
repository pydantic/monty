//! Validation of an [`Input`] against an [`Arena`] validator, producing the
//! coerced value or the line errors, with pydantic-core's semantics for each
//! validator kind (`validators/*.rs`).
//!
//! Written once over the [`Input`] trait, so the Python and JSON modes share
//! every rule here and differ only in how a value is read. Errors are
//! collected rather than raised at the first failure: a list reports every
//! bad item, a dataclass every bad field.

use std::mem;

use monty_types::LocItem;
use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::pydantic::{
        arena::{
            Arena, DataclassValidator, FieldDefault, FieldValidator, LiteralValidator, NamedTupleValidator, Validator,
            ValidatorId,
        },
        catalogue::ErrorType,
        coerce::{date_at_midnight, datetime_is_midnight},
        errors::{LineError, ValError, ValResult},
        input::{ArgumentsShape, Exactness, Input, InputKind, Mapping, SeqKind, Sequence, SharedInput, State},
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
///
/// Nested validators recurse through here on the native stack, so every arm
/// is a direct call writing one shared result: temporaries in this frame
/// would be repeated at each nesting level, which debug builds feel first.
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
    // A smart union re-validates its input once per member, so the walk can
    // fan out far beyond the input's size: the clock is polled by call count.
    state.steps = state.steps.wrapping_add(1);
    vm.heap.tracker.check_time_every(state.steps)?;
    match arena.get(id) {
        Validator::List(item) => validate_list(arena, id, *item, input, state, vm),
        Validator::Set(item) => validate_set(arena, id, *item, false, input, state, vm),
        Validator::FrozenSet(item) => validate_set(arena, id, *item, true, input, state, vm),
        Validator::Tuple { items, variadic } => validate_tuple(arena, id, items, *variadic, input, state, vm),
        Validator::Dict { key, value } => validate_dict(arena, id, *key, *value, input, state, vm),
        Validator::Union(choices) => validate_union(arena, choices, input, state, vm),
        Validator::Nullable(inner) if !input.is_none() => validate(arena, *inner, input, state, vm),
        Validator::Literal(literal) => validate_literal(literal, input, vm),
        Validator::Dataclass(dataclass) => validate_dataclass(arena, id, dataclass, input, state, vm),
        Validator::NamedTuple(named_tuple) => validate_named_tuple(named_tuple, input, state, vm),
        validator => validate_scalar(validator, input, state, vm),
    }
}

/// The validators that do not nest (a `None` for `Nullable` included), out
/// of line so their temporaries stay off the recursive path through [`validate`].
#[inline(never)]
fn validate_scalar<I: Input>(
    validator: &Validator,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    match validator {
        Validator::Any => {
            // In a union, `Any` is preferred to a lax coercion.
            state.floor_exactness(Exactness::Strict);
            input.to_value(vm)
        }
        // `to_value` consumes a JSON `null`; a Python `None` is free to copy.
        Validator::None | Validator::Nullable(_) if input.is_none() => input.to_value(vm),
        Validator::None => input.error(ErrorType::NoneRequired {}, vm),
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
        _ => unreachable!("validate dispatches the nesting validators itself"),
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
                    Ok(date_at_midnight(date, vm).expect("validate_date yields a date"))
                }
                Err(ValError::Lines(lines)) => Err(fallback_errors(
                    datetime_errors,
                    lines,
                    |error_type| match error_type {
                        ErrorType::DateParsing { error } => {
                            Some(ErrorType::DatetimeFromDateParsing { error: error.clone() })
                        }
                        _ => None,
                    },
                    vm,
                )),
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
                // A datetime parse failure is reported as the date failure it also is.
                Err(ValError::Lines(lines)) => Err(fallback_errors(
                    date_errors,
                    lines,
                    |error_type| match error_type {
                        ErrorType::DatetimeParsing { error } => {
                            Some(ErrorType::DateFromDatetimeParsing { error: error.clone() })
                        }
                        _ => None,
                    },
                    vm,
                )),
                Err(other) => {
                    date_errors.drop_with(vm);
                    Err(other)
                }
            }
        }
        Err(err) => Err(err),
    }
}

/// Whether every line of a failed `datetime` is a text-parsing error (vacuously
/// true for no lines, though a failure always has one): the case the lax
/// fallback retries as a date.
fn is_parsing_error(lines: &[LineError]) -> bool {
    lines.iter().all(|line| {
        matches!(
            line.error_type,
            ErrorType::DatetimeParsing { .. } | ErrorType::DateParsing { .. }
        )
    })
}

/// The errors of a failed lax temporal fallback: its own `lines`, with each
/// error `retype` maps retyped, when it maps any; otherwise the first attempt's
/// `original` errors. The set not reported is released.
fn fallback_errors(
    original: Vec<LineError>,
    lines: Vec<LineError>,
    retype: impl Fn(&ErrorType) -> Option<ErrorType>,
    vm: &mut VM<'_>,
) -> ValError {
    if lines.iter().any(|line| retype(&line.error_type).is_some()) {
        original.drop_with(vm);
        ValError::Lines(
            lines
                .into_iter()
                .map(|line| match retype(&line.error_type) {
                    Some(error_type) => line.with_type(error_type),
                    None => line,
                })
                .collect(),
        )
    } else {
        lines.drop_with(vm);
        ValError::Lines(original)
    }
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
        defer_drop_mut!(seq, vm);
        let items = validate_items::<I, _>(arena, item, seq, Vec::new(), push_item, state, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
    })
}

/// `set[T]` / `frozenset[T]`: every item validated and added; an unhashable
/// result is `set_item_not_hashable`, reported among the items' errors by index.
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
        defer_drop_mut!(seq, vm);
        let set = validate_items::<I, _>(arena, item, seq, Set::new(), add_item, state, vm)?;
        let data = if frozen {
            HeapData::FrozenSet(FrozenSet::from_set(set))
        } else {
            HeapData::Set(set)
        };
        Ok(Value::Ref(vm.heap.allocate(data)))
    })
}

/// Validates each item of `seq` against `item`, handing each success to `add`
/// (with its index, and the errors to report an item it refuses in) and
/// collecting every error located by index; `output` is released on failure.
fn validate_items<I: Input, O: for<'h> DropWithContext<VM<'h>>>(
    arena: &Arena,
    item: ValidatorId,
    seq: &mut I::Seq,
    output: O,
    add: impl Fn(&mut O, Value, usize, &mut Vec<LineError>, &mut VM<'_>) -> RunResult<()>,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<O> {
    // One guard over the output and the errors, so a parse or resource error
    // leaving through `?` releases both.
    let mut collected = DropGuard::new((output, Vec::<LineError>::new()), vm);
    let mut index = 0usize;
    loop {
        let ((output, errors), vm) = collected.as_parts_mut();
        vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
        let Some(mut element) = seq.next(vm)? else { break };
        let added = match validate(arena, item, &mut element, state, vm) {
            Ok(value) => add(output, value, index, errors, vm).map_err(ValError::Run),
            Err(ValError::Lines(lines)) => {
                errors.extend(lines.into_iter().map(|line| line.with_outer_loc(index_loc(index))));
                Ok(())
            }
            Err(err) => Err(err),
        };
        seq.finish_item(element, vm);
        added?;
        index += 1;
    }
    let ((output, errors), vm) = collected.into_parts();
    if errors.is_empty() {
        Ok(output)
    } else {
        output.drop_with(vm);
        Err(ValError::Lines(errors))
    }
}

/// A sequence index as a location step.
fn index_loc(index: usize) -> LocItem {
    LocItem::Index(i64::try_from(index).unwrap_or(i64::MAX))
}

/// Appends a list item, the buffer growing under the resource tracker.
fn push_item(
    output: &mut Vec<Value>,
    value: Value,
    _index: usize,
    _errors: &mut Vec<LineError>,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    match vm
        .heap
        .tracker
        .check_growth(output.len(), output.capacity(), VALUE_SIZE)
    {
        Ok(()) => {
            output.push(value);
            Ok(())
        }
        Err(err) => {
            value.drop_with(vm);
            Err(err.into())
        }
    }
}

/// Adds a set item; as in pydantic, only a `TypeError` means unhashable, and
/// anything else propagates.
fn add_item(set: &mut Set, value: Value, index: usize, errors: &mut Vec<LineError>, vm: &mut VM<'_>) -> RunResult<()> {
    let probe = value.clone_with_heap(vm.heap);
    match set.add(value, vm) {
        Err(err) if err.is_catchable(ExcType::TypeError) => {
            errors.push(LineError::owned(ErrorType::SetItemNotHashable {}, probe).with_outer_loc(index_loc(index)));
            Ok(())
        }
        added => {
            probe.drop_with(vm);
            added.map(drop)
        }
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
        let seq = input.validate_sequence(SeqKind::Tuple, state.strict, vm)?.unpack(state);
        defer_drop_mut!(seq, vm);
        let actual_length = seq.len();
        let fixed = if variadic { items.len() - 1 } else { items.len() };
        let mut collected = DropGuard::new((Vec::new(), Vec::<LineError>::new()), vm);
        let mut index = 0usize;
        let mut too_long = false;
        loop {
            let ((output, errors), vm) = collected.as_parts_mut();
            vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
            let Some(mut element) = seq.next(vm)? else { break };
            let validator = if index < fixed {
                items[index]
            } else if variadic {
                items[fixed]
            } else {
                seq.finish_item(element, vm);
                too_long = true;
                break;
            };
            let added = match validate(arena, validator, &mut element, state, vm) {
                Ok(value) => push_item(output, value, index, errors, vm).map_err(ValError::Run),
                Err(ValError::Lines(lines)) => {
                    errors.extend(lines.into_iter().map(|line| line.with_outer_loc(index_loc(index))));
                    Ok(())
                }
                Err(err) => Err(err),
            };
            seq.finish_item(element, vm);
            added?;
            index += 1;
        }
        if too_long {
            drop(collected);
            // A streamed input reports its length too: the rest is walked for the count.
            let actual_length = match actual_length {
                Some(len) => len,
                None => drain_count::<I>(seq, index + 1, vm)?,
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
        for missing in index..fixed {
            let ((_, errors), vm) = collected.as_parts_mut();
            errors.push(missing_error(input, missing, vm)?);
        }
        let ((output, errors), vm) = collected.into_parts();
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
        vm.heap.tracker.check_time_every(count).map_err(RunError::from)?;
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
        let map = input.validate_dict(state.strict, vm)?.unpack(state);
        defer_drop_mut!(map, vm);
        let mut collected = DropGuard::new((Dict::new(), Vec::<LineError>::new()), vm);
        let mut index = 0usize;
        loop {
            let ((output, errors), vm) = collected.as_parts_mut();
            vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
            let Some((mut key_input, mut value_input)) = map.next(vm)? else {
                break;
            };
            index += 1;
            let entry = validate_entry(arena, key, value, &mut key_input, &mut value_input, state, vm);
            map.finish_entry(key_input, value_input, vm);
            match entry? {
                Ok((k, v)) => {
                    if let Some(old) = output.set(k, v, vm)? {
                        old.drop_with(vm);
                    }
                }
                Err(lines) => errors.extend(lines),
            }
        }
        let ((output, errors), vm) = collected.into_parts();
        if errors.is_empty() {
            Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(output))))
        } else {
            output.drop_with(vm);
            Err(ValError::Lines(errors))
        }
    })
}

/// Validates one dict entry into its key and value, or into the errors of
/// both: a key's located at `(key, '[key]')`, a value's at `(key,)`. The key
/// is rendered as a location only when there are errors.
fn validate_entry<I: Input>(
    arena: &Arena,
    key_validator: ValidatorId,
    value_validator: ValidatorId,
    key: &mut I,
    value: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Result<(Value, Value), Vec<LineError>>> {
    let output_key = match validate(arena, key_validator, key, state, vm) {
        Ok(k) => Ok(k),
        Err(ValError::Lines(lines)) => Err(lines),
        Err(err) => return Err(err),
    };
    let output_value = match validate(arena, value_validator, value, state, vm) {
        Ok(v) => Ok(v),
        Err(ValError::Lines(lines)) => Err(lines),
        Err(err) => {
            match output_key {
                Ok(k) => k.drop_with(vm),
                Err(lines) => lines.drop_with(vm),
            }
            return Err(err);
        }
    };
    Ok(match (output_key, output_value) {
        (Ok(k), Ok(v)) => Ok((k, v)),
        (output_key, output_value) => {
            let loc = loc_item(key, vm);
            let mut errors = Vec::new();
            match output_key {
                Ok(k) => k.drop_with(vm),
                Err(lines) => errors.extend(lines.into_iter().map(|line| {
                    line.with_outer_loc(LocItem::Key("[key]".to_owned()))
                        .with_outer_loc(loc.clone())
                })),
            }
            match output_value {
                Ok(v) => v.drop_with(vm),
                Err(lines) => errors.extend(lines.into_iter().map(|line| line.with_outer_loc(loc.clone()))),
            }
            Err(errors)
        }
    })
}

/// A mapping key as a location step: text and ints (bools included) as
/// themselves, anything else by its repr, as pydantic's `LocItem::from` does.
fn loc_item(key: &impl Input, vm: &mut VM<'_>) -> LocItem {
    match key.key_value() {
        Some(Value::Int(index)) => LocItem::Index(*index),
        Some(Value::Bool(b)) => LocItem::Index(i64::from(*b)),
        Some(other) => match other.as_str(vm) {
            Some(text) => LocItem::Key(text.to_owned()),
            None => LocItem::Key(repr_string(other, vm)),
        },
        // Every key a mapping yields has a value; this is not reached.
        None => LocItem::Key(String::new()),
    }
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
    let mut found = None;
    for choice in &literal.choices {
        if choice.py_type(vm) == input_type && choice.py_eq(value, vm)? {
            found = Some(choice);
            break;
        }
    }
    if found.is_none() {
        for choice in &literal.choices {
            if choice.py_eq(value, vm)? {
                found = Some(choice);
            }
        }
    }
    match found {
        Some(choice) => Ok(choice.clone_with_heap(vm.heap)),
        None => input.error(
            ErrorType::LiteralError {
                expected: literal.expected.clone(),
            },
            vm,
        ),
    }
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
        let map = input.validate_dataclass_args(&dataclass.name, vm)?;
        defer_drop_mut!(map, vm);
        let fields = validate_fields(arena, &dataclass.fields, map, input, state, vm)?;
        state.floor_exactness(Exactness::Strict);
        build_instance(class_id, dataclass, fields, vm).map_err(ValError::Run)
    })
}

/// Validates the entries of `map` into one value per field, field by field in
/// declaration order as pydantic does (so nested `__post_init__` hooks and
/// factories run in that order): an entry for the next field is validated as
/// it is read, any other is held until the fields before it are done. Unknown
/// keys are ignored, a key given twice (JSON allows it) keeps its last value,
/// missing fields take their default or report `missing`, and a field's errors
/// are located by its name.
fn validate_fields<I: Input>(
    arena: &Arena,
    fields: &[FieldValidator],
    map: &mut I::Map,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Vec<Value>> {
    let slots: Vec<FieldSlot<I>> = (0..fields.len()).map(|_| FieldSlot::Empty).collect();
    let mut guard = DropGuard::new(slots, vm);
    // The first field not yet validated.
    let mut next = 0usize;
    let mut index = 0usize;
    loop {
        let (slots, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_memory_time_every(index).map_err(RunError::from)?;
        let Some((key, mut value)) = map.next(vm)? else {
            break;
        };
        index += 1;
        let Some(field_index) = field_position(fields, &key, vm) else {
            // An ignored JSON value is still read, so the document is checked as pydantic parses it.
            let read = value.to_value(vm).map(|v| v.drop_with(vm));
            map.finish_entry(key, value, vm);
            read?;
            continue;
        };
        mem::take(&mut slots[field_index]).drop_with(vm);
        if field_index == next {
            let result = validate(arena, fields[field_index].validator, &mut value, state, vm);
            map.finish_entry(key, value, vm);
            slots[field_index] = FieldSlot::validated(result)?;
            next = validate_held(arena, fields, slots, next + 1, state, vm)?;
        } else {
            slots[field_index] = FieldSlot::Held(map.hold_entry(key, value, vm));
        }
    }
    let (slots, vm) = guard.as_parts_mut();
    state.add_fields_set(slots.iter().filter(|slot| !matches!(slot, FieldSlot::Empty)).count());
    let line_errors = fill_fields(arena, fields, slots, input, state, vm)?;
    if line_errors.is_empty() {
        let (slots, _) = guard.into_parts();
        Ok(slots
            .into_iter()
            .map(|slot| match slot {
                FieldSlot::Valid(value) => value,
                _ => unreachable!("every field is valid"),
            })
            .collect())
    } else {
        Err(ValError::Lines(line_errors))
    }
}

/// What a dataclass field has so far: nothing, an input held for later, or
/// the outcome of validating one.
#[derive(Default)]
enum FieldSlot<I> {
    #[default]
    Empty,
    Held(I),
    Valid(Value),
    Invalid(Vec<LineError>),
}

impl<I> FieldSlot<I> {
    /// A field's validation result as its slot; a raised error propagates.
    fn validated(result: ValResult<Value>) -> ValResult<Self> {
        match result {
            Ok(value) => Ok(Self::Valid(value)),
            Err(ValError::Lines(lines)) => Ok(Self::Invalid(lines)),
            Err(err) => Err(err),
        }
    }
}

impl<'h, I: Input> DropWithContext<VM<'h>> for FieldSlot<I> {
    fn drop_with(self, vm: &mut VM<'h>) {
        match self {
            Self::Empty => {}
            Self::Held(input) => input.drop_with(vm),
            Self::Valid(value) => value.drop_with(vm),
            Self::Invalid(lines) => lines.drop_with(vm),
        }
    }
}

/// Validates the held inputs from field `from` on, in order, stopping at the
/// first field with nothing yet; returns that field's index.
fn validate_held<I: Input>(
    arena: &Arena,
    fields: &[FieldValidator],
    slots: &mut [FieldSlot<I>],
    from: usize,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<usize> {
    let mut next = from;
    while let Some(slot) = slots.get_mut(next) {
        match mem::take(slot) {
            FieldSlot::Empty => break,
            FieldSlot::Held(mut held) => {
                let result = held.replay(vm, |held, vm| validate(arena, fields[next].validator, held, state, vm));
                held.drop_with(vm);
                *slot = FieldSlot::validated(result)?;
            }
            done => *slot = done,
        }
        next += 1;
    }
    Ok(next)
}

/// Finishes every field in order once the input is read: a held input is
/// validated, an absent field takes its default (a factory may run Python) or
/// a `missing` error. Returns every field's errors under its name.
fn fill_fields<I: Input>(
    arena: &Arena,
    fields: &[FieldValidator],
    slots: &mut [FieldSlot<I>],
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Vec<LineError>> {
    let whole = SharedInput::default();
    defer_drop_mut!(whole, vm);
    for (field_index, field) in fields.iter().enumerate() {
        match &slots[field_index] {
            FieldSlot::Held(_) => {
                validate_held(arena, fields, slots, field_index, state, vm)?;
            }
            FieldSlot::Empty => {
                slots[field_index] = match &field.default {
                    FieldDefault::Value(value) => FieldSlot::Valid(value.clone_with_heap(vm.heap)),
                    FieldDefault::Factory(factory) => {
                        FieldSlot::Valid(vm.evaluate_function(factory, ArgValues::Empty).map_err(ValError::Run)?)
                    }
                    FieldDefault::Required => {
                        FieldSlot::Invalid(vec![LineError::owned(ErrorType::Missing {}, whole.get(input, vm)?)])
                    }
                };
            }
            FieldSlot::Valid(_) | FieldSlot::Invalid(_) => {}
        }
    }
    let mut line_errors = Vec::new();
    for (slot, field) in slots.iter_mut().zip(fields) {
        if let FieldSlot::Invalid(lines) = slot {
            let name = vm.interns.get_str(field.name);
            line_errors.extend(
                mem::take(lines)
                    .into_iter()
                    .map(|line| line.with_outer_loc(LocItem::Key(name.to_owned()))),
            );
        }
    }
    Ok(line_errors)
}

/// The field `key` names, if any: an interned key by id, since field names are
/// interned and equal text interns once; any other string by text.
fn field_position<I: Input>(fields: &[FieldValidator], key: &I, vm: &VM<'_>) -> Option<usize> {
    let key = key.key_value()?;
    let by_id = match key {
        Value::InternString(id) => fields.iter().position(|field| field.name == *id),
        _ => None,
    };
    by_id.or_else(|| {
        let name = key.as_str(vm)?;
        fields.iter().position(|field| vm.interns.get_str(field.name) == name)
    })
}

/// Allocates an instance of `class_id` with `values` stored as its fields,
/// then calls `__post_init__` when the class defines one.
fn build_instance(
    class_id: HeapId,
    dataclass: &DataclassValidator,
    values: Vec<Value>,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    // Field names are distinct interned strings, so each is appended without a probe.
    // The values not yet moved stay guarded, so a failed append releases them.
    let mut parts = DropGuard::new((Dict::with_capacity(dataclass.fields.len()), values), vm);
    {
        let ((attrs, values), vm) = parts.as_parts_mut();
        for (field, value) in dataclass.fields.iter().zip(values.iter_mut()) {
            attrs.set_new_interned(field.name, mem::replace(value, Value::None), vm)?;
        }
    }
    let ((attrs, _), vm) = parts.into_parts();
    vm.heap.inc_ref(class_id);
    let instance = Value::Ref(
        vm.heap
            .allocate(HeapData::Instance(Box::new(Instance::new(class_id, attrs)))),
    );
    if dataclass.post_init {
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
    } else {
        Ok(instance)
    }
}

/// A namedtuple built from positional (list or tuple) or keyword (dict)
/// arguments, with pydantic's `arguments` errors.
fn validate_named_tuple<I: Input>(
    named_tuple: &NamedTupleValidator,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    let slots: Vec<Option<Value>> = (0..named_tuple.field_names.len()).map(|_| None).collect();
    // One guard over the filled slots and the argument errors, released together on every failure path.
    let mut collected = DropGuard::new((slots, Vec::<LineError>::new()), vm);
    {
        let ((slots, errors), vm) = collected.as_parts_mut();
        match input.arguments_shape(vm) {
            ArgumentsShape::Keywords => read_keyword_arguments(named_tuple, input, state.strict, slots, errors, vm)?,
            ArgumentsShape::Positional => read_positional_arguments(input, slots, errors, vm)?,
            ArgumentsShape::Other => return input.error(ErrorType::ArgumentsType {}, vm),
        }
        fill_missing_arguments(named_tuple, input, slots, errors, vm)?;
    }
    let ((slots, errors), vm) = collected.into_parts();
    if errors.is_empty() {
        state.floor_exactness(Exactness::Strict);
        let items: Vec<Value> = slots
            .into_iter()
            .map(|slot| slot.expect("every field is set"))
            .collect();
        let Value::Ref(class_id) = named_tuple.class else {
            unreachable!("a namedtuple validator holds its class")
        };
        let HeapData::NamedTupleClass(class) = vm.heap.get(class_id) else {
            unreachable!("a namedtuple validator holds a namedtuple class")
        };
        let name = class.name(vm.interns).to_owned();
        let field_names = class.field_names().to_vec();
        vm.heap.inc_ref(class_id);
        let tuple = NamedTuple::with_class(name, field_names, items, Some(class_id));
        Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(tuple)))))
    } else {
        slots.drop_with(vm);
        Err(ValError::Lines(errors))
    }
}

/// Fills `slots` from a dict's entries: a key that names no field (or one
/// already filled) is `unexpected_keyword_argument`, one that is not a string `invalid_key`.
fn read_keyword_arguments<I: Input>(
    named_tuple: &NamedTupleValidator,
    input: &mut I,
    strict: bool,
    slots: &mut [Option<Value>],
    errors: &mut Vec<LineError>,
    vm: &mut VM<'_>,
) -> ValResult<()> {
    let map = input.validate_dict(strict, vm)?.value;
    defer_drop_mut!(map, vm);
    while let Some((mut key, mut value)) = map.next(vm)? {
        let position = key
            .key_value()
            .and_then(|key| key.as_str(vm))
            .map(|name| named_tuple.field_names.iter().position(|field| field == name));
        let loc = loc_item(&key, vm);
        // A key that is not a string is `invalid_key`, on the key itself.
        let entry = if position.is_some() {
            value.to_value(vm)
        } else {
            key.to_value(vm)
        };
        map.finish_entry(key, value, vm);
        let item = entry?;
        match position {
            Some(Some(position)) if slots[position].is_none() => slots[position] = Some(item),
            Some(_) => errors.push(LineError::owned(ErrorType::UnexpectedKeywordArgument {}, item).with_outer_loc(loc)),
            None => errors.push(LineError::owned(ErrorType::InvalidKey {}, item).with_outer_loc(loc)),
        }
        // Each key scans every field name, so poll per entry rather than every few hundred.
        vm.heap.tracker.check_memory_time()?;
    }
    Ok(())
}

/// Fills `slots` in order from a list or tuple, an item past the last field
/// becoming an `unexpected_positional_argument` error.
fn read_positional_arguments<I: Input>(
    input: &mut I,
    slots: &mut [Option<Value>],
    errors: &mut Vec<LineError>,
    vm: &mut VM<'_>,
) -> ValResult<()> {
    let seq = input.validate_sequence(SeqKind::Tuple, false, vm)?.value;
    defer_drop_mut!(seq, vm);
    let mut index = 0usize;
    while let Some(mut element) = seq.next(vm)? {
        let item = element.to_value(vm);
        seq.finish_item(element, vm);
        let item = item?;
        match slots.get_mut(index) {
            Some(slot) => *slot = Some(item),
            None => errors.push(
                LineError::owned(ErrorType::UnexpectedPositionalArgument {}, item).with_outer_loc(index_loc(index)),
            ),
        }
        index += 1;
        // Polled once the item is owned by `slots` or `errors`, so an error here leaks nothing.
        vm.heap.tracker.check_memory_time_every(index)?;
    }
    Ok(())
}

/// Gives each empty slot its default, or a `missing_argument` error; the
/// missing errors go before the unexpected ones, as pydantic orders them.
fn fill_missing_arguments<I: Input>(
    named_tuple: &NamedTupleValidator,
    input: &mut I,
    slots: &mut [Option<Value>],
    errors: &mut Vec<LineError>,
    vm: &mut VM<'_>,
) -> ValResult<()> {
    let Value::Ref(class_id) = named_tuple.class else {
        unreachable!("a namedtuple validator holds its class")
    };
    let unexpected = errors.len();
    let defaults_start = slots.len() - named_tuple.defaults;
    let whole = SharedInput::default();
    defer_drop_mut!(whole, vm);
    for (position, slot) in slots.iter_mut().enumerate() {
        vm.heap.tracker.check_memory_time_every(position)?;
        if slot.is_some() {
            continue;
        }
        if position >= defaults_start {
            let HeapData::NamedTupleClass(class) = vm.heap.get(class_id) else {
                unreachable!("a namedtuple validator holds a namedtuple class")
            };
            *slot = Some(class.defaults()[position - defaults_start].clone_with_heap(vm.heap));
        } else {
            let shared = whole.get(input, vm)?;
            errors.push(
                LineError::owned(ErrorType::MissingArgument {}, shared)
                    .with_outer_loc(LocItem::Key(named_tuple.field_names[position].clone())),
            );
        }
    }
    errors.rotate_left(unexpected);
    Ok(())
}
