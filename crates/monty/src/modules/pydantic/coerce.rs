//! Lax coercions shared by the Python and JSON inputs: text to numbers and
//! booleans, and text or timestamps to temporal values, with pydantic-core's
//! rules (`input/shared.rs` and `input/datetime.rs`) and error types.
//!
//! Each helper returns the catalogue error type on failure rather than a
//! `ValError`, since only the caller holds the input to attach it to.

use num_bigint::BigInt;
use speedate::{
    Date as SpeedDate, DateConfig, DateTime as SpeedDateTime, DateTimeConfig, Duration,
    MicrosecondsPrecisionOverflowBehavior, ParseError, Time as SpeedTime, TimeConfig,
};
use strum::EnumMessage;

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{Heap, HeapData},
    modules::pydantic::catalogue::ErrorType,
    types::{LongInt, date as date_type, datetime as datetime_type, time as time_type, timedelta as timedelta_type},
    value::Value,
};

/// `'true'`, `'1'`, `'yes'`, `'on'`, `'t'`, `'y'` and their negatives, case-insensitively.
pub(crate) fn str_as_bool(text: &str) -> Result<bool, ErrorType> {
    if text == "0"
        || text.eq_ignore_ascii_case("f")
        || text.eq_ignore_ascii_case("n")
        || text.eq_ignore_ascii_case("no")
        || text.eq_ignore_ascii_case("off")
        || text.eq_ignore_ascii_case("false")
    {
        Ok(false)
    } else if text == "1"
        || text.eq_ignore_ascii_case("t")
        || text.eq_ignore_ascii_case("y")
        || text.eq_ignore_ascii_case("on")
        || text.eq_ignore_ascii_case("yes")
        || text.eq_ignore_ascii_case("true")
    {
        Ok(true)
    } else {
        Err(ErrorType::BoolParsing {})
    }
}

/// Only `0` and `1` are booleans.
pub(crate) fn int_as_bool(int: i64) -> Result<bool, ErrorType> {
    match int {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ErrorType::BoolParsing {}),
    }
}

/// An integral float as an `int`; a fraction, infinity or NaN is refused.
#[expect(clippy::cast_possible_truncation, reason = "the range check keeps the cast exact")]
pub(crate) fn float_as_int(float: f64) -> Result<i64, ErrorType> {
    if float.is_infinite() || float.is_nan() {
        Err(ErrorType::FiniteNumber {})
    } else if float % 1.0 != 0.0 {
        Err(ErrorType::IntFromFloat {})
    } else if (i64::MIN as f64) < float && float < (i64::MAX as f64) {
        Ok(float as i64)
    } else {
        Err(ErrorType::IntParsingSize {})
    }
}

/// Text as an `int`: plain digits, then pydantic's cleaned form (surrounding
/// whitespace, a sign, leading zeros, `1_000` separators, a `.0` fraction).
pub(crate) fn str_as_int(text: &str, heap: &Heap) -> Result<Value, ErrorType> {
    match parse_digits(text) {
        Ok(value) => return Ok(int_value(value, heap)),
        Err(DigitsError::TooLong) => return Err(ErrorType::IntParsingSize {}),
        Err(DigitsError::NotDigits) => {}
    }
    match clean_int_str(text) {
        Some(cleaned) => match parse_digits(&cleaned) {
            Ok(value) => Ok(int_value(value, heap)),
            Err(_) => Err(ErrorType::IntParsing {}),
        },
        None => Err(ErrorType::IntParsing {}),
    }
}

/// Text as a `float`: Rust's parser after trimming, then with `_` separators removed.
pub(crate) fn str_as_float(text: &str) -> Result<f64, ErrorType> {
    match text.trim().parse::<f64>() {
        Ok(float) => Ok(float),
        Err(_) => match strip_underscores(text).and_then(|stripped| stripped.parse().ok()) {
            Some(float) => Ok(float),
            None => Err(ErrorType::FloatParsing {}),
        },
    }
}

/// A parsed integer, big when it overflows `i64`.
enum Digits {
    Small(i64),
    Big(BigInt),
}

enum DigitsError {
    NotDigits,
    /// A longer number than pydantic-core's `jiter` parser accepts (4300 bytes).
    TooLong,
}

/// Parses an optionally negative run of ASCII digits, as `jiter`'s
/// `NumberInt::try_from` does: a leading zero is refused first, then a number
/// over 4300 bytes (sign included), then anything after the digits.
fn parse_digits(text: &str) -> Result<Digits, DigitsError> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let run = digits.bytes().take_while(u8::is_ascii_digit).count();
    if run == 0 || (run > 1 && digits.starts_with('0')) {
        return Err(DigitsError::NotDigits);
    }
    if text.len() - digits.len() + run > 4300 {
        return Err(DigitsError::TooLong);
    }
    if run != digits.len() {
        return Err(DigitsError::NotDigits);
    }
    match text.parse::<i64>() {
        Ok(value) => Ok(Digits::Small(value)),
        Err(_) => text
            .parse::<BigInt>()
            .map(Digits::Big)
            .map_err(|_| DigitsError::NotDigits),
    }
}

fn int_value(digits: Digits, heap: &Heap) -> Value {
    match digits {
        Digits::Small(value) => Value::Int(value),
        Digits::Big(value) => bigint_value(value, heap),
    }
}

/// A big integer as a value, small when it fits `i64`.
pub(crate) fn bigint_value(value: BigInt, heap: &Heap) -> Value {
    match i64::try_from(&value) {
        Ok(small) => Value::Int(small),
        Err(_) => Value::Ref(heap.allocate(HeapData::LongInt(LongInt::new(value)))),
    }
}

/// pydantic-core's `strip_underscores`: `1_000` becomes `1000`; a leading,
/// trailing or doubled underscore disqualifies the text.
fn strip_underscores(text: &str) -> Option<String> {
    if text.starts_with('_') || text.ends_with('_') || !text.contains('_') || text.contains("__") {
        None
    } else {
        Some(text.replace('_', ""))
    }
}

/// pydantic-core's `clean_int_str`: the digits of an integer written with
/// surrounding whitespace, a sign, leading zeros, underscores or a `.000`
/// fraction, or `None` when the text is not such an integer.
fn clean_int_str(text: &str) -> Option<String> {
    let len_before = text.len();
    let mut s = text.trim();
    if let Some(suffix) = s.strip_prefix('+') {
        if suffix.starts_with('-') {
            return None;
        }
        s = suffix;
    }
    let mut is_negative = false;
    if let Some(suffix) = s.strip_prefix('-') {
        if suffix.starts_with('-') || suffix.starts_with('+') {
            return None;
        }
        is_negative = true;
        s = suffix;
    }
    s = strip_leading_zeros(s)?;
    if let Some(i) = s.find('.') {
        let decimal_part = &s[i + 1..];
        if !decimal_part.is_empty() && decimal_part.chars().all(|c| c == '0') {
            s = &s[..i];
        }
    }
    let sign = if is_negative { "-" } else { "" };
    if let Some(stripped) = strip_underscores(s) {
        return Some(format!("{sign}{stripped}"));
    }
    if len_before == s.len() {
        return None;
    }
    Some(format!("{sign}{s}"))
}

/// Drops leading zeros (and the underscores among them), keeping one zero for `0`.
fn strip_leading_zeros(s: &str) -> Option<&str> {
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, '0')) => (),
        Some((_, c)) if ('1'..='9').contains(&c) || c == '-' => return Some(s),
        _ => return None,
    }
    for (i, c) in chars {
        match c {
            '0' | '_' => (),
            '1'..='9' | '-' => return Some(&s[i..]),
            '.' => return Some(&s[(i - 1)..]),
            _ => return None,
        }
    }
    Some(&s[s.len() - 1..])
}

/// `speedate`'s wording for a failed parse, as pydantic's messages quote it.
fn parse_error_text(err: &ParseError) -> String {
    err.get_documentation().unwrap_or_default().to_owned()
}

/// Microseconds past six digits are truncated, pydantic's default.
fn time_config() -> TimeConfig {
    TimeConfig {
        microseconds_precision_overflow_behavior: MicrosecondsPrecisionOverflowBehavior::Truncate,
        unix_timestamp_offset: Some(0),
    }
}

fn datetime_config() -> DateTimeConfig {
    DateTimeConfig {
        timestamp_unit: speedate::TimestampUnit::default(),
        time_config: time_config(),
    }
}

/// Text as a `date`.
pub(crate) fn bytes_as_date(bytes: &[u8], heap: &Heap) -> Result<Value, ErrorType> {
    match SpeedDate::parse_bytes_with_config(bytes, &DateConfig::builder().build()) {
        Ok(date) => date_value(date, heap),
        Err(err) => Err(ErrorType::DateParsing {
            error: parse_error_text(&err),
        }),
    }
}

/// Text as a parsed `time`, for [`time_value`] once the text's borrow is released:
/// building the value needs the VM, which the text may be borrowed from.
pub(crate) fn parse_time(bytes: &[u8]) -> Result<SpeedTime, ErrorType> {
    SpeedTime::parse_bytes_with_config(bytes, &time_config()).map_err(|err| ErrorType::TimeParsing {
        error: parse_error_text(&err),
    })
}

/// Text as a parsed `datetime`, for [`datetime_value`]; see [`parse_time`].
pub(crate) fn parse_datetime(bytes: &[u8]) -> Result<SpeedDateTime, ErrorType> {
    SpeedDateTime::parse_bytes_with_config(bytes, &datetime_config()).map_err(|err| ErrorType::DatetimeParsing {
        error: parse_error_text(&err),
    })
}

/// Text as a `timedelta`: ISO 8601 durations and `HH:MM:SS` forms.
///
/// Like the other duration coercions, the outer error is the `OverflowError`
/// pydantic raises (not reports) for a duration `timedelta` cannot hold.
pub(crate) fn bytes_as_timedelta(bytes: &[u8], heap: &Heap) -> RunResult<Result<Value, ErrorType>> {
    match Duration::parse_bytes_with_config(bytes, &time_config()) {
        Ok(duration) => duration_value(&duration, heap).map(Ok),
        Err(err) => Ok(Err(ErrorType::TimeDeltaParsing {
            error: parse_error_text(&err),
        })),
    }
}

/// A Unix timestamp (seconds, or milliseconds past `speedate`'s watershed) as a UTC `datetime`.
pub(crate) fn int_as_datetime(timestamp: i64, microseconds: u32, vm: &mut VM<'_>) -> Result<Value, ErrorType> {
    match SpeedDateTime::from_timestamp_with_config(timestamp, microseconds, &datetime_config()) {
        Ok(dt) => datetime_value(&dt, vm),
        Err(err) => Err(ErrorType::DatetimeParsing {
            error: parse_error_text(&err),
        }),
    }
}

pub(crate) fn float_as_datetime(timestamp: f64, vm: &mut VM<'_>) -> Result<Value, ErrorType> {
    if timestamp.is_nan() {
        return Err(ErrorType::DatetimeParsing {
            error: "NaN values not permitted".to_owned(),
        });
    }
    match SpeedDateTime::from_float_with_config(timestamp, &datetime_config()) {
        Ok(dt) => datetime_value(&dt, vm),
        Err(err) => Err(ErrorType::DatetimeParsing {
            error: parse_error_text(&err),
        }),
    }
}

/// Seconds since midnight as a `time`.
pub(crate) fn int_as_time(timestamp: i64, microseconds: u32, vm: &mut VM<'_>) -> Result<Value, ErrorType> {
    let seconds = match timestamp {
        t if t < 0 => {
            return Err(ErrorType::TimeParsing {
                error: "time in seconds should be positive".to_owned(),
            });
        }
        t => u32::try_from(t).unwrap_or(u32::MAX),
    };
    match SpeedTime::from_timestamp_with_config(seconds, microseconds, &time_config()) {
        Ok(time) => Ok(time_value(&time, vm)),
        Err(err) => Err(ErrorType::TimeParsing {
            error: parse_error_text(&err),
        }),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is non-negative and below a million; `int_as_time` checks the floor's sign"
)]
pub(crate) fn float_as_time(timestamp: f64, vm: &mut VM<'_>) -> Result<Value, ErrorType> {
    if timestamp.is_nan() {
        return Err(ErrorType::TimeParsing {
            error: "NaN values not permitted".to_owned(),
        });
    }
    let microseconds = timestamp.fract().abs() * 1_000_000.0;
    int_as_time(timestamp.floor() as i64, microseconds.round() as u32, vm)
}

/// Seconds as a `timedelta`; see [`bytes_as_timedelta`] for the two errors.
pub(crate) fn int_as_duration(total_seconds: i64, heap: &Heap) -> RunResult<Result<Value, ErrorType>> {
    let positive = total_seconds >= 0;
    let total_seconds = total_seconds.unsigned_abs();
    let Ok(days) = u32::try_from(total_seconds / 86400) else {
        return Ok(Err(ErrorType::TimeDeltaParsing {
            error: parse_error_text(&ParseError::DurationDaysTooLarge),
        }));
    };
    let seconds = (total_seconds % 86400) as u32;
    checked_duration_value(Duration::new(positive, days, seconds, 0), heap)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the parts are split from the absolute value, as pydantic-core does"
)]
pub(crate) fn float_as_duration(total_seconds: f64, heap: &Heap) -> RunResult<Result<Value, ErrorType>> {
    if total_seconds.is_nan() {
        return Ok(Err(ErrorType::TimeDeltaParsing {
            error: "NaN values not permitted".to_owned(),
        }));
    }
    let positive = total_seconds >= 0.0;
    let total_seconds = total_seconds.abs();
    let microsecond = total_seconds.fract() * 1_000_000.0;
    let days = (total_seconds / 86400.0) as u32;
    let seconds = total_seconds as u64 % 86400;
    checked_duration_value(
        Duration::new(positive, days, seconds as u32, microsecond.round() as u32),
        heap,
    )
}

/// A `speedate` duration, or its construction error, as a `timedelta`.
fn checked_duration_value(duration: Result<Duration, ParseError>, heap: &Heap) -> RunResult<Result<Value, ErrorType>> {
    match duration {
        Ok(duration) => duration_value(&duration, heap).map(Ok),
        Err(err) => Ok(Err(ErrorType::TimeDeltaParsing {
            error: parse_error_text(&err),
        })),
    }
}

/// A `speedate` date as a `datetime.date`; year 0 has no Python form.
fn date_value(date: SpeedDate, heap: &Heap) -> Result<Value, ErrorType> {
    date_type::from_ymd(i32::from(date.year), i32::from(date.month), i32::from(date.day))
        .map(|date| Value::Ref(heap.allocate(HeapData::Date(date))))
        .map_err(|_| ErrorType::DateParsing {
            error: YEAR_OUT_OF_RANGE.to_owned(),
        })
}

/// The wording for a date `speedate` accepts and Python's calendar does not.
const YEAR_OUT_OF_RANGE: &str = "year is out of range";

/// A `speedate` time as a `datetime.time`, with a fixed-offset zone when it has one.
pub(crate) fn time_value(time: &SpeedTime, vm: &mut VM<'_>) -> Value {
    time_type::allocate_parsed(
        i32::from(time.hour),
        i32::from(time.minute),
        i32::from(time.second),
        i32::try_from(time.microsecond).expect("microseconds are below a million"),
        time.tz_offset,
        vm,
    )
}

/// A `speedate` datetime as a `datetime.datetime`, aware when it carries an offset.
pub(crate) fn datetime_value(dt: &SpeedDateTime, vm: &mut VM<'_>) -> Result<Value, ErrorType> {
    datetime_type::allocate_parsed(
        i32::from(dt.date.year),
        i32::from(dt.date.month),
        i32::from(dt.date.day),
        i32::from(dt.time.hour),
        i32::from(dt.time.minute),
        i32::from(dt.time.second),
        i32::try_from(dt.time.microsecond).expect("microseconds are below a million"),
        dt.time.tz_offset,
        vm.heap,
    )
    .ok_or_else(|| ErrorType::DatetimeParsing {
        error: YEAR_OUT_OF_RANGE.to_owned(),
    })
}

/// A `speedate` duration as a `datetime.timedelta`. `speedate` bounds the
/// magnitude's days, but a negative one can still normalise past
/// `timedelta.min` (`-P999999999DT1S`), which raises `OverflowError` as in pydantic.
fn duration_value(duration: &Duration, heap: &Heap) -> RunResult<Value> {
    let micros = i128::from(duration.day) * 86_400_000_000
        + i128::from(duration.second) * 1_000_000
        + i128::from(duration.microsecond);
    let delta = timedelta_type::from_total_microseconds(if duration.positive { micros } else { -micros })?;
    Ok(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))))
}

/// A `date` value as the naive `datetime` at its midnight, as a lax `datetime`
/// reads a date; `None` for any other value.
pub(crate) fn date_at_midnight(value: &Value, vm: &mut VM<'_>) -> Option<Value> {
    let Value::Ref(id) = value else { return None };
    let HeapData::Date(date) = vm.heap.get(*id) else {
        return None;
    };
    let (year, month, day) = date_type::to_ymd(*date);
    let (month, day) = (i32::try_from(month), i32::try_from(day));
    let (month, day) = (month.expect("month is 1..=12"), day.expect("day is 1..=31"));
    let midnight = datetime_type::allocate_parsed(year, month, day, 0, 0, 0, 0, None, vm.heap);
    Some(midnight.expect("a date's components are in range"))
}

/// Whether a `datetime` value is at midnight with no sub-second part, for
/// `date` validation of a datetime input.
pub(crate) fn datetime_is_midnight(value: &Value, vm: &VM<'_>) -> bool {
    let Value::Ref(id) = value else { return false };
    let HeapData::DateTime(dt) = vm.heap.get(*id) else {
        return false;
    };
    datetime_type::to_components(dt)
        .is_some_and(|(_, _, _, hour, minute, second, micro)| hour == 0 && minute == 0 && second == 0 && micro == 0)
}
