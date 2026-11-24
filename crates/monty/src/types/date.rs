//! Python `datetime.date` implementation.
//!
//! Monty stores dates with `jiff::civil::Date` and keeps CPython-compatible
//! constructor validation and arithmetic behavior.

use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use jiff::{
    SignedDuration,
    civil::{self, Date as CivilDate},
    fmt::{
        StdFmtWrite,
        strtime::{BrokenDownTime, Config, PosixCustom},
    },
};
use monty_types::{OsFunctionCall, ResourceTracker};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    string_builder::StringBuilder,
    types::{
        CmpOrder, LazyHeapSet, PyTrait, TimeDelta, Type, TypeRef, datetime,
        str::{allocate_string, allocate_string_no_interning},
        timedelta,
        timedelta::DAY_MICROSECONDS,
        timezone::{format_offset_compact, format_offset_hms, tzname_string},
    },
    value::{EitherStr, Value},
};

const MICROSECONDS_PER_DAY: i128 = 86_400_000_000;

/// `datetime.date` storage backed by `jiff::civil::Date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(crate) struct Date(pub(crate) CivilDate);

impl Hash for Date {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Dumps persist the former packed date hash: year, ordinal, common-year bit,
        // and the preceding December 31's nonzero Monday-based weekday.
        let year = i32::from(self.0.year());
        let january = civil::date(self.0.year(), 1, 1);
        let weekday = i32::from(january.weekday().to_monday_zero_offset());
        let previous = (weekday + 5) % 7 + 1;
        let common = i32::from(!self.0.in_leap_year()) << 3;
        ((year << 13) | (i32::from(self.0.day_of_year()) << 4) | common | previous).hash(state);
    }
}

/// Creates a date from validated civil components.
///
/// Error messages match CPython 3.14 format exactly.
pub(crate) fn from_ymd(year: i32, month: i32, day: i32) -> RunResult<Date> {
    if !(1..=9999).contains(&year) {
        return Err(year_out_of_range(year));
    }
    if !(1..=12).contains(&month) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("month must be in 1..12, not {month}")).into(),
        );
    }
    let date = i8::try_from(day).ok().and_then(|day| {
        CivilDate::new(
            i16::try_from(year).expect("validated year"),
            i8::try_from(month).expect("validated month"),
            day,
        )
        .ok()
    });
    let Some(date) = date else {
        return Err(day_out_of_range_error(day, month, year));
    };
    Ok(Date(date))
}

/// `date`'s year-range error, raised both by construction and by the probe
/// [`datetime.astimezone`](super::datetime) makes a day either side of a value.
pub(crate) fn year_out_of_range(year: impl Into<i64>) -> RunError {
    let year = year.into();
    SimpleException::new_msg(ExcType::ValueError, format!("year must be in 1..9999, not {year}")).into()
}

/// The proleptic Gregorian year of the day `days` after the Unix epoch (1970-01-01),
/// for any `i64` (Howard Hinnant's `civil_from_days`).
pub(crate) fn civil_year_from_days(days: i64) -> i64 {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Years run March to February here, so January and February belong to the next one.
    let march_based_month = (5 * day_of_year + 2) / 153;
    year_of_era + era * 400 + i64::from(march_based_month >= 10)
}

/// Produces a CPython-compatible error for an invalid day value.
///
/// Format: `"day {day} must be in range 1..{max_day} for month {month} in year {year}"`
fn day_out_of_range_error(day: i32, month: i32, year: i32) -> RunError {
    let max_day = max_day_for_month(year, month);
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("day {day} must be in range 1..{max_day} for month {month} in year {year}"),
    )
    .into()
}

/// Returns the maximum valid day for a given month and year.
fn max_day_for_month(year: i32, month: i32) -> u32 {
    CivilDate::new(
        i16::try_from(year).expect("validated year"),
        i8::try_from(month).expect("validated month"),
        1,
    )
    .map_or(31, |date| {
        u32::try_from(date.days_in_month()).expect("positive day count")
    })
}

/// Creates a date from a proleptic Gregorian ordinal value.
pub(crate) fn from_ordinal(ordinal: i32) -> RunResult<Date> {
    let Ok(date) =
        civil::date(1970, 1, 1).checked_add(SignedDuration::from_secs((i64::from(ordinal) - 719_163) * 86_400))
    else {
        return Err(date_value_out_of_range());
    };
    if !(1..=9999).contains(&date.year()) {
        return Err(date_value_out_of_range());
    }
    Ok(Date(date))
}

/// CPython's `OverflowError` for arithmetic that leaves years 1 to 9999.
pub(crate) fn date_value_out_of_range() -> RunError {
    ExcType::overflow_error("date value out of range")
}

/// Returns the proleptic Gregorian ordinal (`1 == 0001-01-01`) for a date.
#[must_use]
pub(crate) fn to_ordinal(date: Date) -> i32 {
    i32::try_from(date.0.duration_since(civil::date(1, 1, 1)).as_secs() / 86_400 + 1).expect("civil ordinal fits i32")
}

/// Returns civil components `(year, month, day)`.
#[must_use]
pub(crate) fn to_ymd(date: Date) -> (i32, u32, u32) {
    (
        i32::from(date.0.year()),
        u32::try_from(date.0.month()).expect("positive month"),
        u32::try_from(date.0.day()).expect("positive day"),
    )
}

/// Constructor for `date(year, month, day)`.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DateInitArgs { year, month, day } = DateInitArgs::from_args(args, vm)?;
    let date = from_ymd(year, month, day)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Date(date))))
}

/// Argument shape for `date(year, month, day)`.
///
/// CPython's `date()` is C-implemented (`PyArg_ParseTupleAndKeywords`), hence
/// `style = c` — "function takes at most N arguments", "function missing
/// required argument 'X' (pos N)", etc. Unlike `datetime()` it has no
/// keyword-only fields, so the derive keeps the plain (non-"positional")
/// at-most wording automatically.
#[derive(FromArgs)]
#[from_args(name = "function", style = c, at_most_total)]
struct DateInitArgs {
    year: i32,
    month: i32,
    day: i32,
}

/// `date.min`, `date.max` and `date.resolution`.
pub(crate) fn class_attr(attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
    Some(match attr.static_string(vm.interns)? {
        StaticStrings::Min => allocate_ymd(1, 1, 1, vm.heap),
        StaticStrings::Max => allocate_ymd(9999, 12, 31, vm.heap),
        StaticStrings::Resolution => timedelta::allocate_micros(DAY_MICROSECONDS, vm.heap),
        _ => return None,
    })
}

/// `date.today()`, `date.fromtimestamp()` and `date.fromisoformat()`.
pub(crate) fn class_method(
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Result<RunResult<CallResult>, ArgValues> {
    Ok(match attr.static_string(vm.interns) {
        Some(StaticStrings::Today) => class_today(vm, args),
        Some(StaticStrings::Fromisoformat) => class_fromisoformat(vm.heap, args, vm.interns).map(CallResult::Value),
        Some(StaticStrings::Fromordinal) => class_fromordinal(vm, args).map(CallResult::Value),
        Some(StaticStrings::Fromtimestamp) => class_fromtimestamp(vm, args).map(CallResult::Value),
        _ => return Err(args),
    })
}

/// Reads `date.today()` from the session's clock in the session zone. A `CallHost`
/// clock requests a `DateToday` answer constructed with `MontyObject::date`.
pub(crate) fn class_today(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    args.check_zero_args("date.today", vm.heap)?;
    Ok(match datetime::sandbox_instant(vm)? {
        None => CallResult::OsCall(OsFunctionCall::DateToday),
        Some(utc) => {
            let local = datetime::sandbox_local_wall_clock(vm, utc)?;
            CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::Date(Date(local.date())))))
        }
    })
}

/// Classmethod `date.fromtimestamp(timestamp)`: the session-local date of a Unix
/// timestamp, read in the session zone as CPython reads it in the host's.
pub(crate) fn class_fromtimestamp(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args
        .reject_kwargs("date.fromtimestamp", vm.heap)?
        .get_one_arg("date.fromtimestamp", vm.heap)?;
    defer_drop!(value, vm);
    // `date` floors to whole seconds and uses `__index__`-style wording, where
    // `datetime` rounds to microseconds and names both numeric types.
    let (seconds, micros) = datetime::timestamp_parts(value, datetime::TimestampRounding::Floor, vm)?
        .ok_or_else(|| ExcType::type_error_not_integer(&value.py_type_name(vm)))?;
    let offset = datetime::timestamp_zone(seconds, micros, &vm.env.os_policy.timezone).offset_seconds;
    let local = datetime::timestamp_wall_clock(seconds, micros, offset)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Date(Date(local.date())))))
}

/// Constructs a `Date` from a proleptic Gregorian day ordinal.
pub(crate) fn date_from_ordinal(ordinal: i32) -> RunResult<Date> {
    if ordinal < 1 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "ordinal must be >= 1").into());
    }
    if ordinal > 3_652_059 {
        let year = civil_year_from_days(i64::from(ordinal) - 719_163);
        return Err(year_out_of_range(year));
    }
    let date = civil::date(1970, 1, 1)
        .checked_add(SignedDuration::from_secs((i64::from(ordinal) - 719_163) * 86_400))
        .expect("ordinal in 1..=3_652_059 is always a valid Gregorian date");
    Ok(Date(date))
}

/// Classmethod `date.fromordinal(ordinal)`: construct a `date` from a proleptic Gregorian ordinal.
pub(crate) fn class_fromordinal(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FromOrdinalArgs { ordinal } = FromOrdinalArgs::from_args(args, vm)?;
    let date = date_from_ordinal(ordinal)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Date(date))))
}

/// Classmethod `date.fromisoformat(date_string)`.
///
/// Parses ISO 8601 date strings in the formats `YYYY-MM-DD` and `YYYYMMDD`,
/// matching CPython 3.11+ behavior.
pub(crate) fn class_fromisoformat(heap: &mut Heap, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args
        .reject_kwargs("date.fromisoformat", heap)?
        .get_one_arg("date.fromisoformat", heap)?;
    let s = extract_str_arg(&value, "fromisoformat", heap, interns);
    value.drop_with(heap);
    let s = s?;

    let date = parse_iso_date(&s)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("Invalid isoformat string: '{s}'")))?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(date))))
}

/// Parses an ISO 8601 date string into a `Date`.
///
/// Uses speedate for Python-compatible ISO 8601 parsing.
fn parse_iso_date(s: &str) -> Option<Date> {
    let parsed = speedate::Date::parse_bytes(s.as_bytes()).ok()?;
    from_ymd(i32::from(parsed.year), i32::from(parsed.month), i32::from(parsed.day)).ok()
}

/// Allocates a `date` from already-in-range components.
///
/// For Rust-side construction where the values are known good (the `date.min` /
/// `date.max` class constants); anything derived from user input must go
/// through [`from_ymd`] so the components are validated.
pub(crate) fn allocate_ymd(year: i32, month: i32, day: i32, heap: &Heap) -> Value {
    let date = from_ymd(year, month, day).expect("caller guarantees in-range date components");
    Value::Ref(heap.allocate(HeapData::Date(date)))
}

/// Extracts a string from a `Value` for use by classmethods.
pub(crate) fn extract_str_arg(value: &Value, method_name: &str, heap: &Heap, interns: &Interns) -> RunResult<String> {
    value
        .as_str_heap(heap, interns)
        .map(str::to_owned)
        .ok_or_else(|| ExcType::type_error(format!("{method_name}: argument must be str")))
}

impl HeapItem for Date {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `Date`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident dates.
impl<'h> PyTrait<'h> for HeapObjectRead<'h, Date> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::Date.into()
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::Date(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(*self.get(vm.heap) == *other.get(vm.heap)))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        Ok(CmpOrder::from_total(self.get(vm.heap).partial_cmp(other.get(vm.heap))))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        write!(f, "datetime.date({year}, {month}, {day})")?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        Ok(allocate_string(format!("{year:04}-{month:02}-{day:02}"), vm.heap))
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(HeapReadOutput::TimeDelta(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        py_add(*self.get(vm.heap), *other.get(vm.heap), vm.heap).map(Some)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::Date(other)) => Ok(py_sub_date(*self.get(vm.heap), *other.get(vm.heap), vm.heap)),
            Some(HeapReadOutput::TimeDelta(other)) => {
                py_sub_timedelta(*self.get(vm.heap), *other.get(vm.heap), vm.heap).map(Some)
            }
            _ => Ok(None),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let date = *self.get(vm.heap);
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Isoformat) => {
                args.check_zero_args("date.isoformat", vm.heap)?;
                let (year, month, day) = to_ymd(date);
                Ok(CallResult::Value(allocate_string_no_interning(
                    format!("{year:04}-{month:02}-{day:02}"),
                    vm.heap,
                )))
            }
            Some(StaticStrings::Strftime) => {
                let StrftimeArgs { format } = StrftimeArgs::from_args(args, vm)?;
                defer_drop!(format, vm);
                let formatted = format_date_strftime(date, format.as_str(vm), &vm.heap.tracker)?;
                Ok(CallResult::Value(allocate_string(formatted, vm.heap)))
            }
            Some(StaticStrings::Replace) => {
                let (year, month, day) = to_ymd(date);
                let DateReplaceArgs {
                    year: new_year,
                    month: new_month,
                    day: new_day,
                } = DateReplaceArgs::from_args(args, vm)?;
                let new_date = from_ymd(
                    new_year.unwrap_or(year),
                    new_month.unwrap_or(i32::try_from(month).expect("month in 1..=12")),
                    new_day.unwrap_or(i32::try_from(day).expect("day in 1..=31")),
                )?;
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Date(new_date)),
                )))
            }
            Some(StaticStrings::Weekday) => {
                args.check_zero_args("date.weekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    date.0.weekday().to_monday_zero_offset(),
                ))))
            }
            Some(StaticStrings::Isoweekday) => {
                args.check_zero_args("date.isoweekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    date.0.weekday().to_monday_one_offset(),
                ))))
            }
            Some(StaticStrings::Toordinal) => {
                args.check_zero_args("date.toordinal", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(to_ordinal(date)))))
            }
            // A classmethod, so CPython also reaches it through an instance.
            Some(StaticStrings::Fromordinal) => class_fromordinal(vm, args).map(CallResult::Value),
            Some(StaticStrings::Fromtimestamp) => class_fromtimestamp(vm, args).map(CallResult::Value),
            _ => Err(ExcType::attribute_error_method(Type::Date, attr, args, vm)),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Year) => Ok(Some(CallResult::Value(Value::Int(i64::from(year))))),
            Some(StaticStrings::Month) => Ok(Some(CallResult::Value(Value::Int(i64::from(month))))),
            Some(StaticStrings::Day) => Ok(Some(CallResult::Value(Value::Int(i64::from(day))))),
            _ => Ok(None),
        }
    }
}

/// `date - date` returns a timedelta with the difference in days.
pub(crate) fn py_sub_date(a: Date, b: Date, heap: &mut Heap) -> Option<Value> {
    let diff_days = i64::from(to_ordinal(a)) - i64::from(to_ordinal(b));
    let delta = timedelta::from_total_microseconds(i128::from(diff_days) * MICROSECONDS_PER_DAY).ok()?;
    Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))))
}

/// `date + timedelta` helper.
pub(crate) fn py_add(date: Date, delta: TimeDelta, heap: &mut Heap) -> RunResult<Value> {
    let (days, _, _) = timedelta::components(&delta);
    let value = offset_days(date, days)?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(value))))
}

/// `date - timedelta` helper.
pub(crate) fn py_sub_timedelta(date: Date, delta: TimeDelta, heap: &mut Heap) -> RunResult<Value> {
    let (days, _, _) = timedelta::components(&delta);
    let value = offset_days(date, -days)?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(value))))
}

/// The date `days` after `date`, or CPython's `OverflowError` outside years 1 to 9999.
fn offset_days(date: Date, days: i32) -> RunResult<Date> {
    // A timedelta has at most 999999999 days, so this cannot overflow `i32`
    from_ordinal(to_ordinal(date) + days)
}

/// Formats a [`Date`] with a `strftime` directive string, shared by the
/// `date.strftime()` method and f-string formatting (`f"{d:%Y-%m-%d}"`).
///
/// Unknown directives pass through verbatim, matching glibc/Linux CPython.
/// Zone directives are empty because a date has no timezone.
pub(crate) fn format_date_strftime(date: Date, format: &str, tracker: &ResourceTracker) -> RunResult<String> {
    // Anchored at midnight so time directives render CPython's zeros
    // (`date(2024, 6, 15).strftime('%H:%M')` is `'00:00'` on both) rather than
    // failing for want of a time component.
    let anchored = date.0.at(0, 0, 0, 0);
    let format = rewrite_zone_directives(format, None, None, tracker)?;
    render_strftime(anchored, &format, tracker)
}

/// Substitutes the zone directives CPython fills from `utcoffset()` and
/// `tzname()`: `%z` (`±HHMM[SS]`), `%:z` (`±HH:MM[:SS]`) and `%Z` (the zone
/// name), all empty for a naive value. Jiff renders the naive components
/// and cannot supply them. A `%` in the name is doubled to stay literal.
/// Every `%Z` copies the name, so the output is built against the tracker.
pub(crate) fn rewrite_zone_directives<'f>(
    format: &'f str,
    offset_seconds: Option<i32>,
    name: Option<&str>,
    tracker: &ResourceTracker,
) -> RunResult<Cow<'f, str>> {
    // One byte-pair pass: three `str::contains` calls cost more than the whole
    // Jiff render on a short format. `%%z` and `%:` without `z` pass here
    // and are handled by the loop below.
    let may_have_zone = format
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'%' && matches!(pair[1], b'z' | b'Z' | b':'));
    if !may_have_zone {
        return Ok(Cow::Borrowed(format));
    }
    let zone_name = offset_seconds.map(|offset| tzname_string(offset, name).replace('%', "%%"));
    let mut out = StringBuilder::with_capacity(format.len(), tracker)?;
    let mut rest = format;
    while let Some(percent) = rest.find('%') {
        let (before, from_percent) = rest.split_at(percent);
        out.push_str(before)?;
        let directive = &from_percent[1..];
        let consumed = if let Some(after) = directive.strip_prefix('z') {
            if let Some(offset) = offset_seconds {
                out.push_str(&format_offset_compact(offset))?;
            }
            directive.len() - after.len() + 1
        } else if let Some(after) = directive.strip_prefix(":z") {
            if let Some(offset) = offset_seconds {
                out.push_str(&format_offset_hms(offset))?;
            }
            directive.len() - after.len() + 1
        } else if let Some(after) = directive.strip_prefix('Z') {
            if let Some(zone_name) = &zone_name {
                out.push_str(zone_name)?;
            }
            directive.len() - after.len() + 1
        } else {
            // Copy the directive whole, so `%%z` consumes `%%` and leaves `z` as text.
            out.push('%')?;
            match directive.chars().next() {
                Some(c) => {
                    out.push(c)?;
                    1 + c.len_utf8()
                }
                None => 1,
            }
        };
        rest = &from_percent[consumed..];
    }
    out.push_str(rest)?;
    out.finish_raw().map(Cow::Owned)
}

/// Formats civil components without panicking on unsupported directives.
/// POSIX locale names and literal unknown directives preserve sandbox behavior.
pub(crate) fn render_strftime(datetime: civil::DateTime, format: &str, tracker: &ResourceTracker) -> RunResult<String> {
    let mut out = StringBuilder::new(tracker);
    let broken = BrokenDownTime::from(datetime);
    let config = Config::new().custom(PosixCustom::new()).lenient(true);
    let mut rest = format;
    while let Some(at) = rest.find('%') {
        let (literal, directive) = rest.split_at(at);
        out.push_str(literal)?;
        if directive.starts_with("%#z") {
            return Err(invalid_strftime_error());
        }
        let (end, conversion) = strftime_directive(directive);
        let spec = &directive[..end];
        // Only glibc's conversions reach Jiff, so its extensions (`%q`, `%.f`, `%N`) stay literal.
        let jiff_spec = match conversion {
            Some('s') => {
                let seconds = datetime::epoch_duration(datetime).as_nanos().div_euclid(1_000_000_000);
                out.push_str(&seconds.to_string())?;
                None
            }
            // CPython substitutes only a bare `%f`, always six digits.
            Some('f') if end == 2 => Some("%6f"),
            Some(c) if GLIBC_CONVERSIONS.contains(c) => Some(spec),
            _ => {
                out.push_str(spec)?;
                None
            }
        };
        if let Some(jiff_spec) = jiff_spec
            && broken
                .format_with_config(&config, jiff_spec, &mut StdFmtWrite(&mut out))
                .is_err()
        {
            // A resource error is retained by the builder and takes precedence.
            out.finish_raw()?;
            return Err(invalid_strftime_error());
        }
        rest = &directive[end..];
    }
    out.push_str(rest)?;
    out.finish_raw()
}

/// glibc's `strftime` conversions; any other directive is copied through verbatim.
const GLIBC_CONVERSIONS: &str = "%ABCDFGHIMPRSTUVWXYZabcdeghjklmnprstuwxyz";

/// Splits a `%` directive into its byte length and conversion, after glibc's flags and width.
/// `%:z` keeps its colon; `None` means the format ended mid-directive.
fn strftime_directive(directive: &str) -> (usize, Option<char>) {
    let mut start = directive[1..]
        .find(|c: char| !matches!(c, '-' | '_' | '0' | '^' | '#') && !c.is_ascii_digit())
        .map_or(directive.len(), |at| 1 + at);
    if directive[start..].starts_with(":z") {
        start += 1;
    }
    match directive[start..].chars().next() {
        Some(c) => (start + c.len_utf8(), Some(c)),
        None => (start, None),
    }
}

/// The `ValueError` for unsupported offset-dependent directives (`%#z`).
///
/// Unrecognised directives no longer reach this path — the lenient parser
/// emits them verbatim to match glibc/Linux CPython (`strftime('%Q') == '%Q'`),
/// rather than CPython's macOS behaviour (`'Q'`) which we deliberately don't
/// follow; see `limitations/datetime.md`.
pub(crate) fn invalid_strftime_error() -> RunError {
    SimpleException::new_msg(ExcType::ValueError, "Invalid format string".to_owned()).into()
}

/// Argument shape for `date.strftime(format)` and `datetime.strftime(format)`.
///
/// CPython implements `strftime` as a C method and reports errors with the
/// bare method name (no class prefix), so we use `style = c_named` + the
/// `"strftime"` descriptor — matching wordings like
/// `strftime() missing required argument 'format' (pos 1)` and
/// `strftime() takes at most 1 argument (2 given)`.
///
/// `bad_arg` opts the wrong-type wording into CPython's `_PyArg_BadArgument`
/// form (`strftime() argument 1 must be str, not <type>`), including the
/// `None`-vs-`NoneType` special case — so the type-check logic lives in
/// the derive rather than a hand-written extract helper.
#[derive(FromArgs)]
#[from_args(name = "strftime", style = c_named, at_most_total, bad_arg)]
pub(crate) struct StrftimeArgs {
    pub(crate) format: StrArg,
}

/// Keyword arguments for `date.replace()`. All keyword-only; absent fields
/// inherit the original date's component via `unwrap_or` at the call site.
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct DateReplaceArgs {
    #[from_args(kw_only, default)]
    year: Option<i32>,
    #[from_args(kw_only, default)]
    month: Option<i32>,
    #[from_args(kw_only, default)]
    day: Option<i32>,
}

/// Argument shape for `date.fromordinal(ordinal)` and `datetime.fromordinal(ordinal)`.
#[derive(FromArgs)]
#[from_args(name = "fromordinal", style = parse_tuple)]
pub(crate) struct FromOrdinalArgs {
    #[from_args(pos_only)]
    pub(crate) ordinal: i32,
}
