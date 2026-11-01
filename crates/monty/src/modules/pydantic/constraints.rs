//! `Field(...)` constraints: coercing a bound to its validator's type while the
//! arena is built, and checking a validated value against the constraints, with
//! pydantic-core's rules and order (`validators/{int,float,decimal,string,list}.rs`).

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    expressions::CmpOperator,
    heap::{DropGuard, HeapData},
    modules::pydantic::{
        arena::{Arena, Constraints, NumberConstraints, StrConstraints, Validator, ValidatorId},
        catalogue::ErrorType,
        errors::ValResult,
        input::{Input, InputKind, State},
        snapshot::snapshot_value,
        validate::validate,
    },
    types::{FieldInfo, PyTrait, allocate_string, decimal::py as py_decimal},
    value::Value,
};

/// Which numeric type a bound is coerced to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumberKind {
    Int,
    Float,
    Decimal,
}

/// The numeric bounds of `info` coerced to `kind`, or `None` when it sets none;
/// a bound of the wrong type raises pydantic-core's `ValueError`.
pub(crate) fn number_constraints(
    info: &FieldInfo,
    kind: NumberKind,
    vm: &mut VM<'_>,
) -> RunResult<Option<NumberConstraints>> {
    let bounds = [
        ("multiple_of", &info.multiple_of),
        ("le", &info.le),
        ("lt", &info.lt),
        ("ge", &info.ge),
        ("gt", &info.gt),
    ];
    if bounds.iter().all(|(_, bound)| bound.is_none()) {
        return Ok(None);
    }
    let mut constraints = DropGuard::new(NumberConstraints::default(), vm);
    for (name, bound) in bounds {
        let Some(bound) = bound else { continue };
        let (constraints, vm) = constraints.as_parts_mut();
        let coerced = coerce_bound(name, bound, kind, vm)?;
        *match name {
            "multiple_of" => &mut constraints.multiple_of,
            "le" => &mut constraints.le,
            "lt" => &mut constraints.lt,
            "ge" => &mut constraints.ge,
            _ => &mut constraints.gt,
        } = Some(coerced);
    }
    Ok(Some(constraints.into_inner()))
}

/// One bound as a value of `kind`: an `int` must already be one, a `float`
/// takes ints too, a `Decimal` anything `Decimal()` accepts.
fn coerce_bound(name: &str, bound: &Value, kind: NumberKind, vm: &mut VM<'_>) -> RunResult<Value> {
    let is_int = |value: &Value| {
        matches!(value, Value::Int(_) | Value::InternLongInt(_))
            || matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)))
    };
    match kind {
        NumberKind::Int if is_int(bound) => Ok(bound.clone_with_heap(vm.heap)),
        NumberKind::Int => Err(ExcType::value_error(format!(
            "'{name}' must be coercible to an integer"
        ))),
        NumberKind::Float => match bound {
            Value::Float(_) => Ok(bound.clone_with_heap(vm.heap)),
            Value::Int(int) => Ok(Value::Float(*int as f64)),
            _ => Err(ExcType::value_error(format!("'{name}' must be coercible to a float"))),
        },
        NumberKind::Decimal => py_decimal::init(vm, ArgValues::One(bound.clone_with_heap(vm.heap)))
            .map_err(|_| ExcType::value_error(format!("'{name}' must be coercible to a Decimal"))),
    }
}

/// Validates `inner`, then applies `constraints` to its output.
pub(crate) fn validate_constrained<I: Input>(
    arena: &Arena,
    inner: ValidatorId,
    constraints: &Constraints,
    input: &mut I,
    state: &mut State,
    vm: &mut VM<'_>,
) -> ValResult<Value> {
    if let Constraints::Str(rules) = constraints
        && rules.coerce_numbers_to_str
        && let Some(text) = coerce_number_to_str(input, vm)?
    {
        return apply_str(rules, text, input, vm);
    }
    let value = validate(arena, inner, input, state, vm)?;
    match constraints {
        Constraints::Number(bounds) => {
            let mut guard = DropGuard::new(value, vm);
            let (value, vm) = guard.as_parts();
            check_number(bounds, value, input, vm)?;
            Ok(guard.into_inner())
        }
        Constraints::Str(rules) => apply_str(rules, value, input, vm),
        Constraints::Bytes { min_length, max_length } => {
            let mut guard = DropGuard::new(value, vm);
            let (value, vm) = guard.as_parts();
            let len = bytes_len(value, vm);
            if let Some(min) = *min_length
                && len < min
            {
                return input.error(ErrorType::BytesTooShort { min_length: min }, vm);
            }
            if let Some(max) = *max_length
                && len > max
            {
                return input.error(ErrorType::BytesTooLong { max_length: max }, vm);
            }
            Ok(guard.into_inner())
        }
        Constraints::Length { min_length, max_length } => {
            let mut guard = DropGuard::new(value, vm);
            let (value, vm) = guard.as_parts();
            let (field_type, len, exact) = container_len(arena.get(inner), value, vm);
            if let Some(min) = *min_length
                && len < min
            {
                let error = ErrorType::TooShort {
                    field_type: field_type.to_owned(),
                    min_length: min,
                    actual_length: len,
                };
                return input.error(error, vm);
            }
            if let Some(max) = *max_length
                && len > max
            {
                let error = ErrorType::TooLong {
                    field_type: field_type.to_owned(),
                    max_length: max,
                    actual_length: exact.then_some(len),
                };
                return input.error(error, vm);
            }
            Ok(guard.into_inner())
        }
    }
}

/// `coerce_numbers_to_str`: an `int`, `float` or `Decimal` input as its `str()`.
fn coerce_number_to_str<I: Input>(input: &mut I, vm: &mut VM<'_>) -> ValResult<Option<Value>> {
    let is_number = match input.kind() {
        InputKind::Int | InputKind::Float => true,
        InputKind::Other => input
            .key_value()
            .is_some_and(|value| py_decimal::as_decimal(value, vm).is_some()),
        _ => false,
    };
    if !is_number {
        return Ok(None);
    }
    let number = input.to_value(vm)?;
    defer_drop!(number, vm);
    Ok(Some(number.py_str(vm)?))
}

/// pydantic-core's numeric checks, in its order: `multiple_of`, `le`, `lt`, `ge`, `gt`.
fn check_number<I: Input>(bounds: &NumberConstraints, value: &Value, input: &mut I, vm: &mut VM<'_>) -> ValResult<()> {
    if let Some(multiple_of) = &bounds.multiple_of
        && !is_multiple(value, multiple_of, vm)?
    {
        let error = ErrorType::MultipleOf {
            multiple_of: snapshot_value(multiple_of, vm),
        };
        return input.error(error, vm);
    }
    for (bound, op, error) in [
        (&bounds.le, CmpOperator::LtE, ErrorKind::Le),
        (&bounds.lt, CmpOperator::Lt, ErrorKind::Lt),
        (&bounds.ge, CmpOperator::GtE, ErrorKind::Ge),
        (&bounds.gt, CmpOperator::Gt, ErrorKind::Gt),
    ] {
        if let Some(bound) = bound
            && !vm.cmp_values(op, value, bound)?
        {
            let number = snapshot_value(bound, vm);
            let error = match error {
                ErrorKind::Le => ErrorType::LessThanEqual { le: number },
                ErrorKind::Lt => ErrorType::LessThan { lt: number },
                ErrorKind::Ge => ErrorType::GreaterThanEqual { ge: number },
                ErrorKind::Gt => ErrorType::GreaterThan { gt: number },
            };
            return input.error(error, vm);
        }
    }
    Ok(())
}

/// Which inequality failed, for building its error.
enum ErrorKind {
    Le,
    Lt,
    Ge,
    Gt,
}

/// pydantic-core's `multiple_of` test for each numeric type: exact remainder
/// for ints, a `1e-9` tolerance for floats, and `(value / m) % 1 == 0` for decimals.
fn is_multiple(value: &Value, multiple_of: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match (value, multiple_of) {
        (Value::Float(float), Value::Float(step)) => {
            let rounded = (float / step).round();
            Ok((float - rounded * step).abs() <= 1e-9)
        }
        _ if py_decimal::as_decimal(value, vm).is_some() => {
            // A quotient the decimal context cannot represent is not a multiple.
            let Ok(quotient) = value.py_truediv(multiple_of, vm) else {
                return Ok(false);
            };
            defer_drop!(quotient, vm);
            let fraction = quotient.py_mod(&Value::Int(1), vm)?;
            defer_drop!(fraction, vm);
            vm.cmp_values(CmpOperator::Eq, fraction, &Value::Int(0))
        }
        _ => {
            let remainder = value.py_mod(multiple_of, vm)?;
            defer_drop!(remainder, vm);
            vm.cmp_values(CmpOperator::Eq, remainder, &Value::Int(0))
        }
    }
}

/// pydantic-core's string rules on the validated `value`: strip, check the
/// length, search the pattern, then change case; the error reports the input.
fn apply_str<I: Input>(rules: &StrConstraints, value: Value, input: &mut I, vm: &mut VM<'_>) -> ValResult<Value> {
    let mut guard = DropGuard::new(value, vm);
    let (value, vm) = guard.as_parts();
    let text = value.as_str(vm).expect("a str validator yields a str");
    let text = if rules.strip_whitespace { text.trim() } else { text };
    if rules.min_length.is_some() || rules.max_length.is_some() {
        let len = text.chars().count();
        if let Some(min) = rules.min_length
            && len < min
        {
            return input.error(ErrorType::StringTooShort { min_length: min }, vm);
        }
        if let Some(max) = rules.max_length
            && len > max
        {
            return input.error(ErrorType::StringTooLong { max_length: max }, vm);
        }
    }
    if let Some(pattern) = &rules.pattern
        && !pattern.is_match(text)?
    {
        let error = ErrorType::StringPatternMismatch {
            pattern: pattern.source.clone(),
        };
        return input.error(error, vm);
    }
    let changed = if rules.to_lower {
        Some(text.to_lowercase())
    } else if rules.to_upper {
        Some(text.to_uppercase())
    } else if rules.strip_whitespace && text.len() != value.as_str(vm).expect("checked above").len() {
        Some(text.to_owned())
    } else {
        None
    };
    Ok(match changed {
        Some(changed) => allocate_string(changed, vm.heap),
        None => guard.into_inner(),
    })
}

/// The byte length of a validated `bytes` value.
fn bytes_len(value: &Value, vm: &VM<'_>) -> usize {
    match value {
        Value::InternBytes(id) => vm.interns.get_bytes(*id).len(),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => bytes.as_slice().len(),
            _ => unreachable!("a bytes validator yields bytes"),
        },
        _ => unreachable!("a bytes validator yields bytes"),
    }
}

/// A validated container's pydantic name and length; `exact` is false for the
/// sets, whose `too_long` reports no count, as pydantic-core stops early there.
fn container_len(validator: &Validator, value: &Value, vm: &VM<'_>) -> (&'static str, usize, bool) {
    let Value::Ref(id) = value else {
        unreachable!("a container validator yields a heap value")
    };
    match (validator, vm.heap.get(*id)) {
        (Validator::List(_), HeapData::List(list)) => ("List", list.len(), true),
        (Validator::Tuple { .. }, HeapData::Tuple(tuple)) => ("Tuple", tuple.as_slice().len(), true),
        (Validator::Set(_), HeapData::Set(set)) => ("Set", set.len(), false),
        (Validator::FrozenSet(_), HeapData::FrozenSet(set)) => ("Frozenset", set.len(), false),
        (Validator::Dict { .. }, HeapData::Dict(dict)) => ("Dictionary", dict.len(), true),
        _ => unreachable!("length constraints are built only on containers"),
    }
}
