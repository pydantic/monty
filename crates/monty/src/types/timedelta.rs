//! Python `datetime.timedelta` implementation.
//!
//! Monty stores timedeltas using `chrono::TimeDelta`, while preserving CPython's
//! normalized `(days, seconds, microseconds)` semantics for constructors, arithmetic,
//! and formatting.

use std::fmt::Write;

use ahash::AHashSet;
use chrono::TimeDelta as ChronoTimeDelta;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::{PyTrait, Type},
    value::{EitherStr, Value},
};

/// Minimum allowed day magnitude for `timedelta`.
pub(crate) const MIN_TIMEDELTA_DAYS: i32 = -999_999_999;
/// Maximum allowed day magnitude for `timedelta`.
pub(crate) const MAX_TIMEDELTA_DAYS: i32 = 999_999_999;

const DAY_SECONDS: i32 = 86_400;
pub(crate) const SECONDS_PER_HOUR: i32 = 3_600;
pub(crate) const SECONDS_PER_MINUTE: i32 = 60;
pub(crate) const MICROSECONDS_PER_SECOND: i128 = 1_000_000;
const MILLISECONDS_PER_SECOND: i128 = 1_000;
const DAY_MICROSECONDS: i128 = (DAY_SECONDS as i128) * MICROSECONDS_PER_SECOND;
const HOUR_MICROSECONDS: i128 = (SECONDS_PER_HOUR as i128) * MICROSECONDS_PER_SECOND;
const MINUTE_MICROSECONDS: i128 = (SECONDS_PER_MINUTE as i128) * MICROSECONDS_PER_SECOND;

/// `datetime.timedelta` storage backed by `chrono::TimeDelta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimeDelta(pub(crate) ChronoTimeDelta);

/// Creates a normalized timedelta value from CPython components.
pub(crate) fn new(days: i32, seconds: i32, microseconds: i32) -> RunResult<TimeDelta> {
    if !(MIN_TIMEDELTA_DAYS..=MAX_TIMEDELTA_DAYS).contains(&days) {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            format!("days={days}; must have magnitude <= 999999999"),
        )
        .into());
    }
    if !(0..DAY_SECONDS).contains(&seconds)
        || !(0..i32::try_from(MICROSECONDS_PER_SECOND).unwrap()).contains(&microseconds)
    {
        return Err(SimpleException::new_msg(ExcType::ValueError, "timedelta normalized fields out of range").into());
    }
    let total_microseconds =
        i128::from(days) * DAY_MICROSECONDS + i128::from(seconds) * MICROSECONDS_PER_SECOND + i128::from(microseconds);
    from_total_microseconds(total_microseconds)
}

/// Returns CPython normalized `(days, seconds, microseconds)` components.
#[must_use]
pub(crate) fn components(delta: &TimeDelta) -> (i32, i32, i32) {
    let total_microseconds = total_microseconds(delta);
    let days = total_microseconds.div_euclid(DAY_MICROSECONDS);
    let rem = total_microseconds.rem_euclid(DAY_MICROSECONDS);
    let seconds = rem / MICROSECONDS_PER_SECOND;
    let micros = rem % MICROSECONDS_PER_SECOND;
    (
        i32::try_from(days).expect("chrono day range fits CPython i32 day bounds"),
        i32::try_from(seconds).expect("seconds are bounded by one day"),
        i32::try_from(micros).expect("microseconds are bounded by one second"),
    )
}

/// Returns the duration as total microseconds.
#[must_use]
pub(crate) fn total_microseconds(delta: &TimeDelta) -> i128 {
    // `subsec_nanos` can be negative for negative durations; summing both parts
    // yields an exact signed duration as long as we keep microsecond precision.
    let seconds = i128::from(delta.0.num_seconds());
    let microseconds =
        i128::from(delta.0.subsec_nanos() / i32::try_from(MILLISECONDS_PER_SECOND).expect("1000 fits in i32"));
    seconds * MICROSECONDS_PER_SECOND + microseconds
}

/// Returns the duration as total whole seconds plus fractional microseconds.
#[must_use]
pub(crate) fn total_seconds(delta: &TimeDelta) -> f64 {
    total_microseconds(delta) as f64 / (MICROSECONDS_PER_SECOND as f64)
}

/// Returns total seconds only when exact (no microseconds), otherwise `None`.
#[must_use]
pub(crate) fn exact_total_seconds(delta: &TimeDelta) -> Option<i128> {
    let (days, seconds, microseconds) = components(delta);
    if microseconds == 0 {
        Some(i128::from(days) * i128::from(DAY_SECONDS) + i128::from(seconds))
    } else {
        None
    }
}

/// Exposes the underlying chrono duration for datetime/date arithmetic.
#[must_use]
pub(crate) fn chrono_delta(delta: &TimeDelta) -> ChronoTimeDelta {
    delta.0
}

/// Divides a timedelta by an integer divisor using CPython's rounding rule.
///
/// CPython rounds to the nearest microsecond with ties going to the even result,
/// rather than truncating toward zero.
#[must_use]
pub(crate) fn div_microseconds_round_ties_even(total_microseconds: i128, divisor: i128) -> i128 {
    debug_assert_ne!(divisor, 0);

    let negative = total_microseconds.is_negative() ^ divisor.is_negative();
    let numerator = total_microseconds.abs();
    let denominator = divisor.abs();
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;

    let rounded = match (remainder * 2).cmp(&denominator) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => {
            if quotient % 2 == 0 {
                quotient
            } else {
                quotient + 1
            }
        }
    };

    if negative { -rounded } else { rounded }
}

/// Converts a chrono duration to Monty's bounded timedelta.
pub(crate) fn from_chrono(delta: ChronoTimeDelta) -> RunResult<TimeDelta> {
    from_total_microseconds(
        i128::from(delta.num_seconds()) * MICROSECONDS_PER_SECOND
            + i128::from(delta.subsec_nanos() / i32::try_from(MILLISECONDS_PER_SECOND).expect("1000 fits in i32")),
    )
}

/// Builds a normalized timedelta from an arbitrary microsecond count.
pub(crate) fn from_total_microseconds(total_microseconds: i128) -> RunResult<TimeDelta> {
    let days = total_microseconds.div_euclid(DAY_MICROSECONDS);
    if !(i128::from(MIN_TIMEDELTA_DAYS)..=i128::from(MAX_TIMEDELTA_DAYS)).contains(&days) {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            format!("days={days}; must have magnitude <= 999999999"),
        )
        .into());
    }

    let seconds = total_microseconds.div_euclid(MICROSECONDS_PER_SECOND);
    let micros = total_microseconds.rem_euclid(MICROSECONDS_PER_SECOND);

    let seconds = i64::try_from(seconds)
        .map_err(|_| SimpleException::new_msg(ExcType::OverflowError, "timedelta value out of range"))?;
    let nanos = u32::try_from(micros * MILLISECONDS_PER_SECOND)
        .expect("microsecond remainder is in 0..1_000_000 and fits u32 nanoseconds");

    let delta = ChronoTimeDelta::new(seconds, nanos)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, "timedelta value out of range"))?;
    Ok(TimeDelta(delta))
}

/// Creates a `timedelta` from constructor arguments.
///
/// Supports positional `(days, seconds, microseconds)` and keyword arguments
/// `days`, `seconds`, `microseconds`, `milliseconds`, `minutes`, `hours`, `weeks`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut days = 0_i128;
    let mut seconds = 0_i128;
    let mut microseconds = 0_i128;
    let mut milliseconds = 0_i128;
    let mut minutes = 0_i128;
    let mut hours = 0_i128;
    let mut weeks = 0_i128;

    let mut seen_days = false;
    let mut seen_seconds = false;
    let mut seen_microseconds = false;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => {
                days = value_to_i128(arg, heap)?;
                seen_days = true;
            }
            1 => {
                seconds = value_to_i128(arg, heap)?;
                seen_seconds = true;
            }
            2 => {
                microseconds = value_to_i128(arg, heap)?;
                seen_microseconds = true;
            }
            _ => return Err(ExcType::type_error_at_most("timedelta", 3, index + 1)),
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let parsed = value_to_i128(value, heap)?;

        match key_name.string_id() {
            Some(id) if id == StaticStrings::Days => {
                if seen_days {
                    return Err(ExcType::type_error_multiple_values("timedelta", "days"));
                }
                days = parsed;
                seen_days = true;
            }
            Some(id) if id == StaticStrings::Seconds => {
                if seen_seconds {
                    return Err(ExcType::type_error_multiple_values("timedelta", "seconds"));
                }
                seconds = parsed;
                seen_seconds = true;
            }
            Some(id) if id == StaticStrings::Microseconds => {
                if seen_microseconds {
                    return Err(ExcType::type_error_multiple_values("timedelta", "microseconds"));
                }
                microseconds = parsed;
                seen_microseconds = true;
            }
            Some(id) if id == StaticStrings::Milliseconds => milliseconds = parsed,
            Some(id) if id == StaticStrings::Minutes => minutes = parsed,
            Some(id) if id == StaticStrings::Hours => hours = parsed,
            Some(id) if id == StaticStrings::Weeks => weeks = parsed,
            _ => {
                return Err(ExcType::type_error_unexpected_keyword(
                    "timedelta",
                    key_name.as_str(interns),
                ));
            }
        }
    }

    let total_microseconds = checked_component(weeks, 7 * DAY_MICROSECONDS)?
        + checked_component(days, DAY_MICROSECONDS)?
        + checked_component(hours, HOUR_MICROSECONDS)?
        + checked_component(minutes, MINUTE_MICROSECONDS)?
        + checked_component(seconds, MICROSECONDS_PER_SECOND)?
        + checked_component(milliseconds, MILLISECONDS_PER_SECOND)?
        + microseconds;

    let delta = from_total_microseconds(total_microseconds)?;
    Ok(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?))
}

fn checked_component(value: i128, unit_microseconds: i128) -> RunResult<i128> {
    value.checked_mul(unit_microseconds).ok_or_else(|| {
        SimpleException::new_msg(ExcType::OverflowError, "timedelta argument overflow while normalizing").into()
    })
}

fn value_to_i128(value: &Value, _heap: &Heap<impl ResourceTracker>) -> RunResult<i128> {
    match value {
        Value::Bool(b) => Ok(i128::from(i64::from(*b))),
        Value::Int(i) => Ok(i128::from(*i)),
        _ => Err(SimpleException::new_msg(ExcType::TypeError, "an integer is required (got type float)").into()),
    }
}

/// Formats a `TimeDelta` as its Python `repr()` string, e.g. `"datetime.timedelta(days=1, seconds=3600)"`.
///
/// Shared by `TimeDelta::py_repr_fmt` and `timezone.rs` for formatting offsets.
#[must_use]
pub(crate) fn format_repr(delta: &TimeDelta) -> String {
    let (days, seconds, microseconds) = components(delta);
    if days == 0 && seconds == 0 && microseconds == 0 {
        return "datetime.timedelta(0)".to_owned();
    }

    let mut repr = String::from("datetime.timedelta(");
    let mut first = true;
    if days != 0 {
        write!(repr, "days={days}").expect("writing to String cannot fail");
        first = false;
    }
    if seconds != 0 {
        if !first {
            repr.push_str(", ");
        }
        write!(repr, "seconds={seconds}").expect("writing to String cannot fail");
        first = false;
    }
    if microseconds != 0 {
        if !first {
            repr.push_str(", ");
        }
        write!(repr, "microseconds={microseconds}").expect("writing to String cannot fail");
    }
    repr.push(')');
    repr
}

impl HeapItem for TimeDelta {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `TimeDelta`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident timedeltas.
impl<'h> PyTrait<'h> for HeapRead<'h, TimeDelta> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::TimeDelta
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(total_microseconds(self.get(vm.heap)) == total_microseconds(other.get(vm.heap)))
    }

    fn py_cmp(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        Ok(total_microseconds(self.get(vm.heap)).partial_cmp(&total_microseconds(other.get(vm.heap))))
    }

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        total_microseconds(self.get(vm.heap)) != 0
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str(&format_repr(self.get(vm.heap)))?;
        Ok(())
    }

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<std::borrow::Cow<'static, str>> {
        let (days, seconds, microseconds) = components(self.get(vm.heap));
        let hours = seconds / SECONDS_PER_HOUR;
        let minutes = (seconds % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
        let second = seconds % SECONDS_PER_MINUTE;
        let time = if microseconds == 0 {
            format!("{hours}:{minutes:02}:{second:02}")
        } else {
            format!("{hours}:{minutes:02}:{second:02}.{microseconds:06}")
        };

        if days == 0 {
            return Ok(std::borrow::Cow::Owned(time));
        }

        let day_word = if days.abs() == 1 { "day" } else { "days" };
        Ok(std::borrow::Cow::Owned(format!("{days} {day_word}, {time}")))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        if attr.string_id() == Some(StaticStrings::TotalSeconds.into()) {
            // Copy the TimeDelta to release the HeapRead borrow before checking args
            let td = *self.get(vm.heap);
            args.check_zero_args("timedelta.total_seconds", vm.heap)?;
            return Ok(CallResult::Value(Value::Float(total_seconds(&td))));
        }
        Err(ExcType::attribute_error(Type::TimeDelta, attr.as_str(vm.interns)))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let (days, seconds, microseconds) = components(self.get(vm.heap));
        match attr.string_id() {
            Some(id) if id == StaticStrings::Days => Ok(Some(CallResult::Value(Value::Int(i64::from(days))))),
            Some(id) if id == StaticStrings::Seconds => Ok(Some(CallResult::Value(Value::Int(i64::from(seconds))))),
            Some(id) if id == StaticStrings::Microseconds => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(microseconds)))))
            }
            _ => Ok(None),
        }
    }
}
