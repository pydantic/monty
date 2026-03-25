//! Python `datetime.datetime` implementation.
//!
//! Monty stores datetimes with chrono primitives and layers CPython-compatible
//! constructor rules, aware/naive comparison semantics, and arithmetic on top.

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use chrono::{
    DateTime as ChronoDateTime, Datelike, FixedOffset, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta,
    Timelike, Utc,
};
use num_traits::ToPrimitive;

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::Interns,
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::{
        AttrCallResult, PyTrait, Str, TimeDelta, TimeZone, Type, date,
        datetime_os_bridge::{DATETIME_NOW_FIXED_OFFSET_INTERNAL_MODE, DATETIME_NOW_NAIVE_INTERNAL_MODE},
        str::StringRepr,
        timedelta, timezone,
    },
    value::Value,
};

/// Number of microseconds in a single second.
const MICROS_PER_SECOND: i64 = 1_000_000;
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

impl std::hash::Hash for DateTime {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
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
        return Err(SimpleException::new_msg(ExcType::ValueError, "hour must be in 0..23").into());
    }
    if !(0..=59).contains(&minute) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "minute must be in 0..59").into());
    }
    if !(0..=59).contains(&second) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "second must be in 0..59").into());
    }
    if !(0..=999_999).contains(&microsecond) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "microsecond must be in 0..999999").into());
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

/// Creates a datetime from callback payload.
pub(crate) fn from_now_payload(
    timestamp_utc: f64,
    local_offset_seconds: i32,
    tzinfo: Option<TimeZone>,
) -> RunResult<DateTime> {
    if !timestamp_utc.is_finite() {
        return Err(
            SimpleException::new_msg(ExcType::TypeError, "datetime.now payload timestamp must be finite").into(),
        );
    }
    let micros_f = timestamp_utc * (MICROS_PER_SECOND as f64);
    if micros_f < i64::MIN as f64 || micros_f > i64::MAX as f64 {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "timestamp out of range").into());
    }
    let utc_micros = rounded_f64_to_i64(micros_f.round());

    if let Some(tz) = tzinfo {
        return from_utc_micros_with_timezone(utc_micros, tz)
            .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
    }

    let local_offset_micros = checked_offset_micros(local_offset_seconds)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE))?;
    let local_micros = utc_micros
        .checked_add(local_offset_micros)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE))?;
    from_local_unix_micros(local_micros)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into())
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
            0 => year = Some(value_to_i32(arg, heap)?),
            1 => month = Some(value_to_i32(arg, heap)?),
            2 => day = Some(value_to_i32(arg, heap)?),
            3 => {
                hour = value_to_i32(arg, heap)?;
                seen_hour = true;
            }
            4 => {
                minute = value_to_i32(arg, heap)?;
                seen_minute = true;
            }
            5 => {
                second = value_to_i32(arg, heap)?;
                seen_second = true;
            }
            6 => {
                microsecond = value_to_i32(arg, heap)?;
                seen_microsecond = true;
            }
            7 => {
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(arg, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            _ => return Err(ExcType::type_error_at_most("datetime", 8, index + 1)),
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let key_name = key_name.as_str(interns);
        match key_name {
            "year" => {
                if year.is_some() {
                    return Err(ExcType::type_error_multiple_values("datetime", "year"));
                }
                year = Some(value_to_i32(value, heap)?);
            }
            "month" => {
                if month.is_some() {
                    return Err(ExcType::type_error_multiple_values("datetime", "month"));
                }
                month = Some(value_to_i32(value, heap)?);
            }
            "day" => {
                if day.is_some() {
                    return Err(ExcType::type_error_multiple_values("datetime", "day"));
                }
                day = Some(value_to_i32(value, heap)?);
            }
            "hour" => {
                if seen_hour {
                    return Err(ExcType::type_error_multiple_values("datetime", "hour"));
                }
                hour = value_to_i32(value, heap)?;
                seen_hour = true;
            }
            "minute" => {
                if seen_minute {
                    return Err(ExcType::type_error_multiple_values("datetime", "minute"));
                }
                minute = value_to_i32(value, heap)?;
                seen_minute = true;
            }
            "second" => {
                if seen_second {
                    return Err(ExcType::type_error_multiple_values("datetime", "second"));
                }
                second = value_to_i32(value, heap)?;
                seen_second = true;
            }
            "microsecond" => {
                if seen_microsecond {
                    return Err(ExcType::type_error_multiple_values("datetime", "microsecond"));
                }
                microsecond = value_to_i32(value, heap)?;
                seen_microsecond = true;
            }
            "tzinfo" => {
                if seen_tzinfo {
                    return Err(ExcType::type_error_multiple_values("datetime", "tzinfo"));
                }
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(value, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            _ => return Err(ExcType::type_error_unexpected_keyword("datetime", key_name)),
        }
    }

    let Some(year) = year else {
        return Err(ExcType::type_error_missing_positional_with_names(
            "datetime",
            &["year", "month", "day"],
        ));
    };
    let Some(month) = month else {
        return Err(ExcType::type_error_missing_positional_with_names(
            "datetime",
            &["month", "day"],
        ));
    };
    let Some(day) = day else {
        return Err(ExcType::type_error_missing_positional_with_names("datetime", &["day"]));
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
pub(crate) fn class_now(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<AttrCallResult> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut tzinfo: Option<TimeZone> = None;
    let mut seen_tz = false;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => {
                let (value_tzinfo, _) = tzinfo_from_value(arg, heap)?;
                tzinfo = value_tzinfo;
                seen_tz = true;
            }
            _ => return Err(ExcType::type_error_at_most("datetime.now", 1, index + 1)),
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let key_name = key_name.as_str(interns);
        if key_name != "tz" {
            return Err(ExcType::type_error_unexpected_keyword("datetime.now", key_name));
        }
        if seen_tz {
            return Err(ExcType::type_error_multiple_values("datetime.now", "tz"));
        }
        let (value_tzinfo, _) = tzinfo_from_value(value, heap)?;
        tzinfo = value_tzinfo;
        seen_tz = true;
    }

    // Internal-only mode encoding:
    // 1 => datetime.now(tz=None), 2 => datetime.now(tz=<fixed offset>[, explicit name])
    let os_args = match tzinfo {
        Some(tz) => {
            let mode = Value::Int(DATETIME_NOW_FIXED_OFFSET_INTERNAL_MODE);
            let offset = Value::Int(i64::from(tz.offset_seconds));
            if let Some(name) = tz.name {
                let name = Value::Ref(heap.allocate(HeapData::Str(Str::new(name)))?);
                ArgValues::ArgsKargs {
                    args: vec![mode, offset, name],
                    kwargs: KwargsValues::Empty,
                }
            } else {
                ArgValues::Two(mode, offset)
            }
        }
        None => ArgValues::One(Value::Int(DATETIME_NOW_NAIVE_INTERNAL_MODE)),
    };
    Ok(AttrCallResult::OsCall(OsFunction::DateTimeNow, os_args))
}

/// `datetime + timedelta`
pub(crate) fn py_add(
    datetime: &DateTime,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
    _interns: &Interns,
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

fn value_to_i32(value: &Value, _heap: &Heap<impl ResourceTracker>) -> RunResult<i32> {
    let int_value = match value {
        Value::Bool(b) => i64::from(*b),
        Value::Int(i) => *i,
        _ => {
            return Err(
                SimpleException::new_msg(ExcType::TypeError, "an integer is required (got type float)").into(),
            );
        }
    };
    i32::try_from(int_value)
        .map_err(|_| SimpleException::new_msg(ExcType::OverflowError, "signed integer is greater than maximum").into())
}

fn tzinfo_from_value(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<(Option<TimeZone>, Option<HeapId>)> {
    match value {
        Value::None => Ok((None, None)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(tz) => Ok((Some(tz.clone()), Some(*id))),
            _other => Err(ExcType::type_error(
                "tzinfo argument must be datetime.timezone or None".to_owned(),
            )),
        },
        _ => Err(ExcType::type_error(
            "tzinfo argument must be datetime.timezone or None".to_owned(),
        )),
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
    let old = std::mem::replace(retained_tzinfo, Value::None);
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

fn from_local_unix_micros(local_unix_micros: i64) -> Option<DateTime> {
    let datetime = ChronoDateTime::<Utc>::from_timestamp_micros(local_unix_micros)?;
    from_local_naive(datetime.naive_utc())
}

fn from_utc_micros_with_timezone(utc_micros: i64, tzinfo: TimeZone) -> Option<DateTime> {
    let datetime = ChronoDateTime::<Utc>::from_timestamp_micros(utc_micros)?;
    from_utc_naive_with_timezone(datetime.naive_utc(), tzinfo)
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

fn from_utc_naive_with_timezone(utc_naive: NaiveDateTime, tzinfo: TimeZone) -> Option<DateTime> {
    from_utc_naive_with_timezone_parts(utc_naive, tzinfo.offset_seconds, tzinfo.name)
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

fn checked_offset_micros(offset_seconds: i32) -> Option<i64> {
    i64::from(offset_seconds).checked_mul(MICROS_PER_SECOND)
}

fn rounded_f64_to_i64(value: f64) -> i64 {
    value
        .to_i64()
        .expect("rounded timestamp should always fit i64 after explicit range check")
}

#[must_use]
fn year_in_python_range(year: i32) -> bool {
    (1..=9999).contains(&year)
}

impl<'h> PyTrait<'h> for DateTime {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::DateTime
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        if is_aware(self) != is_aware(other) {
            return Ok(false);
        }
        if is_aware(self) {
            return Ok(utc_micros(self) == utc_micros(other));
        }
        Ok(local_micros(self) == local_micros(other))
    }

    fn py_cmp(
        &self,
        other: &Self,
        _vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        if is_aware(self) != is_aware(other) {
            return Ok(None);
        }
        if is_aware(self) {
            return Ok(utc_micros(self).partial_cmp(&utc_micros(other)));
        }
        Ok(local_micros(self).partial_cmp(&local_micros(other)))
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(self) else {
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
        if let Some(tzinfo) = timezone_info(self) {
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

    fn py_str(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(self) else {
            return Ok(Cow::Borrowed("<out of range>"));
        };
        let mut s = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
        if microsecond != 0 {
            write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
        }
        if let Some(offset) = offset_seconds(self) {
            s.push_str(&timezone::format_offset_hms(offset));
        }
        Ok(Cow::Owned(s))
    }

    fn py_add(&self, other: &Self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let _ = other;
        Ok(None)
    }

    fn py_sub(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        if is_aware(self) != is_aware(other) {
            return Ok(None);
        }

        let diff = if is_aware(self) {
            let Some(lhs_utc) = to_utc_naive(self) else {
                return Ok(None);
            };
            let Some(rhs_utc) = to_utc_naive(other) else {
                return Ok(None);
            };
            lhs_utc.signed_duration_since(rhs_utc)
        } else {
            self.naive.signed_duration_since(other.naive)
        };

        let Ok(delta) = timedelta::from_chrono(diff) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?)))
    }

    fn py_getattr(
        &self,
        attr: &crate::value::EitherStr,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<CallResult>> {
        if attr.as_str(vm.interns) == "tzinfo" {
            if let Some(tzinfo_ref) = self.tzinfo_ref {
                vm.heap.inc_ref(tzinfo_ref);
                return Ok(Some(CallResult::Value(Value::Ref(tzinfo_ref))));
            }
            if let Some(tz) = timezone_info(self) {
                if tz.offset_seconds == 0 && tz.name.is_none() {
                    return Ok(Some(CallResult::Value(vm.heap.get_timezone_utc()?)));
                }
                return Ok(Some(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::TimeZone(tz))?,
                ))));
            }
            return Ok(Some(CallResult::Value(Value::None)));
        }
        Ok(None)
    }
}

impl HeapItem for DateTime {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.timezone_name.as_ref().map_or(0, String::len)
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
        Ok(self.get(vm.heap) == other.get(vm.heap))
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        self.get(vm.heap).py_repr_fmt(f, vm, heap_ids)
    }

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        self.get(vm.heap).py_str(vm)
    }

    fn py_getattr(
        &self,
        attr: &crate::value::EitherStr,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<CallResult>> {
        // Clone to release the HeapRead borrow before passing &mut VM
        let dt = self.get(vm.heap).clone();
        dt.py_getattr(attr, vm)
    }
}
