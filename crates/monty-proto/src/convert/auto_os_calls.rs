//! `AutoOsCalls` ↔ `pb::AutoOsCalls` conversions.
//!
//! Every unset wire arm means that field's default, so an empty message is
//! `AutoOsCalls::default()` and a parent that predates a field still gets the
//! behaviour it had. Rust → proto sets every arm; proto → Rust rejects a
//! microsecond past a second and a non-finite float seed.

use std::time::Duration;

use monty_types::{AutoOsCalls, DateTimeSource, RandomSeed, RandomStart, SandboxTimeZone, SleepMode};
use num_bigint::BigInt;

use crate::{
    convert::ProtoConvertError,
    pb::{
        self,
        auto_os_calls::{Datetime, RandomStart as WireRandomStart, SleepMode as WireSleepMode},
        random_seed::Value,
        sandbox_time_zone::Zone,
    },
};

impl From<&AutoOsCalls> for pb::AutoOsCalls {
    fn from(calls: &AutoOsCalls) -> Self {
        let datetime = match calls.datetime {
            DateTimeSource::CallHost => Datetime::CallHost(pb::Unit {}),
            DateTimeSource::System => Datetime::System(pb::Unit {}),
            DateTimeSource::Fixed {
                unix_seconds,
                microsecond,
            } => Datetime::Fixed(pb::FixedDateTime {
                unix_seconds,
                microsecond,
            }),
        };
        let zone = match &calls.timezone {
            SandboxTimeZone::CallHost => Zone::CallHost(pb::Unit {}),
            SandboxTimeZone::System => Zone::System(pb::Unit {}),
            SandboxTimeZone::Fixed { offset_seconds, name } => Zone::Fixed(pb::TimeZone {
                offset_seconds: *offset_seconds,
                name: name.clone(),
            }),
        };
        let random_start = match &calls.random_start {
            RandomStart::CallHost => WireRandomStart::RandomCallHost(pb::Unit {}),
            RandomStart::Random => WireRandomStart::Random(pb::Unit {}),
            RandomStart::Seed(seed) => WireRandomStart::Seed(seed.into()),
        };
        let sleep_mode = match calls.sleep {
            SleepMode::CallHost => WireSleepMode::SleepCallHost(pb::Unit {}),
            SleepMode::Zero => WireSleepMode::SleepZero(pb::Unit {}),
            SleepMode::SandboxSleep(clamp) => WireSleepMode::SandboxSleep(pb::SandboxSleep {
                clamp_micros: Some(u64::try_from(clamp.as_micros()).unwrap_or(u64::MAX)),
            }),
        };
        Self {
            datetime: Some(datetime),
            timezone: Some(pb::SandboxTimeZone { zone: Some(zone) }),
            sleep_mode: Some(sleep_mode),
            random_start: Some(random_start),
        }
    }
}

impl TryFrom<pb::AutoOsCalls> for AutoOsCalls {
    type Error = ProtoConvertError;

    fn try_from(calls: pb::AutoOsCalls) -> Result<Self, ProtoConvertError> {
        let defaults = Self::default();
        let datetime = match calls.datetime {
            None => defaults.datetime,
            Some(Datetime::CallHost(_)) => DateTimeSource::CallHost,
            Some(Datetime::System(_)) => DateTimeSource::System,
            Some(Datetime::Fixed(fixed)) => {
                if fixed.microsecond > 999_999 {
                    return Err(ProtoConvertError::InvalidValue {
                        field: "FixedDateTime.microsecond",
                        reason: format!("{} is not below 1000000", fixed.microsecond),
                    });
                }
                DateTimeSource::Fixed {
                    unix_seconds: fixed.unix_seconds,
                    microsecond: fixed.microsecond,
                }
            }
        };
        let timezone = match calls.timezone.and_then(|timezone| timezone.zone) {
            None => defaults.timezone,
            Some(Zone::CallHost(_)) => SandboxTimeZone::CallHost,
            Some(Zone::System(_)) => SandboxTimeZone::System,
            Some(Zone::Fixed(fixed)) => SandboxTimeZone::Fixed {
                offset_seconds: fixed.offset_seconds,
                name: fixed.name,
            },
        };
        let sleep = match calls.sleep_mode {
            None => defaults.sleep,
            Some(WireSleepMode::SleepCallHost(_)) => SleepMode::CallHost,
            Some(WireSleepMode::SleepZero(_)) => SleepMode::Zero,
            Some(WireSleepMode::SandboxSleep(sandbox)) => SleepMode::SandboxSleep(
                sandbox
                    .clamp_micros
                    .map_or(SleepMode::DEFAULT_CLAMP, Duration::from_micros),
            ),
        };
        let random_start = match calls.random_start {
            None => defaults.random_start,
            Some(WireRandomStart::RandomCallHost(_)) => RandomStart::CallHost,
            Some(WireRandomStart::Random(_)) => RandomStart::Random,
            Some(WireRandomStart::Seed(seed)) => RandomStart::Seed(seed.try_into()?),
        };
        Ok(Self {
            datetime,
            timezone,
            sleep,
            random_start,
        })
    }
}

impl From<&RandomSeed> for pb::RandomSeed {
    fn from(seed: &RandomSeed) -> Self {
        let value = match seed {
            RandomSeed::Int(n) => Value::Int(n.to_signed_bytes_le().into()),
            RandomSeed::Float(f) => Value::Float(*f),
            RandomSeed::Str(s) => Value::Str(s.clone()),
            RandomSeed::Bytes(b) => Value::Bytes(b.clone().into()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<pb::RandomSeed> for RandomSeed {
    type Error = ProtoConvertError;

    fn try_from(seed: pb::RandomSeed) -> Result<Self, ProtoConvertError> {
        match seed.value {
            None => Err(ProtoConvertError::MissingField("RandomSeed.value")),
            Some(Value::Int(bytes)) => Ok(Self::Int(BigInt::from_signed_bytes_le(&bytes))),
            Some(Value::Float(f)) if f.is_finite() => Ok(Self::Float(f)),
            Some(Value::Float(f)) => Err(ProtoConvertError::InvalidValue {
                field: "RandomSeed.float",
                reason: format!("{f} is not finite"),
            }),
            Some(Value::Str(s)) => Ok(Self::Str(s)),
            Some(Value::Bytes(b)) => Ok(Self::Bytes(b.into_inner())),
        }
    }
}
