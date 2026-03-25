//! Datetime OS-call bridge helpers.
//!
//! This module owns the internal metadata protocol for `datetime.date.today()` and
//! `datetime.datetime.now(...)` when they are routed through `OsFunction::DateTimeNow`.
//! It keeps encoding/decoding and host payload conversion out of VM scheduler paths.

use crate::{
    MontyException,
    object::{MontyDate, MontyDateTime, MontyObject},
    os::OsFunction,
    types::{TimeZone, datetime},
};

/// Internal mode code used by `date.today()` in hidden `datetime.now` args.
pub(crate) const DATE_TODAY_INTERNAL_MODE: i64 = 0;
/// Internal mode code used by naive `datetime.now()` in hidden `datetime.now` args.
pub(crate) const DATETIME_NOW_NAIVE_INTERNAL_MODE: i64 = 1;
/// Internal mode code used by fixed-offset `datetime.now(tz=...)` in hidden args.
pub(crate) const DATETIME_NOW_FIXED_OFFSET_INTERNAL_MODE: i64 = 2;

/// Opaque OS-call metadata preserved when an OS call is resumed asynchronously.
///
/// Most OS calls do not need special host-payload conversion on resume. For
/// `datetime.now`, hidden args are carried here so future resolution can rebuild
/// the same Python-visible return shape as synchronous resume.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OsCallMetadata {
    /// The OS function this metadata belongs to.
    pub function: OsFunction,
    /// Hidden positional args (if any) for internal conversion semantics.
    pub args: Vec<MontyObject>,
    /// Hidden keyword args (if any) for internal conversion semantics.
    pub kwargs: Vec<(MontyObject, MontyObject)>,
}

/// Internal resume mode encoded in hidden `datetime.now` callback metadata.
#[derive(Debug, Clone)]
pub(crate) enum DateTimeNowResumeMode {
    /// Resume as `datetime.date.today()`.
    Today,
    /// Resume as naive `datetime.datetime.now()`.
    Naive,
    /// Resume as fixed-offset `datetime.datetime.now(tz=...)`.
    FixedOffset {
        /// Fixed UTC offset in seconds for the target timezone.
        offset_seconds: i32,
        /// Optional explicit timezone name.
        timezone_name: Option<String>,
    },
}

/// Decodes hidden internal args for `OsFunction::DateTimeNow`.
///
/// This validates and decodes the metadata protocol emitted by datetime type
/// classmethods. It is intentionally strict because malformed metadata indicates
/// an internal bug, not user input error.
pub(crate) fn decode_datetime_now_internal_args(
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
) -> Result<DateTimeNowResumeMode, MontyException> {
    if !kwargs.is_empty() {
        return Err(MontyException::runtime_error(
            "internal datetime.now metadata should not contain keyword arguments",
        ));
    }
    let [mode_obj, rest @ ..] = args else {
        return Err(MontyException::runtime_error(
            "internal datetime.now metadata missing mode argument",
        ));
    };
    let MontyObject::Int(mode) = mode_obj else {
        return Err(MontyException::runtime_error(
            "internal datetime.now mode argument must be an integer",
        ));
    };

    match *mode {
        DATE_TODAY_INTERNAL_MODE => {
            if rest.is_empty() {
                Ok(DateTimeNowResumeMode::Today)
            } else {
                Err(MontyException::runtime_error(
                    "internal datetime.now date.today mode should not include extra arguments",
                ))
            }
        }
        DATETIME_NOW_NAIVE_INTERNAL_MODE => {
            if rest.is_empty() {
                Ok(DateTimeNowResumeMode::Naive)
            } else {
                Err(MontyException::runtime_error(
                    "internal datetime.now naive mode should not include extra arguments",
                ))
            }
        }
        DATETIME_NOW_FIXED_OFFSET_INTERNAL_MODE => {
            let (offset_obj, timezone_name_obj) = match rest {
                [offset] => (offset, None),
                [offset, name] => (offset, Some(name)),
                _ => {
                    return Err(MontyException::runtime_error(
                        "internal datetime.now fixed-offset mode requires one or two arguments",
                    ));
                }
            };
            let MontyObject::Int(offset_raw) = offset_obj else {
                return Err(MontyException::runtime_error(
                    "internal datetime.now offset argument must be an integer",
                ));
            };
            let offset_seconds = i32::try_from(*offset_raw)
                .map_err(|_| MontyException::runtime_error("internal datetime.now offset argument must fit in i32"))?;
            let timezone_name = match timezone_name_obj {
                None => None,
                Some(MontyObject::String(name)) => Some(name.clone()),
                Some(_) => {
                    return Err(MontyException::runtime_error(
                        "internal datetime.now timezone name must be a string",
                    ));
                }
            };
            Ok(DateTimeNowResumeMode::FixedOffset {
                offset_seconds,
                timezone_name,
            })
        }
        _ => Err(MontyException::runtime_error(format!(
            "internal datetime.now mode {mode} is unsupported"
        ))),
    }
}

/// Applies any OS-call-specific return-value conversion before VM resume.
///
/// For most OS calls this returns the payload unchanged. `datetime.now` uses
/// hidden metadata to convert `(timestamp_utc, local_offset_seconds)` into
/// `MontyObject::Date` or `MontyObject::DateTime`.
pub(crate) fn convert_os_call_result(
    metadata: &OsCallMetadata,
    payload: MontyObject,
) -> Result<MontyObject, MontyException> {
    match metadata.function {
        OsFunction::DateTimeNow => {
            let mode = decode_datetime_now_internal_args(&metadata.args, &metadata.kwargs)?;
            convert_datetime_now_callback_result(&mode, payload)
        }
        _ => Ok(payload),
    }
}

/// Converts a raw `datetime.now` callback payload into the API-specific value.
///
/// The host callback always returns `(timestamp_utc_seconds: float,
/// local_offset_seconds: int)`. The hidden resume mode determines whether this
/// becomes `date.today()`, naive `datetime.now()`, or fixed-offset aware
/// `datetime.now(tz=...)`.
fn convert_datetime_now_callback_result(
    mode: &DateTimeNowResumeMode,
    payload: MontyObject,
) -> Result<MontyObject, MontyException> {
    let MontyObject::Tuple(values) = payload else {
        return Err(invalid_datetime_now_return_type(
            "datetime.now callback must return a 2-tuple",
        ));
    };
    let [timestamp_obj, local_offset_obj] = values.as_slice() else {
        return Err(invalid_datetime_now_return_type(
            "datetime.now callback must return exactly two values",
        ));
    };

    let timestamp_utc = match timestamp_obj {
        MontyObject::Float(value) => *value,
        _ => {
            return Err(invalid_datetime_now_return_type(
                "datetime.now timestamp must be a float",
            ));
        }
    };
    if !timestamp_utc.is_finite() {
        return Err(invalid_datetime_now_return_type(
            "datetime.now timestamp must be finite",
        ));
    }

    let local_offset_seconds = match local_offset_obj {
        MontyObject::Int(value) => i32::try_from(*value).map_err(|_| {
            invalid_datetime_now_return_type("datetime.now local offset must be an integer fitting i32")
        })?,
        _ => {
            return Err(invalid_datetime_now_return_type(
                "datetime.now local offset must be an integer fitting i32",
            ));
        }
    };

    let tzinfo = match mode {
        DateTimeNowResumeMode::Today | DateTimeNowResumeMode::Naive => None,
        DateTimeNowResumeMode::FixedOffset {
            offset_seconds,
            timezone_name,
        } => Some(
            TimeZone::new(*offset_seconds, timezone_name.clone())
                .map_err(|_| MontyException::runtime_error("internal datetime.now fixed offset is out of range"))?,
        ),
    };

    let datetime = datetime::from_now_payload(timestamp_utc, local_offset_seconds, tzinfo)
        .map_err(|_| invalid_datetime_now_return_type("datetime.now payload produced out-of-range datetime"))?;
    let Some((year, month, day, hour, minute, second, microsecond)) = datetime::to_components(&datetime) else {
        return Err(invalid_datetime_now_return_type(
            "datetime.now payload produced out-of-range datetime",
        ));
    };

    match mode {
        DateTimeNowResumeMode::Today => Ok(MontyObject::Date(MontyDate { year, month, day })),
        DateTimeNowResumeMode::Naive | DateTimeNowResumeMode::FixedOffset { .. } => {
            let tzinfo = datetime::timezone_info(&datetime);
            Ok(MontyObject::DateTime(MontyDateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                microsecond,
                offset_seconds: datetime::offset_seconds(&datetime),
                timezone_name: tzinfo.and_then(|tz| tz.name),
            }))
        }
    }
}

/// Constructs the standardized host API error for invalid `datetime.now` payloads.
fn invalid_datetime_now_return_type(msg: &'static str) -> MontyException {
    MontyException::runtime_error(format!("invalid return type: {msg}"))
}
