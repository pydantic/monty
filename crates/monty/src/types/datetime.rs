//! Python `datetime.datetime` implementation.
//!
//! Monty stores datetimes with chrono primitives and layers CPython-compatible
//! constructor rules, aware/naive comparison semantics, and arithmetic on top.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use chrono::{
    Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta, Timelike,
    format::{Parsed, StrftimeItems, parse as chrono_parse, parse_and_remainder},
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
    types::{
        CmpOrder, LazyHeapSet, PyTrait, TimeDelta, TimeZone, Type,
        date::{self, StrftimeArgs},
        str::{StringRepr, allocate_string, allocate_string_no_interning},
        time::{self, Time, TimeSpec},
        timedelta, timezone,
    },
    value::{EitherStr, Value},
};

/// Number of microseconds in a single second.
const DATE_OUT_OF_RANGE: &str = "date value out of range";

/// `datetime.datetime` storage backed by `chrono::NaiveDateTime` plus optional fixed offset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DateTime {
    naive: NaiveDateTime,
    offset_seconds: Option<i32>,
    timezone_name: Option<String>,
    /// Stable timezone object identity for aware datetimes.
    ///
    /// CPython preserves the original `tzinfo` object identity (`dt.tzinfo is tz`)
    /// and repeated `dt.tzinfo` access returns the same object. We store a retained
    /// heap reference so attribute lookup can return a stable object instead of
    /// allocating a new timezone each time.
    #[serde(default)]
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
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("second must be in 0..59, not {second}")).into(),
        );
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
    let time = NaiveTime::from_hms_micro_opt(
        u32::try_from(hour).expect("hour validated to 0..=23"),
        u32::try_from(minute).expect("minute validated to 0..=59"),
        u32::try_from(second).expect("second validated to 0..=59"),
        u32::try_from(microsecond).expect("microsecond validated to 0..=999_999"),
    )
    .expect("validated time components must produce a NaiveTime");

    let (offset_seconds, timezone_name) = match tzinfo {
        Some(tz) => (Some(tz.offset_seconds), tz.name),
        None => (None, None),
    };
    if let Some(offset_seconds) = offset_seconds
        && FixedOffset::east_opt(offset_seconds).is_none()
    {
        return Err(SimpleException::new_msg(ExcType::ValueError, "timezone offset out of range").into());
    }

    let mut datetime = DateTime {
        naive: date_value.0.and_time(time),
        offset_seconds,
        timezone_name,
        tzinfo_ref: None,
    };

    if let Some(offset_seconds) = offset_seconds {
        let Some(utc) = to_utc_naive(&datetime) else {
            return Err(SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
        };
        if from_utc_naive_with_offset(utc, offset_seconds).is_none() {
            return Err(SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
        }
    }

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
    let year = datetime.naive.date().year();
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
        datetime.naive.and_utc().timestamp_subsec_micros(),
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

/// `datetime.astimezone(tz=None)`: the same instant in `tz`, or in the sandbox
/// zone at that instant when `tz` is `None`. A naive value is read as sandbox-local
/// wall time, as CPython reads it in the host's zone.
fn astimezone(dt: &DateTime, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let AstimezoneArgs { tz } = AstimezoneArgs::from_args(args, vm)?;
    defer_drop!(tz, vm);
    let (tz, tz_ref) = tzinfo_from_value(tz, vm.heap, vm.interns)?;
    let zone = &vm.env.auto_os_calls.timezone;
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
fn probe_neighbouring_days(date: NaiveDate) -> RunResult<()> {
    // chrono's own range is far wider than Python's, so the years are what decide.
    let in_range = |day: Option<NaiveDate>| day.is_some_and(|day| year_in_python_range(day.year()));
    if !in_range(date.pred_opt()) {
        Err(date::year_out_of_range(0))
    } else if !in_range(date.succ_opt()) {
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
pub(crate) fn sandbox_instant(vm: &VM<'_>) -> RunResult<Option<NaiveDateTime>> {
    match vm.env.auto_os_calls.datetime {
        DateTimeSource::CallHost => Ok(None),
        source => source.read().map(Some).ok_or_else(date_out_of_range),
    }
}

/// Converts UTC to the session zone's wall clock for naive `now()` and `today()`;
/// out-of-range years raise `OverflowError`.
pub(crate) fn sandbox_local_wall_clock(vm: &VM<'_>, utc: NaiveDateTime) -> RunResult<NaiveDateTime> {
    let offset = vm.env.auto_os_calls.timezone.at(utc).offset_seconds;
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
/// Parses a date/time string using the given format. Delegates to chrono's
/// `NaiveDateTime::parse_from_str`, expanding Python `%f` directives into the
/// chrono widths needed to accept 1 through 6 fractional digits.
pub(crate) fn class_strptime(heap: &mut Heap, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (date_string_val, format_val) = args.get_two_args("datetime.strptime", heap)?;

    let date_string = date::extract_str_arg(&date_string_val, "strptime", heap, interns);
    let fmt = date::extract_str_arg(&format_val, "strptime", heap, interns);
    date_string_val.drop_with(heap);
    format_val.drop_with(heap);
    let date_string = date_string?;
    let fmt = fmt?;

    reject_bad_strptime_directive(&fmt)?;

    // Python's `%f` accepts 1..=6 digits and right-pads with zeros, while chrono
    // requires an explicit width. Try all valid `%f` widths before reporting a
    // mismatch so `datetime.strptime(..., '%f')` matches CPython.
    let Some(parsed) = parse_strptime(&date_string, &fmt) else {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("time data '{date_string}' does not match format '{fmt}'"),
        )
        .into());
    };
    let (naive, offset_seconds) = parsed?;

    if !year_in_python_range(naive.date().year()) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "year is out of range").into());
    }
    if let Some(offset_seconds) = offset_seconds {
        timezone::check_offset_seconds(offset_seconds)?;
    }

    let dt = DateTime {
        naive,
        offset_seconds,
        timezone_name: None,
        tzinfo_ref: None,
    };
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))))
}

/// CPython's `strptime` has no `%:z`, so the `:` reads as a directive of its own.
/// Monty's `strftime` does accept `%:z`, which makes silently parsing it here the
/// wrong kind of asymmetry.
fn reject_bad_strptime_directive(fmt: &str) -> RunResult<()> {
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch == '%'
            && let Some(next) = chars.next()
            && next == ':'
        {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("'{next}' is a bad directive in format '{fmt}'"),
            )
            .into());
        }
    }
    Ok(())
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
    let value = args.get_one_arg("datetime.fromisoformat", heap)?;
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
        naive.year(),
        i32::try_from(naive.month()).expect("month in 1..12"),
        i32::try_from(naive.day()).expect("day in 1..31"),
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

/// Parses a `time.strptime()` input, whose unset fields default to 1900-01-01.
///
/// `datetime.strptime` needs a date; `time.strptime('12:30', '%H:%M')` does not,
/// so a format that matched nothing is retried with the default date prefixed to
/// both sides — the anchor CPython fills those fields from.
pub(crate) fn parse_time_strptime(date_string: &str, fmt: &str) -> RunResult<NaiveDateTime> {
    reject_bad_strptime_directive(fmt)?;
    let anchored = || {
        let dated = format!("{DEFAULT_STRPTIME_DATE} {date_string}");
        parse_strptime(&dated, &format!("%Y-%m-%d {fmt}"))
    };
    match parse_strptime(date_string, fmt).or_else(anchored) {
        Some(parsed) => parsed.map(|(naive, _)| naive),
        None => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!(
                "time data {} does not match format {}",
                StringRepr(date_string),
                StringRepr(fmt)
            ),
        )
        .into()),
    }
}

/// The date `time.strptime` leaves in fields its format did not set.
const DEFAULT_STRPTIME_DATE: &str = "1900-01-01";

/// Parses a `strptime` input into naive components plus the offset a `%z`
/// directive asked for, `None` when nothing in the input matches the format.
///
/// The outer `Option` is the match, the inner `Result` what the matched `%z`
/// token turned out to be: CPython only rejects a mismatched pair of separators
/// once the rest of the format has matched, so that error cannot be reported
/// until the match is settled.
fn parse_strptime(date_string: &str, fmt: &str) -> Option<RunResult<(NaiveDateTime, Option<i32>)>> {
    let Some((before, after)) = split_zone_directive(fmt) else {
        return parse_strptime_naive(date_string, fmt).map(|naive| Ok((naive, None)));
    };
    // Chrono cannot express CPython's `%z` — an optional colon, optional seconds,
    // or a bare `Z` — so the directives either side of it are parsed in turn and the
    // token is read from between them, sharing one `Parsed` as chrono's own
    // `parse_from_str` does. Chrono reports where the leading directives stopped,
    // which is the only place the token can begin: hunting for it through the input
    // instead would cost a parse of the whole string per candidate position.
    for before_fmt in chrono_strptime_formats(&before) {
        let mut parsed = Parsed::new();
        let Ok(rest) = parse_and_remainder(&mut parsed, date_string, StrftimeItems::new(&before_fmt)) else {
            continue;
        };
        let Some(token) = strptime_offset_at(rest) else {
            continue;
        };
        for after_fmt in chrono_strptime_formats(&after) {
            let mut parsed = parsed.clone();
            if chrono_parse(&mut parsed, &rest[token.len..], StrftimeItems::new(&after_fmt)).is_err() {
                continue;
            }
            let Some(naive) = naive_from_parsed(&parsed) else {
                continue;
            };
            return Some(if token.colons_agree {
                Ok((naive, Some(token.offset_seconds)))
            } else {
                let matched = &rest[..token.len];
                Err(SimpleException::new_msg(ExcType::ValueError, format!("Inconsistent use of : in {matched}")).into())
            });
        }
    }
    None
}

/// The datetime chrono accumulated, defaulting a date-only format to midnight as
/// [`parse_strptime_naive`] does.
fn naive_from_parsed(parsed: &Parsed) -> Option<NaiveDateTime> {
    parsed
        .to_naive_datetime_with_offset(0)
        .ok()
        .or_else(|| parsed.to_naive_date().ok()?.and_hms_opt(0, 0, 0))
}

/// The format either side of its `%z` directive, or `None` when it has none.
/// `%%z` is a literal `z` and is not the directive.
fn split_zone_directive(fmt: &str) -> Option<(String, String)> {
    let mut rest = fmt;
    let mut before = String::with_capacity(fmt.len());
    while let Some(at) = rest.find('%') {
        let (literal, directive) = rest.split_at(at);
        before.push_str(literal);
        let mut chars = directive.chars();
        chars.next();
        match chars.next() {
            Some('z') => return Some((before, chars.as_str().to_owned())),
            Some(other) => {
                before.push('%');
                before.push(other);
                rest = chars.as_str();
            }
            None => return None,
        }
    }
    None
}

/// A `%z` token matched at some position in a `strptime` input.
struct ZoneToken {
    offset_seconds: i32,
    /// The token's length in bytes, so the caller can lift it out of the input.
    len: usize,
    /// Whether the minute and second separators agree. `+01:02:03` and
    /// `+010203` do, `+01:0203` and `+0102:03` do not — CPython matches the
    /// mixed pair and then rejects it, rather than reading a shorter token.
    colons_agree: bool,
}

/// CPython's `%z` token at the start of `s`: a bare `Z`, or a sign, two hour
/// digits and a colon-optional minute pair, optionally followed by a second
/// pair. A fractional part is not accepted; see `limitations/datetime.md`.
fn strptime_offset_at(s: &str) -> Option<ZoneToken> {
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'Z') {
        return Some(ZoneToken {
            offset_seconds: 0,
            len: 1,
            colons_agree: true,
        });
    }
    let sign = match bytes.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hours, at) = two_digits(bytes, 1)?;
    let minute_colon = bytes.get(at) == Some(&b':');
    let (minutes, at) = colon_pair(bytes, at)?;
    // CPython's pattern caps minutes and seconds at 59 and leaves the hour to the
    // `timezone` range check, so an out-of-range minute is simply not a match.
    if minutes > 59 {
        return None;
    }
    let second_colon = bytes.get(at) == Some(&b':');
    let (seconds, at, colons_agree) = match colon_pair(bytes, at) {
        Some((seconds, next)) if seconds <= 59 => (seconds, next, second_colon == minute_colon),
        _ => (0, at, true),
    };
    let total = i32::try_from(hours * 3600 + minutes * 60 + seconds).ok()?;
    Some(ZoneToken {
        offset_seconds: sign * total,
        len: at,
        colons_agree,
    })
}

/// Two decimal digits at `at`, with the index just past them.
fn two_digits(bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    pair.iter()
        .all(u8::is_ascii_digit)
        .then(|| (u32::from(pair[0] - b'0') * 10 + u32::from(pair[1] - b'0'), at + 2))
}

/// An optional `:` then two digits, the separator `%z` allows between its parts.
fn colon_pair(bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    let at = if bytes.get(at) == Some(&b':') { at + 1 } else { at };
    two_digits(bytes, at)
}

/// Parses a `datetime.strptime()` input using chrono format strings expanded for
/// Python's variable-width `%f` semantics.
fn parse_strptime_naive(date_string: &str, fmt: &str) -> Option<NaiveDateTime> {
    for chrono_fmt in chrono_strptime_formats(fmt) {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(date_string, &chrono_fmt) {
            return Some(ndt);
        }
        if let Ok(naive_date) = chrono::NaiveDate::parse_from_str(date_string, &chrono_fmt) {
            return naive_date.and_hms_opt(0, 0, 0);
        }
    }
    None
}

/// Rewrites Python `%f` directives into chrono-compatible formats.
///
/// Python `%f` accepts 1 through 6 microsecond digits. Chrono only has two
/// useful parsing forms for Monty here:
/// - `%.f` for variable-width fractions that include the leading dot
/// - `%6f` for fixed-width fractions without the leading dot
fn chrono_strptime_formats(fmt: &str) -> Vec<String> {
    let mut chrono_fmt = String::with_capacity(fmt.len());
    let mut chars = fmt.chars();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            chrono_fmt.push(ch);
            continue;
        }

        let Some(next) = chars.next() else {
            chrono_fmt.push('%');
            break;
        };

        if next == '%' {
            chrono_fmt.push('%');
            chrono_fmt.push('%');
            continue;
        }

        if next == 'f' {
            if chrono_fmt.ends_with('.') {
                chrono_fmt.pop();
                chrono_fmt.push('%');
                chrono_fmt.push('.');
                chrono_fmt.push('f');
            } else {
                chrono_fmt.push('%');
                chrono_fmt.push('6');
                chrono_fmt.push('f');
            }
            continue;
        }

        chrono_fmt.push('%');
        chrono_fmt.push(next);
    }

    vec![chrono_fmt]
}

/// `datetime + timedelta`
pub(crate) fn py_add(datetime: &DateTime, delta: &TimeDelta, heap: &mut Heap) -> Option<Value> {
    let chrono_delta = timedelta::chrono_delta(delta);

    let next = if let Some(offset) = datetime.offset_seconds {
        let utc = to_utc_naive(datetime)?;
        let next_utc = utc.checked_add_signed(chrono_delta)?;
        from_utc_naive_with_timezone_parts(next_utc, offset, datetime.timezone_name.clone())
    } else {
        let next_local = datetime.naive.checked_add_signed(chrono_delta)?;
        from_local_naive(next_local)
    };

    let mut next = next?;
    attach_or_allocate_tzinfo_ref(&mut next, datetime.tzinfo_ref, heap);
    Some(Value::Ref(heap.allocate(HeapData::DateTime(next))))
}

/// `datetime - timedelta`
pub(crate) fn py_sub_timedelta(datetime: &DateTime, delta: &TimeDelta, heap: &mut Heap) -> Option<Value> {
    let chrono_delta = timedelta::chrono_delta(delta);

    let next = if let Some(offset) = datetime.offset_seconds {
        let utc = to_utc_naive(datetime)?;
        let next_utc = utc.checked_sub_signed(chrono_delta)?;
        from_utc_naive_with_timezone_parts(next_utc, offset, datetime.timezone_name.clone())
    } else {
        let next_local = datetime.naive.checked_sub_signed(chrono_delta)?;
        from_local_naive(next_local)
    };

    let mut next = next?;
    attach_or_allocate_tzinfo_ref(&mut next, datetime.tzinfo_ref, heap);
    Some(Value::Ref(heap.allocate(HeapData::DateTime(next))))
}

/// `datetime - datetime` returns a timedelta with the difference.
///
/// Both datetimes must be either aware or naive; mixing returns `None`.
pub(crate) fn py_sub_datetime(a: &DateTime, b: &DateTime, heap: &mut Heap) -> Option<Value> {
    if is_aware(a) != is_aware(b) {
        return None;
    }

    let diff = if is_aware(a) {
        let lhs_utc = to_utc_naive(a)?;
        let rhs_utc = to_utc_naive(b)?;
        lhs_utc.signed_duration_since(rhs_utc)
    } else {
        a.naive.signed_duration_since(b.naive)
    };

    let delta = timedelta::from_chrono(diff).ok()?;
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
        _ => Err(ExcType::type_error_tzinfo(&value.py_type_shallow().name(heap, interns))),
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
    Some(datetime.naive.and_utc().timestamp_micros())
}

/// Returns UTC microseconds since Unix epoch for aware datetimes, otherwise local micros.
#[must_use]
pub(crate) fn utc_micros(datetime: &DateTime) -> Option<i64> {
    match datetime.offset_seconds {
        Some(_) => {
            let utc = to_utc_naive(datetime)?;
            Some(utc.and_utc().timestamp_micros())
        }
        None => local_micros(datetime),
    }
}

fn from_local_naive(naive: NaiveDateTime) -> Option<DateTime> {
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

fn from_utc_naive_with_offset(utc_naive: NaiveDateTime, offset_seconds: i32) -> Option<DateTime> {
    from_utc_naive_with_timezone_parts(utc_naive, offset_seconds, None)
}

fn from_utc_naive_with_timezone_parts(
    utc_naive: NaiveDateTime,
    offset_seconds: i32,
    timezone_name: Option<String>,
) -> Option<DateTime> {
    FixedOffset::east_opt(offset_seconds)?;
    let offset_delta = ChronoTimeDelta::try_seconds(i64::from(offset_seconds))?;
    let local = utc_naive.checked_add_signed(offset_delta)?;
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

fn to_utc_naive(datetime: &DateTime) -> Option<NaiveDateTime> {
    let offset_seconds = datetime.offset_seconds?;
    let offset_delta = ChronoTimeDelta::try_seconds(i64::from(offset_seconds))?;
    datetime.naive.checked_sub_signed(offset_delta)
}

#[must_use]
fn year_in_python_range(year: i32) -> bool {
    (1..=9999).contains(&year)
}

/// Formats a [`DateTime`] with a `strftime` directive string, shared by the
/// `datetime.strftime()` method and f-string formatting (`f"{dt:%Y-%m-%d}"`).
///
/// Uses the naive (wall-clock) components, mirroring `chrono`'s formatting of
/// `NaiveDateTime`, with the **lenient** parser so an unrecognised directive is
/// passed through verbatim to match glibc/Linux CPython (see
/// [`date::format_date_strftime`]). The zone directives are substituted from
/// the offset and name first (`%z` and `%Z` are empty for a naive value).
pub(crate) fn format_datetime_strftime(dt: &DateTime, format: &str, tracker: &ResourceTracker) -> RunResult<String> {
    let format = date::rewrite_zone_directives(format, dt.offset_seconds, dt.timezone_name.as_deref(), tracker)?;
    let format = date::rewrite_microsecond_directive(&format);
    date::render_strftime(dt.naive.format_with_items(StrftimeItems::new_lenient(&format)))
        .ok_or_else(date::invalid_strftime_error)
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
    // `to_str_heap`'s own message is the generic `expected string, not X`, so
    // only its success is used and the rejection is reworded here.
    match sep.to_str_heap(vm.heap, vm.interns).ok() {
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
    let local = dt.naive.and_utc();
    let seconds = local.timestamp() - i64::from(offset);
    Ok(seconds as f64 + f64::from(local.timestamp_subsec_micros()) / 1_000_000.0)
}

/// CPython solves a naive value's instant by rendering two candidates as civil
/// datetimes: the value shifted by the zone's offset, and the day before it.
/// Either can leave `datetime`'s range at the ends of it, where CPython's own
/// error escapes, so it is reproduced. Unlike [`probe_neighbouring_days`] this
/// depends on the offset: at the last representable day only a zone east of UTC
/// shifts past the end.
fn probe_local_offset(naive: NaiveDateTime, zone: &SandboxTimeZone) -> RunResult<()> {
    let offset = ChronoTimeDelta::seconds(i64::from(zone.at(naive).offset_seconds));
    // chrono's range is far wider than Python's, so a shift of at most a day lands.
    let shifted = naive.checked_add_signed(offset).unwrap_or(naive).year();
    if naive
        .date()
        .pred_opt()
        .is_none_or(|day| !year_in_python_range(day.year()))
    {
        Err(date::year_out_of_range(0))
    } else if year_in_python_range(shifted) {
        Ok(())
    } else {
        Err(date::year_out_of_range(shifted))
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
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DateTime
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
        Ok(py_add(&value, &other, vm.heap))
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
                Ok(py_sub_timedelta(&value, &other, vm.heap))
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
                    dt.naive.date().weekday().num_days_from_monday(),
                ))))
            }
            Some(StaticStrings::Isoweekday) => {
                args.check_zero_args("datetime.isoweekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    dt.naive.date().weekday().number_from_monday(),
                ))))
            }
            Some(StaticStrings::Date) => {
                args.check_zero_args("datetime.date", vm.heap)?;
                let d = date::from_ymd(
                    dt.naive.date().year(),
                    i32::try_from(dt.naive.date().month()).expect("month in 1..12"),
                    i32::try_from(dt.naive.date().day()).expect("day in 1..31"),
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
                let micros = i32::try_from(dt.naive.and_utc().timestamp_subsec_micros())
                    .expect("microsecond is always in 0..=999_999");
                Ok(CallResult::Value(time::allocate(
                    vm,
                    i32::try_from(naive.hour()).expect("hour is always in 0..=23"),
                    i32::try_from(naive.minute()).expect("minute is always in 0..=59"),
                    i32::try_from(naive.second()).expect("second is always in 0..=59"),
                    micros,
                    // `DateTime` does not store `fold`, so the time is always unfolded.
                    0,
                    tzinfo,
                )?))
            }
            Some(StaticStrings::Timestamp) => {
                args.check_zero_args("datetime.timestamp", vm.heap)?;
                let ts = compute_timestamp(&dt, &vm.env.auto_os_calls.timezone)?;
                Ok(CallResult::Value(Value::Float(ts)))
            }
            Some(StaticStrings::Astimezone) => astimezone(&dt, vm, args),
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
        let int_attr = |value: u32| Ok(Some(CallResult::Value(Value::Int(i64::from(value)))));
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Year) => Ok(Some(CallResult::Value(Value::Int(i64::from(
                self.get(vm.heap).naive.date().year(),
            ))))),
            Some(StaticStrings::Month) => int_attr(self.get(vm.heap).naive.date().month()),
            Some(StaticStrings::Day) => int_attr(self.get(vm.heap).naive.date().day()),
            Some(StaticStrings::Hour) => int_attr(self.get(vm.heap).naive.time().hour()),
            Some(StaticStrings::Minute) => int_attr(self.get(vm.heap).naive.time().minute()),
            Some(StaticStrings::Second) => int_attr(self.get(vm.heap).naive.time().second()),
            Some(StaticStrings::Microsecond) => int_attr(self.get(vm.heap).naive.and_utc().timestamp_subsec_micros()),
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
        // `NaiveDateTime` is `Copy` and the zone is taken by value, so the heap
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
            year.unwrap_or_else(|| naive.date().year()),
            month.unwrap_or_else(|| i32::try_from(naive.date().month()).expect("month in 1..12")),
            day.unwrap_or_else(|| i32::try_from(naive.date().day()).expect("day in 1..31")),
            hour.unwrap_or_else(|| i32::try_from(naive.time().hour()).expect("hour in 0..23")),
            minute.unwrap_or_else(|| i32::try_from(naive.time().minute()).expect("minute in 0..59")),
            second.unwrap_or_else(|| i32::try_from(naive.time().second()).expect("second in 0..59")),
            microsecond.unwrap_or_else(|| {
                i32::try_from(naive.and_utc().timestamp_subsec_micros()).expect("micros in 0..999999")
            }),
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
