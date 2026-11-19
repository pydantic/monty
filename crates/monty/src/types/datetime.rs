//! Python `datetime.datetime` implementation.
//!
//! Monty stores datetimes with Jiff civil primitives and layers CPython-compatible
//! constructor rules, aware/naive comparison semantics, and arithmetic on top.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use jiff::{
    SignedDuration,
    civil::{self, Date as CivilDate, DateTime as CivilDateTime, Time as CivilTime},
};
use monty_types::{
    DateTimeSource, MontyDateTime, MontyTimeZone, OsFunctionCall, ResourceTracker, SandboxTimeZone, local_wall_clock,
};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    py_value::PyValue,
    types::{
        CmpOrder, LazyHeapSet, PyTrait, TimeDelta, TimeZone, Type, TypeRef,
        date::{self, StrftimeArgs},
        str::{StringRepr, allocate_string, allocate_string_no_interning},
        time::{self, Time, TimeSpec},
        timedelta, timezone,
    },
    value::{EitherStr, Value},
};

pub(crate) mod strptime;

/// Shared overflow wording for datetime conversions.
const DATE_OUT_OF_RANGE: &str = "date value out of range";

/// `datetime.datetime` storage backed by Jiff civil components plus an optional fixed offset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DateTime {
    naive: CivilDateTime,
    offset_seconds: Option<i32>,
    timezone_name: Option<String>,
    /// Stable timezone object identity for aware datetimes.
    ///
    /// CPython preserves the original `tzinfo` object identity (`dt.tzinfo is tz`)
    /// and repeated `dt.tzinfo` access returns the same object. We store a retained
    /// heap reference so attribute lookup can return a stable object instead of
    /// allocating a new timezone each time.
    tzinfo_ref: Option<HeapId>,
}

impl DateTime {
    /// Returns the retained `tzinfo` heap reference, if this datetime is timezone-aware.
    ///
    /// Used by GC traversal (`collect_child_ids`) and ref-count cascade
    /// (`py_dec_ref_ids_for_data`) so that the timezone object stays alive as long
    /// as the datetime references it. Without this, `gc.collect` cannot reach the
    /// tzinfo and may sweep it while the datetime still points at the freed slot.
    pub(crate) fn tzinfo_ref(&self) -> Option<HeapId> {
        self.tzinfo_ref
    }
}

impl Hash for DateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash must be consistent with equality (py_eq).
        if is_aware(self) {
            // Aware datetimes compare equal if they represent the same UTC instant,
            // regardless of their local offset or timezone name.
            let _ = utc_micros(self).inspect(|m| m.hash(state));
        } else {
            // Naive datetimes compare equal if they have the same local fields.
            local_micros(self).hash(state);
        }
    }
}

/// Creates a datetime from civil components and optional fixed offset.
#[expect(clippy::too_many_arguments)]
pub(crate) fn from_components(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    tzinfo: Option<TimeZone>,
    tzinfo_ref: Option<HeapId>,
    heap: &mut Heap,
) -> RunResult<DateTime> {
    if !(0..=23).contains(&hour) {
        return Err(SimpleException::new_msg(ExcType::ValueError, format!("hour must be in 0..23, not {hour}")).into());
    }
    if !(0..=59).contains(&minute) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("minute must be in 0..59, not {minute}")).into(),
        );
    }
    if !(0..=59).contains(&second) {
        return Err(ExcType::second_out_of_range(second));
    }
    if !(0..=999_999).contains(&microsecond) {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("microsecond must be in 0..999999, not {microsecond}"),
        )
        .into());
    }

    // Delegate all date-component validation to `date::from_ymd` so date and datetime
    // constructors stay in lockstep on CPython-compatible error behavior.
    let date_value = date::from_ymd(year, month, day)?;
    let time = CivilTime::new(
        i8::try_from(hour).expect("validated hour"),
        i8::try_from(minute).expect("validated minute"),
        i8::try_from(second).expect("validated second"),
        microsecond * 1_000,
    )
    .expect("validated time components");

    let (offset_seconds, timezone_name) = match tzinfo {
        Some(tz) => (Some(tz.offset_seconds), tz.name),
        None => (None, None),
    };
    if let Some(offset_seconds) = offset_seconds
        && !(-86_399..=86_399).contains(&offset_seconds)
    {
        return Err(SimpleException::new_msg(ExcType::ValueError, "timezone offset out of range").into());
    }

    let mut datetime = DateTime {
        naive: date_value.0.to_datetime(time),
        offset_seconds,
        timezone_name,
        tzinfo_ref: None,
    };

    attach_or_allocate_tzinfo_ref(&mut datetime, tzinfo_ref, heap);
    Ok(datetime)
}

/// Allocates a naive `datetime` from already-in-range components.
///
/// For the `datetime.min` / `datetime.max` class constants; anything derived
/// from user input must go through [`from_components`] to be validated. Takes
/// `&mut Heap`, unlike its siblings, because that shared path may allocate a
/// timezone.
#[expect(clippy::too_many_arguments)]
pub(crate) fn allocate_naive(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    heap: &mut Heap,
) -> Value {
    let datetime = from_components(year, month, day, hour, minute, second, microsecond, None, None, heap)
        .expect("caller guarantees in-range datetime components");
    Value::Ref(heap.allocate(HeapData::DateTime(datetime)))
}

/// Returns true when this is an aware datetime.
#[must_use]
pub(crate) fn is_aware(datetime: &DateTime) -> bool {
    datetime.offset_seconds.is_some()
}

/// Returns the fixed offset seconds for aware datetimes.
#[must_use]
pub(crate) fn offset_seconds(datetime: &DateTime) -> Option<i32> {
    datetime.offset_seconds
}

/// Returns timezone metadata for aware datetimes.
#[must_use]
pub(crate) fn timezone_info(datetime: &DateTime) -> Option<TimeZone> {
    datetime.offset_seconds.map(|offset_seconds| TimeZone {
        offset_seconds,
        name: datetime.timezone_name.clone(),
    })
}

/// Returns civil components in compact integer widths for object conversion.
#[must_use]
pub(crate) fn to_components(datetime: &DateTime) -> Option<(i32, u8, u8, u8, u8, u8, u32)> {
    let year = i32::from(datetime.naive.year());
    if !year_in_python_range(year) {
        return None;
    }

    Some((
        year,
        u8::try_from(datetime.naive.date().month()).expect("month is always in 1..=12"),
        u8::try_from(datetime.naive.date().day()).expect("day is always in 1..=31"),
        u8::try_from(datetime.naive.time().hour()).expect("hour is always in 0..=23"),
        u8::try_from(datetime.naive.time().minute()).expect("minute is always in 0..=59"),
        u8::try_from(datetime.naive.time().second()).expect("second is always in 0..=59"),
        u32::try_from(datetime.naive.subsec_nanosecond() / 1_000).expect("positive microseconds"),
    ))
}

/// The host-value form of a datetime, or `None` when its year is outside 1..=9999.
#[must_use]
pub(crate) fn to_monty_datetime(datetime: &DateTime) -> Option<MontyDateTime> {
    let (year, month, day, hour, minute, second, microsecond) = to_components(datetime)?;
    Some(MontyDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        microsecond,
        offset_seconds: datetime.offset_seconds,
        timezone_name: datetime.timezone_name.clone(),
    })
}

/// Constructor for `datetime(...)`.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DatetimeInitArgs {
        year,
        month,
        day,
        hour,
        minute,
        second,
        microsecond,
        tzinfo,
        fold,
    } = DatetimeInitArgs::from_args(args, vm)?;
    // `tzinfo` owns the input ref; keep it alive across `tzinfo_from_value` and
    // `from_components` so the heap-allocated TimeZone (if any) is not freed
    // before `attach_or_allocate_tzinfo_ref` takes its own reference.
    defer_drop_mut!(tzinfo, vm);

    if fold != 0 && fold != 1 {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("fold must be either 0 or 1, not {fold}")).into(),
        );
    }

    let (tz, tz_ref) = tzinfo_from_value(tzinfo, vm.heap, vm.interns)?;
    let dt = from_components(year, month, day, hour, minute, second, microsecond, tz, tz_ref, vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(dt))))
}

/// Argument shape for `datetime(year, month, day, hour=0, minute=0, second=0,
/// microsecond=0, tzinfo=None, *, fold=0)`.
///
/// CPython emits two distinct wordings for over-arity: when the overflow
/// could still fit in the keyword-only tail (`actual <= 9`) the message is
/// "function takes at most 8 *positional* arguments"; once it exceeds the
/// total slot count it switches to "function takes at most 9 arguments".
/// The derive turns the pivot on automatically for `style = c` structs with
/// keyword-only fields — the trailing kw-only `fold` slot is what bumps
/// `max_total` to 9.
///
/// `fold` itself is accepted for CPython parity but currently has no effect
/// on the stored datetime — Monty does not track DST-fold disambiguation.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct DatetimeInitArgs {
    year: i32,
    month: i32,
    day: i32,
    #[from_args(default = 0)]
    hour: i32,
    #[from_args(default = 0)]
    minute: i32,
    #[from_args(default = 0)]
    second: i32,
    #[from_args(default = 0)]
    microsecond: i32,
    #[from_args(default = Value::None)]
    tzinfo: Value,
    #[from_args(kw_only, default = 0)]
    fold: i32,
}

/// `datetime.min`, `datetime.max` and `datetime.resolution`.
pub(crate) fn class_attr(attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
    Some(match attr.static_string(vm.interns)? {
        StaticStrings::Min => allocate_naive(1, 1, 1, 0, 0, 0, 0, vm.heap),
        StaticStrings::Max => allocate_naive(9999, 12, 31, 23, 59, 59, 999_999, vm.heap),
        StaticStrings::Resolution => timedelta::allocate_micros(1, vm.heap),
        _ => return None,
    })
}

/// `datetime.now()`, `fromtimestamp()`, `strptime()`, `fromisoformat()` and `combine()`.
pub(crate) fn class_method(
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Result<RunResult<CallResult>, ArgValues> {
    Ok(match attr.static_string(vm.interns) {
        Some(StaticStrings::Now) => class_now(vm, args),
        Some(StaticStrings::Fromtimestamp) => class_fromtimestamp(vm, args).map(CallResult::Value),
        Some(StaticStrings::Strptime) => class_strptime(vm, args).map(CallResult::Value),
        Some(StaticStrings::Fromisoformat) => class_fromisoformat(vm.heap, args, vm.interns).map(CallResult::Value),
        Some(StaticStrings::Combine) => class_combine(vm, args).map(CallResult::Value),
        _ => return Err(args),
    })
}

/// Reads `datetime.now(tz=None)` from the session clock, preserving `tz` identity.
/// Naive results use the session zone. A `CallHost` clock yields `DateTimeNow`
/// with a validated [`Option<MontyTimeZone>`] for the host to answer.
pub(crate) fn class_now(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let NowArgs { tz } = NowArgs::from_args(args, vm)?;
    defer_drop!(tz, vm);
    let (tz, tz_ref) = tzinfo_from_value(tz, vm.heap, vm.interns)?;
    // Invalid fixed instants raise; only CallHost falls back to the host's clock.
    let Some(utc) = sandbox_instant(vm)? else {
        let tz = tz.map(|tz| MontyTimeZone {
            offset_seconds: tz.offset_seconds,
            name: tz.name,
        });
        return Ok(CallResult::OsCall(OsFunctionCall::DateTimeNow(tz)));
    };
    let mut dt = match &tz {
        Some(tz) => from_utc_naive_with_timezone_parts(utc, tz.offset_seconds, tz.name.clone()),
        None => from_local_naive(sandbox_local_wall_clock(vm, utc)?),
    }
    .ok_or_else(date_out_of_range)?;
    attach_or_allocate_tzinfo_ref(&mut dt, tz_ref, vm.heap);
    Ok(CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::DateTime(dt)))))
}

/// `datetime.fromtimestamp(timestamp, tz=None)`: the instant `timestamp` seconds
/// after the Unix epoch, as wall-clock time in `tz`, or in the session zone when
/// `tz` is `None` (CPython reads the host's zone there).
pub(crate) fn class_fromtimestamp(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FromtimestampArgs { timestamp, tz } = FromtimestampArgs::from_args(args, vm)?;
    defer_drop!(timestamp, vm);
    defer_drop!(tz, vm);
    let (tz, tz_ref) = tzinfo_from_value(tz, vm.heap, vm.interns)?;
    let (seconds, micros) = timestamp_parts(timestamp, TimestampRounding::HalfEven, vm)?
        .ok_or_else(|| timestamp_not_a_number(timestamp, vm))?;
    let mut dt = if let Some(tz) = tz {
        // CPython builds the UTC value first, so its year error comes first, and a
        // shifted result that leaves the range is `fromutc()`'s overflow.
        if !(1..=9999).contains(&civil_year(seconds)) {
            return Err(beyond_civil_range(seconds));
        }
        DateTime {
            naive: timestamp_wall_clock(seconds, micros, tz.offset_seconds).map_err(|_| date_out_of_range())?,
            offset_seconds: Some(tz.offset_seconds),
            timezone_name: tz.name,
            tzinfo_ref: None,
        }
    } else {
        let offset = timestamp_zone(seconds, micros, &vm.env.os_policy.timezone).offset_seconds;
        let naive = timestamp_wall_clock(seconds, micros, offset)?;
        // CPython probes a day earlier to detect a DST fold, which can raise at year 1.
        timestamp_wall_clock(seconds - 86_400, micros, offset)?;
        DateTime {
            naive,
            offset_seconds: None,
            timezone_name: None,
            tzinfo_ref: None,
        }
    };
    attach_or_allocate_tzinfo_ref(&mut dt, tz_ref, vm.heap);
    Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(dt))))
}

/// Argument shape for `datetime.fromtimestamp(timestamp, tz=None)`, a
/// `PyArg_ParseTupleAndKeywords` parser: `at_most_total` gives CPython's
/// `fromtimestamp() takes at most 2 arguments (3 given)` for a duplicated `tz`.
#[derive(FromArgs)]
#[from_args(name = "fromtimestamp", style = c_named, at_most_total)]
struct FromtimestampArgs {
    timestamp: Value,
    #[from_args(default = Value::None)]
    tz: Value,
}

/// How a float `fromtimestamp()` argument is cut to the result's precision:
/// `datetime` rounds half-even to microseconds, `date` floors to whole seconds.
#[derive(Clone, Copy)]
pub(crate) enum TimestampRounding {
    HalfEven,
    Floor,
}

/// Resolves an instant's zone even when its UTC year lies outside the civil range.
/// Clamping only affects the rules lookup; wall-clock validation uses the full instant.
pub(crate) fn timestamp_zone(seconds: i64, micros: u32, zone: &SandboxTimeZone) -> MontyTimeZone {
    let utc = civil::date(1970, 1, 1)
        .at(0, 0, 0, 0)
        .checked_add(SignedDuration::new(
            seconds,
            i32::try_from(micros * 1_000).expect("subsecond nanoseconds"),
        ))
        .unwrap_or(if seconds < 0 {
            CivilDateTime::MIN
        } else {
            CivilDateTime::MAX
        });
    zone.at(utc)
}

/// Converts epoch seconds to wall time without requiring the UTC intermediate
/// to fit in Jiff's civil range (year 10000 can shift back into year 9999).
pub(crate) fn timestamp_wall_clock(seconds: i64, micros: u32, offset: i32) -> RunResult<CivilDateTime> {
    let shifted = SignedDuration::new(seconds, i32::try_from(micros * 1_000).expect("subsecond nanoseconds"))
        .checked_add(SignedDuration::from_secs(i64::from(offset)));
    let wall = shifted
        .and_then(|duration| civil::date(1970, 1, 1).at(0, 0, 0, 0).checked_add(duration).ok())
        .ok_or_else(|| beyond_civil_range(seconds.saturating_add(i64::from(offset))))?;
    if year_in_python_range(wall.year()) {
        Ok(wall)
    } else {
        Err(date::year_out_of_range(wall.year()))
    }
}

/// The error for a timestamp outside Jiff's civil range. CPython's `localtime()` still
/// holds the year while `year - 1900` fits a C `int`, so it raises the year error;
/// past that it raises `OSError`, which Monty words as the `time_t` overflow.
/// The zone offset is ignored: it can only move the year within hours of a new year.
#[cold]
fn beyond_civil_range(seconds: i64) -> RunError {
    let year = civil_year(seconds);
    if i32::try_from(year - 1900).is_ok() {
        date::year_out_of_range(year)
    } else {
        ExcType::timestamp_out_of_range()
    }
}

/// The proleptic Gregorian year of the instant `seconds` after the Unix epoch, for any
/// `i64` (Howard Hinnant's `civil_from_days`), including years outside Jiff's range.
fn civil_year(seconds: i64) -> i64 {
    let shifted = seconds.div_euclid(86_400) + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Years run March to February here, so January and February belong to the next one.
    let march_based_month = (5 * day_of_year + 2) / 153;
    year_of_era + era * 400 + i64::from(march_based_month >= 10)
}

/// Whole seconds and microseconds of a timestamp argument, as CPython's
/// `_PyTime_ObjectToTimeval` / `_PyTime_ObjectToTime_t` read it; shared with the
/// `time` module. `None` when it is not a number and has no `__index__`.
pub(crate) fn timestamp_parts(
    value: &Value,
    rounding: TimestampRounding,
    vm: &mut VM<'_>,
) -> RunResult<Option<(i64, u32)>> {
    match value.py_value(vm) {
        PyValue::Bool(b) => Ok(Some((i64::from(b), 0))),
        PyValue::Int(integer) => Ok(Some((
            integer.to_i64(vm).ok_or_else(ExcType::timestamp_out_of_range)?,
            0,
        ))),
        PyValue::Float(f) => split_float_timestamp(f, rounding).map(Some),
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                defer_drop!(index, vm);
                timestamp_parts(index, rounding, vm)
            }
            None => Ok(None),
        },
    }
}

/// Splits a float timestamp into whole seconds and microseconds in `0..1_000_000`,
/// following CPython's `_PyTime_DoubleToDenominator` (half-even) or its
/// `_PyTime_ROUND_FLOOR` conversion to whole seconds.
// Both casts are range-checked: `whole` fits `i64` and `fraction` is a rounded value in `0..1_000_000`.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn split_float_timestamp(value: f64, rounding: TimestampRounding) -> RunResult<(i64, u32)> {
    if value.is_nan() {
        return Err(ExcType::invalid_nan());
    }
    let (mut whole, mut fraction) = match rounding {
        TimestampRounding::HalfEven => {
            let whole = value.trunc();
            (whole, ((value - whole) * 1_000_000.0).round_ties_even())
        }
        TimestampRounding::Floor => (value.floor(), 0.0),
    };
    if fraction >= 1_000_000.0 {
        fraction -= 1_000_000.0;
        whole += 1.0;
    } else if fraction < 0.0 {
        fraction += 1_000_000.0;
        whole -= 1.0;
    }
    // `time_t` is 64 bits on every supported platform; the comparison also rejects infinities.
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&whole) {
        return Err(ExcType::timestamp_out_of_range());
    }
    Ok((whole as i64, fraction as u32))
}

/// The wall clock at `offset_seconds` from the UTC instant `utc`, raising CPython's
/// `year must be in 1..9999` when the result leaves `datetime`'s range.
pub(crate) fn wall_clock_at(utc: CivilDateTime, offset_seconds: i32) -> RunResult<CivilDateTime> {
    local_wall_clock(utc, offset_seconds).ok_or_else(|| {
        let seconds =
            i64::try_from(epoch_duration(utc).as_nanos().div_euclid(1_000_000_000)).expect("civil seconds fit i64");
        date::year_out_of_range(civil_year(seconds + i64::from(offset_seconds)))
    })
}

/// Elapsed time from the Unix epoch, without the narrower bounds of a Jiff timestamp.
pub(crate) fn epoch_duration(datetime: CivilDateTime) -> SignedDuration {
    datetime.duration_since(civil::date(1970, 1, 1).at(0, 0, 0, 0))
}

#[cold]
fn timestamp_not_a_number(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!("argument must be int or float, not {}", value.py_type_name(vm)))
}

/// `datetime.astimezone(tz=None)`: the same instant in `tz`, or in the sandbox
/// zone at that instant when `tz` is `None`. A naive value is read as sandbox-local
/// wall time, as CPython reads it in the host's zone.
fn astimezone(dt: &DateTime, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let AstimezoneArgs { tz } = AstimezoneArgs::from_args(args, vm)?;
    defer_drop!(tz, vm);
    let (tz, tz_ref) = tzinfo_from_value(tz, vm.heap, vm.interns)?;
    let zone = &vm.env.os_policy.timezone;
    // CPython forms `self - utcoffset()` as a datetime, so the UTC intermediate
    // must be in range as well as the result.
    let utc = if dt.offset_seconds.is_some() {
        to_utc_naive(dt).filter(|utc| year_in_python_range(utc.year()))
    } else {
        probe_neighbouring_days(dt.naive.date())?;
        zone.utc_from_local(dt.naive)
    }
    .ok_or_else(date_out_of_range)?;
    let target = match tz {
        Some(tz) => MontyTimeZone {
            offset_seconds: tz.offset_seconds,
            name: tz.name,
        },
        None => zone.at(utc),
    };
    let mut converted =
        from_utc_naive_with_timezone_parts(utc, target.offset_seconds, target.name).ok_or_else(date_out_of_range)?;
    attach_or_allocate_tzinfo_ref(&mut converted, tz_ref, vm.heap);
    Ok(CallResult::Value(Value::Ref(
        vm.heap.allocate(HeapData::DateTime(converted)),
    )))
}

/// CPython finds a naive value's local offset by rendering the days either side
/// of it, so the first and last representable days raise before any conversion
/// happens — in every zone, including UTC. Monty needs no such probe, but the
/// error is observable, so it is reproduced.
fn probe_neighbouring_days(date: CivilDate) -> RunResult<()> {
    if date == civil::date(1, 1, 1) {
        Err(date::year_out_of_range(0))
    } else if date == CivilDate::MAX {
        Err(date::year_out_of_range(10_000))
    } else {
        Ok(())
    }
}

/// Argument shape for `datetime.astimezone(tz=None)`, checked like [`NowArgs`]:
/// `at_most_total` gives CPython's `astimezone() takes at most 1 argument (2 given)`.
#[derive(FromArgs)]
#[from_args(name = "astimezone", at_most_total)]
struct AstimezoneArgs {
    #[from_args(default = Value::None)]
    tz: Value,
}

/// Reads the session clock in UTC; `None` means `CallHost` and requires suspension.
/// Unrepresentable fixed instants raise `OverflowError` for all three clock calls.
pub(crate) fn sandbox_instant(vm: &VM<'_>) -> RunResult<Option<CivilDateTime>> {
    match vm.env.os_policy.datetime {
        DateTimeSource::CallHost => Ok(None),
        source => source.read().map(Some).ok_or_else(date_out_of_range),
    }
}

/// Converts UTC to the session zone's wall clock for naive `now()` and `today()`;
/// out-of-range years raise `OverflowError`.
pub(crate) fn sandbox_local_wall_clock(vm: &VM<'_>, utc: CivilDateTime) -> RunResult<CivilDateTime> {
    let offset = vm.env.os_policy.timezone.at(utc).offset_seconds;
    local_wall_clock(utc, offset).ok_or_else(date_out_of_range)
}

fn date_out_of_range() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into()
}

/// Argument shape for `datetime.now(tz=None)`.
///
/// `tz` stays a raw [`Value`] so the None-or-timezone check runs in the body
/// (`tzinfo_from_value`) with CPython's tzinfo wording. `at_most_total`
/// matches CPython's `PyArg_ParseTupleAndKeywords` pre-count: a duplicate tz
/// (`now(utc, tz=utc)`) reports "takes at most 1 argument (2 given)".
#[derive(FromArgs)]
#[from_args(name = "now", at_most_total)]
struct NowArgs {
    #[from_args(default = Value::None)]
    tz: Value,
}

/// Classmethod `datetime.strptime(date_string, format)`.
///
/// Parses civil components with Python-compatible microseconds and UTC offsets.
pub(crate) fn class_strptime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let heap = &mut *vm.heap;
    let interns = vm.interns;
    let (date_string_val, format_val) = args.get_two_args("datetime.strptime", heap)?;

    let date_string = date::extract_str_arg(&date_string_val, "strptime", heap, interns);
    let fmt = date::extract_str_arg(&format_val, "strptime", heap, interns);
    date_string_val.drop_with(heap);
    format_val.drop_with(heap);
    let date_string = date_string?;
    let fmt = fmt?;

    let strptime::ParsedTime {
        wall: naive,
        second,
        offset_seconds,
    } = strptime::parse(&date_string, &fmt, vm.env.os_policy, &heap.tracker)?;

    if !year_in_python_range(naive.date().year()) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "year is out of range").into());
    }
    if let Some(offset_seconds) = offset_seconds {
        timezone::check_offset_seconds(offset_seconds)?;
    }
    if second > 59 {
        return Err(ExcType::second_out_of_range(second));
    }

    let mut dt = DateTime {
        naive,
        offset_seconds,
        timezone_name: None,
        tzinfo_ref: None,
    };
    attach_or_allocate_tzinfo_ref(&mut dt, None, heap);
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))))
}

/// Classmethod `datetime.fromisoformat(date_string)`.
///
/// Parses ISO 8601 datetime strings. Supports the following formats:
/// - `YYYY-MM-DD` (date only, time defaults to midnight)
/// - `YYYY-MM-DDTHH:MM` or `YYYY-MM-DD HH:MM`
/// - `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD HH:MM:SS`
/// - `YYYY-MM-DDTHH:MM:SS.ffffff`
/// - Any of the above with `+HH:MM` or `+HH:MM:SS` timezone suffix
pub(crate) fn class_fromisoformat(heap: &mut Heap, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args
        .reject_kwargs("datetime.fromisoformat", heap)?
        .get_one_arg("datetime.fromisoformat", heap)?;
    let s = date::extract_str_arg(&value, "fromisoformat", heap, interns);
    value.drop_with(heap);
    let s = s?;

    let dt = parse_iso_datetime(&s, heap)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("Invalid isoformat string: '{s}'")))?;

    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))))
}

/// `datetime.combine(date, time, tzinfo=self.tzinfo)`.
///
/// The `date` argument may itself be a `datetime` (CPython makes `datetime` a
/// `date` subclass), in which case only its date part is used. The result takes
/// the time's timezone unless a third argument overrides it; passing `None`
/// explicitly is how CPython drops an aware time's zone.
pub(crate) fn class_combine(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CombineArgs { date, time, tzinfo } = CombineArgs::from_args(args, vm)?;
    defer_drop!(date, vm);
    defer_drop!(time, vm);
    // Guarded before the two type checks below, since both can fail while
    // holding the caller's `tzinfo` reference.
    defer_drop!(tzinfo, vm);

    let date_part = combine_date_part(date, vm)?;
    let time_part = combine_time_part(time, vm)?;

    // Absent keeps the time's zone; present (including `None`) replaces it.
    // Either way the zone is borrowed, so the guard above has to outlive
    // `combine_allocate` — which takes its own ref via `from_components`.
    match tzinfo.as_ref() {
        None => {
            let tz = (time::attached_timezone(&time_part, vm.heap), time_part.tzinfo_ref());
            combine_allocate(vm, date_part, &time_part, tz)
        }
        Some(tzinfo) => {
            let tz = tzinfo_from_value(tzinfo, vm.heap, vm.interns)?;
            combine_allocate(vm, date_part, &time_part, tz)
        }
    }
}

/// Allocates the `datetime` `combine` returns, given its resolved parts.
fn combine_allocate(
    vm: &mut VM<'_>,
    (year, month, day): (i32, i32, i32),
    time: &Time,
    (tz, tz_ref): (Option<TimeZone>, Option<HeapId>),
) -> RunResult<Value> {
    let (hour, minute, second, microsecond) = time.components_i32();
    let combined = from_components(year, month, day, hour, minute, second, microsecond, tz, tz_ref, vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(combined))))
}

/// Argument shape for `datetime.combine(date, time, tzinfo=self.tzinfo)`.
///
/// Every field stays a raw [`Value`] so the type checks run in the body with
/// CPython's own wording (`combine() argument 1 must be datetime.date, not
/// str`, and the shared `tzinfo argument must be ...` message).
#[derive(FromArgs)]
#[from_args(name = "combine", style = c_named, at_most_total)]
struct CombineArgs {
    date: Value,
    time: Value,
    // `Option<Value>` keeps "omitted" (inherit the time's zone) distinct from an
    // explicit `tzinfo=None` (make the result naive).
    #[from_args(default)]
    tzinfo: Option<Value>,
}

/// Extracts `(year, month, day)` from `combine`'s first argument.
fn combine_date_part(date: &Value, vm: &mut VM<'_>) -> RunResult<(i32, i32, i32)> {
    let naive = match date {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Date(d) => Some(d.0),
            HeapData::DateTime(dt) => Some(dt.naive.date()),
            _ => None,
        },
        _ => None,
    };
    let naive = naive.ok_or_else(|| {
        ExcType::type_error_bad_arg_pos(
            "combine",
            1,
            "datetime.date",
            date.py_type_heap(vm.heap).cpython_arg_name(vm.heap, vm.interns),
        )
    })?;
    Ok((
        i32::from(naive.year()),
        i32::from(naive.month()),
        i32::from(naive.day()),
    ))
}

/// Extracts the `Time` from `combine`'s second argument.
///
/// The clone shares the original's `tzinfo_ref` without taking a reference to
/// it, so it is only safe while the caller's guard keeps the source `time`
/// alive — which is exactly how long [`class_combine`] uses it.
fn combine_time_part(time: &Value, vm: &mut VM<'_>) -> RunResult<Time> {
    match time {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Time(t) => Ok(t.clone()),
            _ => Err(combine_bad_time_arg(time, vm)),
        },
        _ => Err(combine_bad_time_arg(time, vm)),
    }
}

/// The `combine() argument 2 must be datetime.time, not X` error.
fn combine_bad_time_arg(time: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error_bad_arg_pos(
        "combine",
        2,
        "datetime.time",
        time.py_type_heap(vm.heap).cpython_arg_name(vm.heap, vm.interns),
    )
}

/// Parses an ISO 8601 datetime string into a `DateTime`.
///
/// Uses speedate's RFC 3339 parser for Python-compatible ISO 8601 parsing (the
/// same parser used by pydantic). Falls back to date-only parsing for bare
/// `YYYY-MM-DD` inputs.
fn parse_iso_datetime(s: &str, heap: &mut Heap) -> Option<DateTime> {
    let bytes = s.as_bytes();

    // Try full datetime first, then fall back to date-only (defaults to midnight)
    if let Ok(parsed) = speedate::DateTime::parse_bytes_rfc3339(bytes) {
        let d = &parsed.date;
        let t = &parsed.time;
        let tz = t.tz_offset.map(|offset_seconds| TimeZone {
            offset_seconds,
            name: None,
        });
        from_components(
            i32::from(d.year),
            i32::from(d.month),
            i32::from(d.day),
            i32::from(t.hour),
            i32::from(t.minute),
            i32::from(t.second),
            i32::try_from(t.microsecond).unwrap_or(0),
            tz,
            None,
            heap,
        )
        .ok()
    } else {
        // Date-only input: parse as date, default time to midnight
        let d = speedate::Date::parse_bytes(bytes).ok()?;
        from_components(
            i32::from(d.year),
            i32::from(d.month),
            i32::from(d.day),
            0,
            0,
            0,
            0,
            None,
            None,
            heap,
        )
        .ok()
    }
}

/// `datetime + timedelta`, raising CPython's `OverflowError` outside years 1 to 9999.
pub(crate) fn py_add(datetime: &DateTime, delta: &TimeDelta, heap: &mut Heap) -> RunResult<Value> {
    offset_datetime(datetime, timedelta::signed_duration(delta), heap)
}

/// `datetime - timedelta`, raising CPython's `OverflowError` outside years 1 to 9999.
pub(crate) fn py_sub_timedelta(datetime: &DateTime, delta: &TimeDelta, heap: &mut Heap) -> RunResult<Value> {
    // Negate the duration, not the Python timedelta, which overflows for timedelta.max.
    offset_datetime(datetime, -timedelta::signed_duration(delta), heap)
}

/// The datetime `delta` after `datetime`, or CPython's `OverflowError` outside years 1 to 9999.
fn offset_datetime(datetime: &DateTime, delta: SignedDuration, heap: &mut Heap) -> RunResult<Value> {
    // Anchor at the epoch: Jiff bounds a duration's day count as an epoch day,
    // which would otherwise reject a valid shift from year 1 to year 9999.
    let naive = epoch_duration(datetime.naive)
        .checked_add(delta)
        .and_then(|duration| civil::date(1970, 1, 1).at(0, 0, 0, 0).checked_add(duration).ok())
        .ok_or_else(date::date_value_out_of_range)?;
    if !year_in_python_range(naive.year()) {
        return Err(date::date_value_out_of_range());
    }
    let mut next = DateTime {
        naive,
        tzinfo_ref: None,
        ..datetime.clone()
    };
    attach_or_allocate_tzinfo_ref(&mut next, datetime.tzinfo_ref, heap);
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(next))))
}

/// `datetime - datetime` returns a timedelta with the difference.
///
/// Both datetimes must be either aware or naive; mixing returns `None`.
pub(crate) fn py_sub_datetime(a: &DateTime, b: &DateTime, heap: &mut Heap) -> Option<Value> {
    if is_aware(a) != is_aware(b) {
        return None;
    }

    let diff = i128::from(utc_micros(a)?) - i128::from(utc_micros(b)?);
    let delta = timedelta::from_total_microseconds(diff).ok()?;
    Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))))
}

/// Validates a `tzinfo` argument and extracts its timezone data plus heap identity.
///
/// Returns `(None, None)` for `None` and `(Some(tz), Some(id))` for a `timezone`
/// instance — the two are always both present or both absent, which is what lets
/// callers keep "aware implies an attached tzinfo object" as an invariant. Any
/// other type is rejected with CPython's wording. Shared with [`time`] so both
/// constructors accept exactly the same tzinfo surface.
///
/// [`time`]: crate::types::time
pub(crate) fn tzinfo_from_value(
    value: &Value,
    heap: &Heap,
    interns: &Interns,
) -> RunResult<(Option<TimeZone>, Option<HeapId>)> {
    match value {
        Value::None => Ok((None, None)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(tz) => Ok((Some(tz.clone()), Some(*id))),
            other => Err(ExcType::type_error_tzinfo(&other.py_type().name(heap, interns))),
        },
        _ => Err(ExcType::type_error_tzinfo(value.py_type_shallow().name())),
    }
}

/// Attaches a stable tzinfo identity to aware datetimes.
///
/// If `preferred_tzinfo_ref` is provided, it is retained and reused so identity
/// semantics (`is`) match the input timezone object. Otherwise we allocate (or
/// canonicalize to the UTC singleton) a timezone object once and reuse it.
fn attach_or_allocate_tzinfo_ref(datetime: &mut DateTime, preferred_tzinfo_ref: Option<HeapId>, heap: &mut Heap) {
    let Some(offset_seconds) = datetime.offset_seconds else {
        datetime.tzinfo_ref = None;
        return;
    };

    let tzinfo_ref = if let Some(tzinfo_ref) = preferred_tzinfo_ref {
        heap.inc_ref(tzinfo_ref);
        tzinfo_ref
    } else {
        allocate_tzinfo_ref(offset_seconds, datetime.timezone_name.clone(), heap)
    };
    datetime.tzinfo_ref = Some(tzinfo_ref);
}

/// Allocates a timezone object for datetime storage, canonicalizing UTC to the
/// shared singleton object.
///
/// Returns an *owned* reference, so the caller must hand it to a field that
/// releases it (`DateTime::tzinfo_ref`, `Time::tzinfo_ref`).
pub(crate) fn allocate_tzinfo_ref(offset_seconds: i32, timezone_name: Option<String>, heap: &mut Heap) -> HeapId {
    if offset_seconds == 0 && timezone_name.is_none() {
        let utc = heap.get_timezone_utc();
        defer_drop!(utc, heap);
        let Value::Ref(id) = utc else {
            unreachable!("timezone.utc must be heap-allocated");
        };
        heap.inc_ref(*id);
        return *id;
    }
    let tz = TimeZone {
        offset_seconds,
        name: timezone_name,
    };
    heap.allocate(HeapData::TimeZone(tz))
}

/// Returns local wall-clock microseconds since Unix epoch for the datetime.
#[must_use]
pub(crate) fn local_micros(datetime: &DateTime) -> Option<i64> {
    if !year_in_python_range(datetime.naive.date().year()) {
        return None;
    }
    i64::try_from(epoch_duration(datetime.naive).as_micros()).ok()
}

/// Returns UTC microseconds since Unix epoch for aware datetimes, otherwise local micros.
#[must_use]
pub(crate) fn utc_micros(datetime: &DateTime) -> Option<i64> {
    let local = local_micros(datetime)?;
    Some(local - i64::from(datetime.offset_seconds.unwrap_or(0)) * 1_000_000)
}

/// Constructs a naive datetime only when its year is in Python's range.
fn from_local_naive(naive: CivilDateTime) -> Option<DateTime> {
    if !year_in_python_range(naive.date().year()) {
        return None;
    }
    Some(DateTime {
        naive,
        offset_seconds: None,
        timezone_name: None,
        tzinfo_ref: None,
    })
}

/// Constructs an aware datetime in a fixed offset, bounding its local year.
fn from_utc_naive_with_timezone_parts(
    utc_naive: CivilDateTime,
    offset_seconds: i32,
    timezone_name: Option<String>,
) -> Option<DateTime> {
    if !(-86_399..=86_399).contains(&offset_seconds) {
        return None;
    }
    let offset_delta = SignedDuration::from_secs(i64::from(offset_seconds));
    let local = utc_naive.checked_add(offset_delta).ok()?;
    if !year_in_python_range(local.date().year()) {
        return None;
    }
    Some(DateTime {
        naive: local,
        offset_seconds: Some(offset_seconds),
        timezone_name,
        tzinfo_ref: None,
    })
}

/// UTC civil components for aware conversions, or `None` outside Jiff's range.
fn to_utc_naive(datetime: &DateTime) -> Option<CivilDateTime> {
    let offset_seconds = datetime.offset_seconds?;
    let offset_delta = SignedDuration::from_secs(i64::from(offset_seconds));
    datetime.naive.checked_sub(offset_delta).ok()
}

#[must_use]
fn year_in_python_range(year: impl Into<i32>) -> bool {
    (1..=9999).contains(&year.into())
}

/// Formats a [`DateTime`] with a `strftime` directive string, shared by the
/// `datetime.strftime()` method and f-string formatting (`f"{dt:%Y-%m-%d}"`).
///
/// Unknown directives pass through as on glibc/Linux CPython.
/// Zone directives come from the stored offset and name, or are empty when naive.
pub(crate) fn format_datetime_strftime(dt: &DateTime, format: &str, tracker: &ResourceTracker) -> RunResult<String> {
    let format = date::rewrite_zone_directives(format, dt.offset_seconds, dt.timezone_name.as_deref(), tracker)?;
    date::render_strftime(dt.naive, &format, tracker)
}

/// Formats a datetime as an ISO 8601 string with the given separator, at the
/// clock precision `spec` asks for.
///
/// Matches CPython's `datetime.isoformat(sep='T', timespec='auto')`.
fn format_isoformat(dt: &DateTime, sep: char, spec: TimeSpec) -> String {
    let Some((year, month, day, hour, minute, second, microsecond)) = to_components(dt) else {
        return "<out of range>".to_owned();
    };
    let mut s = format!("{year:04}-{month:02}-{day:02}{sep}");
    spec.write_clock(
        &mut s,
        u32::from(hour),
        u32::from(minute),
        u32::from(second),
        microsecond,
    );
    if let Some(offset) = offset_seconds(dt) {
        s.push_str(&timezone::format_offset_hms(offset));
    }
    s
}

/// Argument shape for `datetime.isoformat(sep='T', timespec='auto')`.
///
/// `sep` stays a raw [`Value`] because CPython's `C` converter has wording no
/// `FromValue` impl produces — it reports the *length* of a rejected string
/// (`not a string of length 2`) — so [`isoformat_separator`] checks it in the
/// body. `bad_arg` still covers `timespec`.
#[derive(FromArgs)]
#[from_args(name = "isoformat", style = c_named, at_most_total, bad_arg)]
struct IsoformatArgs {
    #[from_args(default)]
    sep: Option<Value>,
    #[from_args(default)]
    timespec: Option<StrArg>,
}

/// Validates `isoformat`'s `sep` argument: any single character, `'T'` by default.
///
/// CPython takes it through the `C` format unit, which accepts one character
/// (not one byte — `dt.isoformat('日')` is fine) and rejects everything else
/// with the argument's length rather than its type.
fn isoformat_separator(sep: Option<&Value>, vm: &VM<'_>) -> RunResult<char> {
    let Some(sep) = sep else { return Ok('T') };
    // `try_to_str_heap`'s own message is the generic `expected string, not X`, so
    // only its success is used and the rejection is reworded here.
    match sep.try_to_str_heap(vm.heap, vm.interns).ok() {
        Some(s) if s.chars().count() == 1 => Ok(s.chars().next().expect("just counted one char")),
        Some(s) => Err(ExcType::type_error_bad_arg_pos(
            "isoformat",
            1,
            "a unicode character",
            format_args!("a string of length {}", s.chars().count()),
        )),
        None => Err(ExcType::type_error_bad_arg_pos(
            "isoformat",
            1,
            "a unicode character",
            sep.py_type_heap(vm.heap).cpython_arg_name(vm.heap, vm.interns),
        )),
    }
}

/// `datetime.timestamp()`: seconds since the Unix epoch. An aware value converts
/// through its own offset; a naive one is read as session-local wall time, as
/// CPython reads it in the host's zone, at its first occurrence in a DST fold.
fn compute_timestamp(dt: &DateTime, zone: &SandboxTimeZone) -> RunResult<f64> {
    let offset = if let Some(offset) = dt.offset_seconds {
        offset
    } else {
        probe_local_offset(dt.naive, zone)?;
        zone.offset_for_local(dt.naive).ok_or_else(date_out_of_range)?
    };
    // Seconds, not a datetime: the instant may fall outside the range a `datetime`
    // holds at either end of it, and CPython still returns the number.
    let nanos = epoch_duration(dt.naive).as_nanos();
    let seconds = i64::try_from(nanos.div_euclid(1_000_000_000)).expect("civil seconds fit i64") - i64::from(offset);
    let micros = u32::try_from(nanos.rem_euclid(1_000_000_000) / 1_000).expect("subsecond microseconds");
    Ok(seconds as f64 + f64::from(micros) / 1_000_000.0)
}

/// CPython solves a naive value's instant by rendering two candidates as civil
/// datetimes: the value shifted by the zone's offset, and the day before it.
/// Either can leave `datetime`'s range at the ends of it, where CPython's own
/// error escapes, so it is reproduced. Unlike [`probe_neighbouring_days`] this
/// depends on the offset: at the last representable day only a zone east of UTC
/// shifts past the end.
fn probe_local_offset(naive: CivilDateTime, zone: &SandboxTimeZone) -> RunResult<()> {
    let offset = zone.at(naive).offset_seconds;
    let shifted = wall_clock_at(naive, offset);
    if naive.date() == civil::date(1, 1, 1) {
        Err(date::year_out_of_range(0))
    } else {
        shifted.map(|_| ())
    }
}

impl HeapItem for DateTime {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(tzinfo_ref) = self.tzinfo_ref {
            stack.push(tzinfo_ref);
        }
    }
}

/// `HeapRead`-based dispatch for `DateTime`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident datetimes.
impl<'h> PyTrait<'h> for HeapObjectRead<'h, DateTime> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::DateTime.into()
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::DateTime(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        Ok(Some(if is_aware(a) != is_aware(b) {
            false
        } else if is_aware(a) {
            utc_micros(a) == utc_micros(b)
        } else {
            local_micros(a) == local_micros(b)
        }))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        if is_aware(a) != is_aware(b) {
            // Comparing offset-naive and offset-aware datetimes has no ordering
            // in CPython (it raises `TypeError`), so report it as incomparable.
            return Ok(CmpOrder::Incomparable);
        }
        // Both sides compare on an integer microsecond count — always ordered.
        if is_aware(a) {
            return Ok(CmpOrder::Ordered(utc_micros(a).cmp(&utc_micros(b))));
        }
        Ok(CmpOrder::Ordered(local_micros(a).cmp(&local_micros(b))))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let dt = self.get(vm.heap);
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(dt) else {
            f.write_str("datetime.datetime(<out of range>)")?;
            return Ok(());
        };

        write!(f, "datetime.datetime({year}, {month}, {day}, {hour}, {minute}")?;
        if second != 0 || microsecond != 0 {
            write!(f, ", {second}")?;
        }
        if microsecond != 0 {
            write!(f, ", {microsecond}")?;
        }
        if let Some(tzinfo) = timezone_info(dt) {
            if tzinfo.offset_seconds == 0 && tzinfo.name.is_none() {
                f.write_str(", tzinfo=datetime.timezone.utc")?;
            } else {
                let timedelta_repr = timezone::format_offset_timedelta_repr(tzinfo.offset_seconds);
                write!(f, ", tzinfo=datetime.timezone({timedelta_repr}")?;
                if let Some(name) = &tzinfo.name {
                    write!(f, ", {}", StringRepr(name))?;
                }
                f.write_char(')')?;
            }
        }
        f.write_char(')')?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let dt = self.get(vm.heap);
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(dt) else {
            return Ok(allocate_string("<out of range>", vm.heap));
        };
        let mut s = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
        if microsecond != 0 {
            write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
        }
        if let Some(offset) = offset_seconds(dt) {
            s.push_str(&timezone::format_offset_hms(offset));
        }
        Ok(allocate_string(s, vm.heap))
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(HeapReadOutput::TimeDelta(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let value = self.get(vm.heap).clone();
        let other = *other.get(vm.heap);
        py_add(&value, &other, vm.heap).map(Some)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::DateTime(other)) => {
                let value = self.get(vm.heap).clone();
                let other = other.get(vm.heap).clone();
                Ok(py_sub_datetime(&value, &other, vm.heap))
            }
            Some(HeapReadOutput::TimeDelta(other)) => {
                let value = self.get(vm.heap).clone();
                let other = *other.get(vm.heap);
                py_sub_timedelta(&value, &other, vm.heap).map(Some)
            }
            _ => Ok(None),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let dt = self.get(vm.heap).clone();
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Isoformat) => {
                let IsoformatArgs { sep, timespec } = IsoformatArgs::from_args(args, vm)?;
                defer_drop!(sep, vm);
                defer_drop!(timespec, vm);
                let separator = isoformat_separator(sep.as_ref(), vm)?;
                let spec = match timespec {
                    Some(timespec) => TimeSpec::parse(timespec.as_str(vm))?,
                    None => TimeSpec::Auto,
                };
                let s = format_isoformat(&dt, separator, spec);
                Ok(CallResult::Value(allocate_string_no_interning(s, vm.heap)))
            }
            Some(StaticStrings::Strftime) => {
                let StrftimeArgs { format } = StrftimeArgs::from_args(args, vm)?;
                defer_drop!(format, vm);
                let formatted = format_datetime_strftime(&dt, format.as_str(vm), &vm.heap.tracker)?;
                Ok(CallResult::Value(allocate_string(formatted, vm.heap)))
            }
            Some(StaticStrings::Replace) => Ok(CallResult::Value(self.replace(vm, args)?)),
            Some(StaticStrings::Weekday) => {
                args.check_zero_args("datetime.weekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    dt.naive.date().weekday().to_monday_zero_offset(),
                ))))
            }
            Some(StaticStrings::Isoweekday) => {
                args.check_zero_args("datetime.isoweekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    dt.naive.date().weekday().to_monday_one_offset(),
                ))))
            }
            Some(StaticStrings::Date) => {
                args.check_zero_args("datetime.date", vm.heap)?;
                let d = date::from_ymd(
                    i32::from(dt.naive.date().year()),
                    i32::from(dt.naive.month()),
                    i32::from(dt.naive.day()),
                )?;
                Ok(CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::Date(d)))))
            }
            Some(method @ (StaticStrings::Time | StaticStrings::Timetz)) => {
                // `time()` drops the timezone, `timetz()` keeps it — the only
                // difference between the two in CPython.
                let keep_tz = method == StaticStrings::Timetz;
                args.check_zero_args(if keep_tz { "datetime.timetz" } else { "datetime.time" }, vm.heap)?;
                // Owned, so it must be released whether or not the time attaches it.
                let tzinfo = if keep_tz { self.tzinfo_value(vm) } else { Value::None };
                defer_drop!(tzinfo, vm);
                let naive = dt.naive.time();
                let micros = dt.naive.subsec_nanosecond() / 1_000;
                Ok(CallResult::Value(time::allocate(
                    vm,
                    i32::from(naive.hour()),
                    i32::from(naive.minute()),
                    i32::from(naive.second()),
                    micros,
                    // `DateTime` does not store `fold`, so the time is always unfolded.
                    0,
                    tzinfo,
                )?))
            }
            Some(StaticStrings::Timestamp) => {
                args.check_zero_args("datetime.timestamp", vm.heap)?;
                let ts = compute_timestamp(&dt, &vm.env.os_policy.timezone)?;
                Ok(CallResult::Value(Value::Float(ts)))
            }
            Some(StaticStrings::Astimezone) => astimezone(&dt, vm, args),
            // A classmethod, so CPython also reaches it through an instance.
            Some(StaticStrings::Fromtimestamp) => class_fromtimestamp(vm, args).map(CallResult::Value),
            Some(StaticStrings::Utcoffset) => {
                args.check_zero_args("datetime.utcoffset", vm.heap)?;
                Ok(CallResult::Value(timezone::utcoffset_value(dt.offset_seconds, vm.heap)))
            }
            Some(StaticStrings::Tzname) => {
                args.check_zero_args("datetime.tzname", vm.heap)?;
                let Some(offset_seconds) = dt.offset_seconds else {
                    return Ok(CallResult::Value(Value::None));
                };
                let name = timezone::tzname_string(offset_seconds, dt.timezone_name.as_deref());
                Ok(CallResult::Value(allocate_string(name, vm.heap)))
            }
            Some(StaticStrings::Dst) => {
                args.check_zero_args("datetime.dst", vm.heap)?;
                // Only fixed-offset zones exist, and none of them observes DST.
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(ExcType::attribute_error_method(Type::DateTime, attr, args, vm)),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        // Read only the field being asked for: cloning the whole `DateTime` would
        // copy the timezone name's `String` on every `dt.year`.
        let int_attr = |value: i64| Ok(Some(CallResult::Value(Value::Int(value))));
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Year) => Ok(Some(CallResult::Value(Value::Int(i64::from(
                self.get(vm.heap).naive.date().year(),
            ))))),
            Some(StaticStrings::Month) => int_attr(i64::from(self.get(vm.heap).naive.month())),
            Some(StaticStrings::Day) => int_attr(i64::from(self.get(vm.heap).naive.day())),
            Some(StaticStrings::Hour) => int_attr(i64::from(self.get(vm.heap).naive.hour())),
            Some(StaticStrings::Minute) => int_attr(i64::from(self.get(vm.heap).naive.minute())),
            Some(StaticStrings::Second) => int_attr(i64::from(self.get(vm.heap).naive.second())),
            Some(StaticStrings::Microsecond) => {
                int_attr(i64::from(self.get(vm.heap).naive.subsec_nanosecond() / 1_000))
            }
            Some(StaticStrings::Tzinfo) => Ok(Some(CallResult::Value(self.tzinfo_value(vm)))),
            _ => Ok(None),
        }
    }
}

/// Datetime behaviour that needs both the object and the VM, so it cannot be
/// expressed as a plain function over [`DateTime`].
impl<'h> HeapObjectRead<'h, DateTime> {
    /// The `tzinfo` of a datetime as an owned value, or `Value::None` if it is naive.
    ///
    /// Prefers the retained object so `dt.tzinfo is tz` holds and repeated access
    /// returns one object. A datetime that has an offset but no retained reference —
    /// `tzinfo_ref` is absent from dumps written before it existed — rebuilds an equal
    /// `timezone` instead of reporting itself naive.
    fn tzinfo_value(&self, vm: &mut VM<'h>) -> Value {
        // Read both branches' inputs before the borrow ends: the rebuild path
        // needs `&mut Heap` for `get_timezone_utc` and `allocate`.
        let dt = self.get(vm.heap);
        let retained = dt.tzinfo_ref;
        let rebuilt = retained.is_none().then(|| timezone_info(dt)).flatten();

        if let Some(tzinfo_ref) = retained {
            vm.heap.inc_ref(tzinfo_ref);
            Value::Ref(tzinfo_ref)
        } else {
            match rebuilt {
                Some(tz) if tz.offset_seconds == 0 && tz.name.is_none() => vm.heap.get_timezone_utc(),
                Some(tz) => Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))),
                None => Value::None,
            }
        }
    }

    /// `datetime.replace(...)` — a copy with the named components substituted.
    ///
    /// Every field the caller omits is carried over, the `tzinfo` object identity
    /// included. Components are validated by [`from_components`], exactly as the
    /// constructor validates them.
    fn replace(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        // `CivilDateTime` is `Copy` and the zone is taken by value, so the heap
        // borrow ends before `from_args` needs `&mut VM`. The carried-over
        // `tzinfo_ref` is borrowed, kept valid by this read handle until
        // `from_components` takes its own reference.
        let dt = self.get(vm.heap);
        let naive = dt.naive;
        let (current_tz, current_tz_ref) = (timezone_info(dt), dt.tzinfo_ref);

        let DatetimeReplaceArgs {
            year,
            month,
            day,
            hour,
            minute,
            second,
            microsecond,
            tzinfo,
        } = DatetimeReplaceArgs::from_args(args, vm)?;

        // `tzinfo` is `Some(v)` only when the caller actually passed the kwarg;
        // absent → preserve existing tzinfo. The guard has to span `from_components`,
        // which is where the new datetime takes its own reference: an argument built
        // in the call, `replace(tzinfo=timezone(...))`, has no other holder until then.
        defer_drop_mut!(tzinfo, vm);
        let (new_tz, new_tz_ref) = match &*tzinfo {
            None => (current_tz, current_tz_ref),
            Some(tzinfo_value) => tzinfo_from_value(tzinfo_value, vm.heap, vm.interns)?,
        };

        let new_dt = from_components(
            year.unwrap_or_else(|| i32::from(naive.date().year())),
            month.unwrap_or_else(|| i32::from(naive.month())),
            day.unwrap_or_else(|| i32::from(naive.day())),
            hour.unwrap_or_else(|| i32::from(naive.hour())),
            minute.unwrap_or_else(|| i32::from(naive.minute())),
            second.unwrap_or_else(|| i32::from(naive.second())),
            microsecond.unwrap_or_else(|| naive.subsec_nanosecond() / 1_000),
            new_tz,
            new_tz_ref,
            vm.heap,
        )?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(new_dt))))
    }
}

/// Keyword arguments for `datetime.replace()`. All keyword-only; absent fields
/// inherit the existing datetime component via `unwrap_or_else` at the call
/// site. `tzinfo` uses `Option<Value>` to distinguish "kwarg absent" (preserve
/// existing) from "tzinfo=None" (clear).
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct DatetimeReplaceArgs {
    #[from_args(kw_only, default)]
    year: Option<i32>,
    #[from_args(kw_only, default)]
    month: Option<i32>,
    #[from_args(kw_only, default)]
    day: Option<i32>,
    #[from_args(kw_only, default)]
    hour: Option<i32>,
    #[from_args(kw_only, default)]
    minute: Option<i32>,
    #[from_args(kw_only, default)]
    second: Option<i32>,
    #[from_args(kw_only, default)]
    microsecond: Option<i32>,
    #[from_args(kw_only, default)]
    tzinfo: Option<Value>,
}
