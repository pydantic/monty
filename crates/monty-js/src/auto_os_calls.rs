//! Extraction of the session's `AutoOsCalls` — what the sandbox answers
//! itself: the clock, the sleeps and `random`'s first state — from the flat
//! fields the TypeScript `checkout()` wrapper puts on `NativeCheckoutOptions`.
//!
//! The wrapper has already validated and normalized the public options
//! (`ts/options.ts`), so this side only rejects what a wire type cannot
//! carry; each absent field is the worker's default.

use std::time::Duration;

use monty_types::{AutoOsCalls, DateTimeSource, RandomSeed, RandomStart, SandboxTimeZone, SleepMode};
use napi::{bindgen_prelude::BigInt, Error, Result, Status};
use num_bigint::BigInt as NumBigInt;

use crate::pool::NativeCheckoutOptions;

/// Builds the `AutoOsCalls` the flat option fields describe.
pub(crate) fn extract_auto_os_calls(options: &NativeCheckoutOptions) -> Result<AutoOsCalls> {
    let defaults = AutoOsCalls::default();
    let datetime = match options.datetime_kind.as_deref() {
        None => defaults.datetime,
        Some("system") => DateTimeSource::System,
        Some("call_host") => DateTimeSource::CallHost,
        Some("fixed") => {
            let unix_seconds = options
                .datetime_unix_seconds
                .as_ref()
                .map(bigint_to_i64)
                .transpose()?
                .ok_or_else(|| invalid("datetime: a fixed clock needs datetimeUnixSeconds"))?;
            let microsecond = options.datetime_microsecond.unwrap_or(0);
            if microsecond > 999_999 {
                return Err(invalid("datetime: microsecond must be below 1000000"));
            }
            DateTimeSource::Fixed {
                unix_seconds,
                microsecond,
            }
        }
        Some(other) => return Err(invalid(&format!("datetime: unknown source '{other}'"))),
    };
    let timezone = match options.timezone_kind.as_deref() {
        None => defaults.timezone,
        Some("system") => SandboxTimeZone::System,
        Some("call_host") => SandboxTimeZone::CallHost,
        Some("fixed") => SandboxTimeZone::Fixed {
            offset_seconds: options
                .timezone_offset_seconds
                .ok_or_else(|| invalid("timezone: a fixed zone needs timezoneOffsetSeconds"))?,
            name: options.timezone_name.clone(),
        },
        Some(other) => return Err(invalid(&format!("timezone: unknown zone '{other}'"))),
    };
    let clamp = match options.sandbox_sleep_clamp_secs {
        None => SleepMode::DEFAULT_CLAMP,
        Some(secs) if secs == f64::INFINITY => Duration::MAX,
        Some(secs) => Duration::try_from_secs_f64(secs).map_err(|err| invalid(&format!("sandboxSleepClamp: {err}")))?,
    };
    // The clamp only applies to a sandbox sleep; the other modes ignore it.
    let sleep = match options.sleep.as_deref() {
        None | Some("sandbox_sleep") => SleepMode::SandboxSleep(clamp),
        Some("zero") => SleepMode::Zero,
        Some("call_host") => SleepMode::CallHost,
        Some(other) => return Err(invalid(&format!("sleep: unknown mode '{other}'"))),
    };
    let random_start = match options.random_start_kind.as_deref() {
        None | Some("random") => RandomStart::Random,
        Some("call_host") => RandomStart::CallHost,
        Some("seed") => RandomStart::Seed(random_seed(options)?),
        Some(other) => return Err(invalid(&format!("randomStart: unknown start '{other}'"))),
    };
    Ok(AutoOsCalls {
        datetime,
        timezone,
        sleep,
        random_start,
    })
}

/// The seed from whichever one of the four typed fields is set.
fn random_seed(options: &NativeCheckoutOptions) -> Result<RandomSeed> {
    match (
        &options.random_seed_int,
        options.random_seed_float,
        &options.random_seed_str,
        &options.random_seed_bytes,
    ) {
        (Some(bytes), None, None, None) => Ok(RandomSeed::Int(NumBigInt::from_signed_bytes_le(bytes))),
        (None, Some(f), None, None) if f.is_finite() => Ok(RandomSeed::Float(f)),
        (None, Some(_), None, None) => Err(invalid("randomStart: a float seed must be finite")),
        (None, None, Some(s), None) => Ok(RandomSeed::Str(s.clone())),
        (None, None, None, Some(bytes)) => Ok(RandomSeed::Bytes(bytes.to_vec())),
        _ => Err(invalid("randomStart: a seed needs exactly one seed field")),
    }
}

/// A JS `bigint` as `i64`, rejecting one that does not fit.
fn bigint_to_i64(value: &BigInt) -> Result<i64> {
    let (n, lossless) = value.get_i64();
    if lossless {
        Ok(n)
    } else {
        Err(invalid("datetime: datetimeUnixSeconds does not fit in 64 bits"))
    }
}

/// An `InvalidArg` error naming the option.
fn invalid(message: &str) -> Error {
    Error::new(Status::InvalidArg, message.to_owned())
}
