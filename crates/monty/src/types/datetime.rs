//! Python `datetime.datetime` implementation.
//!
//! Monty stores datetimes with chrono primitives and layers CPython-compatible
//! constructor rules, aware/naive comparison semantics, and arithmetic on top.

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;
use chrono::{Datelike, FixedOffset, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta, Timelike};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::{
        AttrCallResult, PyTrait, TimeDelta, TimeZone, Type, date, str, str::StringRepr, timedelta, timezone,
        value_to_i32,
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
    heap: &mut Heap<impl ResourceTracker>,
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

    attach_or_allocate_tzinfo_ref(&mut datetime, tzinfo_ref, heap)?;
    Ok(datetime)
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

/// Constructor for `datetime(...)`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);
    // Keep the provided tzinfo object alive across argument parsing so we can
    // safely retain its identity in the constructed datetime.
    let retained_tzinfo = Value::None;
    defer_drop_mut!(retained_tzinfo, heap);

    let mut year: Option<i32> = None;
    let mut month: Option<i32> = None;
    let mut day: Option<i32> = None;
    let mut hour: i32 = 0;
    let mut minute: i32 = 0;
    let mut second: i32 = 0;
    let mut microsecond: i32 = 0;
    let mut tzinfo: Option<TimeZone> = None;
    let mut tzinfo_ref: Option<HeapId> = None;
    let mut seen_hour = false;
    let mut seen_minute = false;
    let mut seen_second = false;
    let mut seen_microsecond = false;
    let mut seen_tzinfo = false;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => year = Some(value_to_i32(arg)?),
            1 => month = Some(value_to_i32(arg)?),
            2 => day = Some(value_to_i32(arg)?),
            3 => {
                hour = value_to_i32(arg)?;
                seen_hour = true;
            }
            4 => {
                minute = value_to_i32(arg)?;
                seen_minute = true;
            }
            5 => {
                second = value_to_i32(arg)?;
                seen_second = true;
            }
            6 => {
                microsecond = value_to_i32(arg)?;
                seen_microsecond = true;
            }
            7 => {
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(arg, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            _ => {
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("function takes at most 8 positional arguments ({} given)", index + 1),
                )
                .into());
            }
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match key_name.string_id() {
            Some(id) if id == StaticStrings::Year => {
                if year.is_some() {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "year", 1));
                }
                year = Some(value_to_i32(value)?);
            }
            Some(id) if id == StaticStrings::Month => {
                if month.is_some() {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "month", 2));
                }
                month = Some(value_to_i32(value)?);
            }
            Some(id) if id == StaticStrings::Day => {
                if day.is_some() {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "day", 3));
                }
                day = Some(value_to_i32(value)?);
            }
            Some(id) if id == StaticStrings::Hour => {
                if seen_hour {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "hour", 4));
                }
                hour = value_to_i32(value)?;
                seen_hour = true;
            }
            Some(id) if id == StaticStrings::Minute => {
                if seen_minute {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "minute", 5));
                }
                minute = value_to_i32(value)?;
                seen_minute = true;
            }
            Some(id) if id == StaticStrings::Second => {
                if seen_second {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "second", 6));
                }
                second = value_to_i32(value)?;
                seen_second = true;
            }
            Some(id) if id == StaticStrings::Microsecond => {
                if seen_microsecond {
                    return Err(ExcType::type_error_positional_keyword_conflict(
                        "function",
                        "microsecond",
                        7,
                    ));
                }
                microsecond = value_to_i32(value)?;
                seen_microsecond = true;
            }
            Some(id) if id == StaticStrings::Tzinfo => {
                if seen_tzinfo {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "tzinfo", 8));
                }
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(value, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            _ => {
                return Err(ExcType::type_error_c_unexpected_keyword(key_name.as_str(interns)));
            }
        }
    }

    let Some(year) = year else {
        return Err(ExcType::type_error_c_missing_required("year", 1));
    };
    let Some(month) = month else {
        return Err(ExcType::type_error_c_missing_required("month", 2));
    };
    let Some(day) = day else {
        return Err(ExcType::type_error_c_missing_required("day", 3));
    };

    let dt = from_components(
        year,
        month,
        day,
        hour,
        minute,
        second,
        microsecond,
        tzinfo,
        tzinfo_ref,
        heap,
    )?;
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))?))
}

/// Classmethod implementation for `datetime.now(tz=None)`.
///
/// Issues a `DateTimeNow` OS call with one argument: the timezone value
/// (`Value::None` for naive, `Value::Ref` to a `TimeZone` for aware).
/// The host should return `MontyObject::DateTime` directly.
pub(crate) fn class_now(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<AttrCallResult> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut tz_value = Value::None;
    let mut seen_tz = false;

    for (index, arg) in pos.by_ref().enumerate() {
        if index == 0 {
            if let Err(e) = validate_tz_arg(&arg, heap) {
                arg.drop_with_heap(heap);
                return Err(e);
            }
            tz_value = arg;
            seen_tz = true;
        } else {
            arg.drop_with_heap(heap);
            tz_value.drop_with_heap(heap);
            return Err(ExcType::type_error_method_at_most("now", 1, index + 1));
        }
    }

    for (key, value) in kwargs {
        let key_name = key.as_either_str(heap);
        key.drop_with_heap(heap);

        let Some(key_name) = key_name else {
            value.drop_with_heap(heap);
            tz_value.drop_with_heap(heap);
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        if key_name.string_id() != Some(StaticStrings::Tz.into()) {
            value.drop_with_heap(heap);
            tz_value.drop_with_heap(heap);
            return Err(ExcType::type_error_unexpected_keyword("now", key_name.as_str(interns)));
        }
        if seen_tz {
            value.drop_with_heap(heap);
            tz_value.drop_with_heap(heap);
            return Err(ExcType::type_error_method_at_most("now", 1, 2));
        }
        if let Err(e) = validate_tz_arg(&value, heap) {
            value.drop_with_heap(heap);
            tz_value.drop_with_heap(heap);
            return Err(e);
        }
        tz_value = value;
        seen_tz = true;
    }

    Ok(AttrCallResult::OsCall(
        OsFunction::DateTimeNow,
        ArgValues::One(tz_value),
    ))
}

/// Classmethod `datetime.strptime(date_string, format)`.
///
/// Parses a date/time string using the given format. Delegates to chrono's
/// `NaiveDateTime::parse_from_str`, expanding Python `%f` directives into the
/// chrono widths needed to accept 1 through 6 fractional digits.
pub(crate) fn class_strptime(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<Value> {
    let (date_string_val, format_val) = args.get_two_args("datetime.strptime", heap)?;

    let date_string = date::extract_str_arg(&date_string_val, "strptime", heap, interns);
    let fmt = date::extract_str_arg(&format_val, "strptime", heap, interns);
    date_string_val.drop_with_heap(heap);
    format_val.drop_with_heap(heap);
    let date_string = date_string?;
    let fmt = fmt?;

    // Python's `%f` accepts 1..=6 digits and right-pads with zeros, while chrono
    // requires an explicit width. Try all valid `%f` widths before reporting a
    // mismatch so `datetime.strptime(..., '%f')` matches CPython.
    let Some(naive) = parse_strptime_naive(&date_string, &fmt) else {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("time data '{date_string}' does not match format '{fmt}'"),
        )
        .into());
    };

    if !year_in_python_range(naive.date().year()) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "year is out of range").into());
    }

    let dt = DateTime {
        naive,
        offset_seconds: None,
        timezone_name: None,
        tzinfo_ref: None,
    };
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))?))
}

/// Classmethod `datetime.fromisoformat(date_string)`.
///
/// Parses ISO 8601 datetime strings. Supports the following formats:
/// - `YYYY-MM-DD` (date only, time defaults to midnight)
/// - `YYYY-MM-DDTHH:MM` or `YYYY-MM-DD HH:MM`
/// - `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD HH:MM:SS`
/// - `YYYY-MM-DDTHH:MM:SS.ffffff`
/// - Any of the above with `+HH:MM` or `+HH:MM:SS` timezone suffix
pub(crate) fn class_fromisoformat(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<Value> {
    let value = args.get_one_arg("datetime.fromisoformat", heap)?;
    let s = date::extract_str_arg(&value, "fromisoformat", heap, interns);
    value.drop_with_heap(heap);
    let s = s?;

    let dt = parse_iso_datetime(&s, heap)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("Invalid isoformat string: '{s}'")))?;

    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))?))
}

/// Parses an ISO 8601 datetime string into a `DateTime`.
///
/// Uses speedate's RFC 3339 parser for Python-compatible ISO 8601 parsing (the
/// same parser used by pydantic). Falls back to date-only parsing for bare
/// `YYYY-MM-DD` inputs.
fn parse_iso_datetime(s: &str, heap: &mut Heap<impl ResourceTracker>) -> Option<DateTime> {
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
pub(crate) fn py_add(
    datetime: &DateTime,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let chrono_delta = timedelta::chrono_delta(delta);

    let next = if let Some(offset) = datetime.offset_seconds {
        let Some(utc) = to_utc_naive(datetime) else {
            return Ok(None);
        };
        let Some(next_utc) = utc.checked_add_signed(chrono_delta) else {
            return Ok(None);
        };
        from_utc_naive_with_timezone_parts(next_utc, offset, datetime.timezone_name.clone())
    } else {
        let Some(next_local) = datetime.naive.checked_add_signed(chrono_delta) else {
            return Ok(None);
        };
        from_local_naive(next_local)
    };

    let Some(mut next) = next else {
        return Ok(None);
    };
    attach_or_allocate_tzinfo_ref(&mut next, datetime.tzinfo_ref, heap)?;
    Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(next))?)))
}

/// `datetime - timedelta`
pub(crate) fn py_sub_timedelta(
    datetime: &DateTime,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let chrono_delta = timedelta::chrono_delta(delta);

    let next = if let Some(offset) = datetime.offset_seconds {
        let Some(utc) = to_utc_naive(datetime) else {
            return Ok(None);
        };
        let Some(next_utc) = utc.checked_sub_signed(chrono_delta) else {
            return Ok(None);
        };
        from_utc_naive_with_timezone_parts(next_utc, offset, datetime.timezone_name.clone())
    } else {
        let Some(next_local) = datetime.naive.checked_sub_signed(chrono_delta) else {
            return Ok(None);
        };
        from_local_naive(next_local)
    };

    let Some(mut next) = next else {
        return Ok(None);
    };
    attach_or_allocate_tzinfo_ref(&mut next, datetime.tzinfo_ref, heap)?;
    Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(next))?)))
}

/// `datetime - datetime` returns a timedelta with the difference.
///
/// Both datetimes must be either aware or naive; mixing returns `Ok(None)`.
pub(crate) fn py_sub_datetime(
    a: &DateTime,
    b: &DateTime,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    if is_aware(a) != is_aware(b) {
        return Ok(None);
    }

    let diff = if is_aware(a) {
        let Some(lhs_utc) = to_utc_naive(a) else {
            return Ok(None);
        };
        let Some(rhs_utc) = to_utc_naive(b) else {
            return Ok(None);
        };
        lhs_utc.signed_duration_since(rhs_utc)
    } else {
        a.naive.signed_duration_since(b.naive)
    };

    let Ok(delta) = timedelta::from_chrono(diff) else {
        return Ok(None);
    };
    Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
}

fn tzinfo_from_value(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<(Option<TimeZone>, Option<HeapId>)> {
    match value {
        Value::None => Ok((None, None)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(tz) => Ok((Some(tz.clone()), Some(*id))),
            other => Err(ExcType::type_error_tzinfo(other.py_type())),
        },
        _ => Err(ExcType::type_error_tzinfo(value.py_type_shallow())),
    }
}

/// Validates that a value is a valid timezone argument (`None` or `TimeZone`).
///
/// Used by `class_now` to validate the `tz` argument before passing it through
/// to the OS call. Unlike `tzinfo_from_value`, this does not extract the timezone
/// data — it only checks the type.
fn validate_tz_arg(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<()> {
    match value {
        Value::None => Ok(()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(_) => Ok(()),
            other => Err(ExcType::type_error_tzinfo(other.py_type())),
        },
        _ => Err(ExcType::type_error_tzinfo(value.py_type_shallow())),
    }
}

/// Updates a temporary retained tzinfo value used during datetime construction.
///
/// This keeps the timezone object alive while constructor argument values are
/// being dropped, so we can safely increment the same object again when
/// attaching it to the resulting datetime.
fn update_retained_tzinfo(
    retained_tzinfo: &mut Value,
    tzinfo_ref: Option<HeapId>,
    heap: &mut Heap<impl ResourceTracker>,
) {
    let old = mem::replace(retained_tzinfo, Value::None);
    old.drop_with_heap(heap);
    *retained_tzinfo = if let Some(tzinfo_ref) = tzinfo_ref {
        heap.inc_ref(tzinfo_ref);
        Value::Ref(tzinfo_ref)
    } else {
        Value::None
    };
}

/// Attaches a stable tzinfo identity to aware datetimes.
///
/// If `preferred_tzinfo_ref` is provided, it is retained and reused so identity
/// semantics (`is`) match the input timezone object. Otherwise we allocate (or
/// canonicalize to the UTC singleton) a timezone object once and reuse it.
fn attach_or_allocate_tzinfo_ref(
    datetime: &mut DateTime,
    preferred_tzinfo_ref: Option<HeapId>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<(), ResourceError> {
    let Some(offset_seconds) = datetime.offset_seconds else {
        datetime.tzinfo_ref = None;
        return Ok(());
    };

    let tzinfo_ref = if let Some(tzinfo_ref) = preferred_tzinfo_ref {
        heap.inc_ref(tzinfo_ref);
        tzinfo_ref
    } else {
        allocate_tzinfo_ref(offset_seconds, datetime.timezone_name.clone(), heap)?
    };
    datetime.tzinfo_ref = Some(tzinfo_ref);
    Ok(())
}

/// Allocates a timezone object for datetime storage, canonicalizing UTC to the
/// shared singleton object.
fn allocate_tzinfo_ref(
    offset_seconds: i32,
    timezone_name: Option<String>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<HeapId, ResourceError> {
    if offset_seconds == 0 && timezone_name.is_none() {
        let utc = heap.get_timezone_utc()?;
        defer_drop!(utc, heap);
        let Value::Ref(id) = utc else {
            unreachable!("timezone.utc must be heap-allocated");
        };
        heap.inc_ref(*id);
        return Ok(*id);
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

/// Formats a datetime as an ISO 8601 string with the given separator.
///
/// Matches CPython's `datetime.isoformat(sep='T')`.
fn format_isoformat(dt: &DateTime, sep: char) -> String {
    let Some((year, month, day, hour, minute, second, microsecond)) = to_components(dt) else {
        return "<out of range>".to_owned();
    };
    let mut s = format!("{year:04}-{month:02}-{day:02}{sep}{hour:02}:{minute:02}:{second:02}");
    if microsecond != 0 {
        write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
    }
    if let Some(offset) = offset_seconds(dt) {
        s.push_str(&timezone::format_offset_hms(offset));
    }
    s
}

/// Computes the POSIX timestamp for a datetime.
///
/// For naive datetimes, treats them as local time and computes the UTC epoch
/// assuming the local wall clock matches UTC (CPython's `datetime.timestamp()`
/// for naive datetimes actually uses the system timezone, but since Monty runs
/// in a sandbox with no timezone database, treating naive as UTC is the best
/// approximation).
///
/// For aware datetimes, converts to UTC first via the stored offset.
fn compute_timestamp(dt: &DateTime) -> f64 {
    let utc_naive = if dt.offset_seconds.is_some() {
        to_utc_naive(dt).unwrap_or(dt.naive)
    } else {
        dt.naive
    };
    let secs = utc_naive.and_utc().timestamp();
    let micros = utc_naive.and_utc().timestamp_subsec_micros();
    secs as f64 + f64::from(micros) / 1_000_000.0
}

/// Parses keyword arguments for `datetime.replace()`.
///
/// Returns a new datetime value with replaced components.
fn extract_datetime_replace_kwargs(
    args: ArgValues,
    dt: &DateTime,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);
    let retained_tzinfo = Value::None;
    defer_drop_mut!(retained_tzinfo, heap);

    let mut year = dt.naive.date().year();
    let mut month = i32::try_from(dt.naive.date().month()).expect("month in 1..12");
    let mut day = i32::try_from(dt.naive.date().day()).expect("day in 1..31");
    let mut hour = i32::try_from(dt.naive.time().hour()).expect("hour in 0..23");
    let mut minute = i32::try_from(dt.naive.time().minute()).expect("minute in 0..59");
    let mut second = i32::try_from(dt.naive.time().second()).expect("second in 0..59");
    let mut microsecond = i32::try_from(dt.naive.and_utc().timestamp_subsec_micros()).expect("micros in 0..999999");
    let mut tzinfo = timezone_info(dt);
    let mut tzinfo_ref = dt.tzinfo_ref;

    // replace() takes no positional args
    if let Some(arg) = pos.next() {
        arg.drop_with_heap(heap);
        return Err(ExcType::type_error(
            "datetime.replace() takes 0 positional arguments".to_owned(),
        ));
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match key_name.string_id() {
            Some(id) if id == StaticStrings::Year => year = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Month => month = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Day => day = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Hour => hour = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Minute => minute = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Second => second = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Microsecond => microsecond = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Tzinfo => {
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(value, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
            }
            _ => {
                return Err(ExcType::type_error_unexpected_keyword(
                    "replace",
                    key_name.as_str(interns),
                ));
            }
        }
    }

    let new_dt = from_components(
        year,
        month,
        day,
        hour,
        minute,
        second,
        microsecond,
        tzinfo,
        tzinfo_ref,
        heap,
    )?;
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(new_dt))?))
}

impl HeapItem for DateTime {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.timezone_name.as_ref().map_or(0, String::len)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(tzinfo_ref) = self.tzinfo_ref {
            stack.push(tzinfo_ref);
        }
    }
}

/// `HeapRead`-based dispatch for `DateTime`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident datetimes.
impl<'h> PyTrait<'h> for HeapRead<'h, DateTime> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::DateTime
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        if is_aware(a) != is_aware(b) {
            return Ok(false);
        }
        if is_aware(a) {
            return Ok(utc_micros(a) == utc_micros(b));
        }
        Ok(local_micros(a) == local_micros(b))
    }

    fn py_hash(
        &self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<u64>, ResourceError> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(hasher.finish()))
    }

    fn py_cmp(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<Ordering>, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        if is_aware(a) != is_aware(b) {
            return Ok(None);
        }
        if is_aware(a) {
            return Ok(utc_micros(a).partial_cmp(&utc_micros(b)));
        }
        Ok(local_micros(a).partial_cmp(&local_micros(b)))
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
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

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        let dt = self.get(vm.heap);
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(dt) else {
            return Ok(Cow::Borrowed("<out of range>"));
        };
        let mut s = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
        if microsecond != 0 {
            write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
        }
        if let Some(offset) = offset_seconds(dt) {
            s.push_str(&timezone::format_offset_hms(offset));
        }
        Ok(Cow::Owned(s))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let dt = self.get(vm.heap).clone();
        match attr.string_id() {
            Some(id) if id == StaticStrings::Isoformat => {
                args.check_zero_args("datetime.isoformat", vm.heap)?;
                let s = format_isoformat(&dt, 'T');
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Str(str::Str::new(s)))?,
                )))
            }
            Some(id) if id == StaticStrings::Strftime => {
                let fmt = date::extract_strftime_arg(args, "datetime.strftime", vm.heap, vm.interns)?;
                let formatted = dt.naive.format(&fmt).to_string();
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Str(str::Str::new(formatted)))?,
                )))
            }
            Some(id) if id == StaticStrings::Replace => {
                let result = extract_datetime_replace_kwargs(args, &dt, vm.heap, vm.interns)?;
                Ok(CallResult::Value(result))
            }
            Some(id) if id == StaticStrings::Weekday => {
                args.check_zero_args("datetime.weekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    dt.naive.date().weekday().num_days_from_monday(),
                ))))
            }
            Some(id) if id == StaticStrings::Isoweekday => {
                args.check_zero_args("datetime.isoweekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    dt.naive.date().weekday().number_from_monday(),
                ))))
            }
            Some(id) if id == StaticStrings::Date => {
                args.check_zero_args("datetime.date", vm.heap)?;
                let d = date::from_ymd(
                    dt.naive.date().year(),
                    i32::try_from(dt.naive.date().month()).expect("month in 1..12"),
                    i32::try_from(dt.naive.date().day()).expect("day in 1..31"),
                )?;
                Ok(CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::Date(d))?)))
            }
            Some(id) if id == StaticStrings::Timestamp => {
                args.check_zero_args("datetime.timestamp", vm.heap)?;
                let ts = compute_timestamp(&dt);
                Ok(CallResult::Value(Value::Float(ts)))
            }
            _ => Err(ExcType::attribute_error(Type::DateTime, attr.as_str(vm.interns))),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        // Clone to release the HeapRead borrow before accessing attributes
        // that may need to allocate (e.g. tzinfo).
        let dt = self.get(vm.heap).clone();
        match attr.string_id() {
            Some(id) if id == StaticStrings::Year => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.date().year())))))
            }
            Some(id) if id == StaticStrings::Month => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.date().month())))))
            }
            Some(id) if id == StaticStrings::Day => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.date().day())))))
            }
            Some(id) if id == StaticStrings::Hour => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.time().hour())))))
            }
            Some(id) if id == StaticStrings::Minute => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.time().minute())))))
            }
            Some(id) if id == StaticStrings::Second => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(dt.naive.time().second())))))
            }
            Some(id) if id == StaticStrings::Microsecond => Ok(Some(CallResult::Value(Value::Int(i64::from(
                dt.naive.and_utc().timestamp_subsec_micros(),
            ))))),
            Some(id) if id == StaticStrings::Tzinfo => {
                if let Some(tzinfo_ref) = dt.tzinfo_ref {
                    vm.heap.inc_ref(tzinfo_ref);
                    return Ok(Some(CallResult::Value(Value::Ref(tzinfo_ref))));
                }
                if let Some(tz) = timezone_info(&dt) {
                    if tz.offset_seconds == 0 && tz.name.is_none() {
                        return Ok(Some(CallResult::Value(vm.heap.get_timezone_utc()?)));
                    }
                    return Ok(Some(CallResult::Value(Value::Ref(
                        vm.heap.allocate(HeapData::TimeZone(tz))?,
                    ))));
                }
                Ok(Some(CallResult::Value(Value::None)))
            }
            _ => Ok(None),
        }
    }
}
