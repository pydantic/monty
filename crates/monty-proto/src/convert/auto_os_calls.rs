//! `AutoOsCalls` ↔ `pb::AutoOsCalls` conversions.
//!
//! Unset wire arms use field defaults for compatibility with older parents.
//! Encoding sets every arm; decoding validates microseconds, zone offsets and seeds.

use std::time::Duration;

use monty_types::{
    AutoOsCalls, DateTimeSource, MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS, ProcessTime, RandomSeed,
    RandomStart, SandboxTimeZone, SleepMode,
};
use num_bigint::BigInt;

use crate::{
    convert::ProtoConvertError,
    pb::{
        self,
        auto_os_calls::{Datetime, ProcessTime as WireProcessTime, RandomStart as WireRandomStart},
        random_seed::Value,
        sandbox_time_zone::Zone,
        sleep_mode::Mode,
    },
};

impl From<&AutoOsCalls> for pb::AutoOsCalls {
    fn from(calls: &AutoOsCalls) -> Self {
        let datetime = match calls.datetime {
            DateTimeSource::System => Datetime::System(pb::Unit {}),
            DateTimeSource::CallHost => Datetime::CallHost(pb::Unit {}),
            DateTimeSource::Fixed {
                unix_seconds,
                microsecond,
            } => Datetime::Fixed(pb::FixedDateTime {
                unix_seconds,
                microsecond,
            }),
        };
        let zone = match &calls.timezone {
            zone if *zone == SandboxTimeZone::utc() => Zone::Utc(pb::Unit {}),
            SandboxTimeZone::Named(zone) => Zone::Named(zone.name().to_owned()),
            SandboxTimeZone::Fixed { offset_seconds, name } => Zone::Fixed(pb::TimeZone {
                offset_seconds: *offset_seconds,
                name: name.clone(),
            }),
        };
        let mode = match calls.sleep {
            SleepMode::System(max) => Mode::System(pb::SystemSleep {
                max_micros: Some(u64::try_from(max.as_micros()).unwrap_or(u64::MAX)),
            }),
            SleepMode::CallHost => Mode::CallHost(pb::Unit {}),
            SleepMode::Zero => Mode::Zero(pb::Unit {}),
        };
        let random_start = match &calls.random_start {
            RandomStart::System => WireRandomStart::RandomSystem(pb::Unit {}),
            RandomStart::CallHost => WireRandomStart::RandomCallHost(pb::Unit {}),
            RandomStart::Seed(seed) => WireRandomStart::Seed(seed.into()),
        };
        let process_time = match calls.process_time {
            ProcessTime::Zero => WireProcessTime::Zero(pb::Unit {}),
            ProcessTime::Elapsed => WireProcessTime::Elapsed(pb::Unit {}),
        };
        Self {
            datetime: Some(datetime),
            timezone: Some(pb::SandboxTimeZone { zone: Some(zone) }),
            sleep: Some(pb::SleepMode { mode: Some(mode) }),
            random_start: Some(random_start),
            process_time: Some(process_time),
        }
    }
}

impl TryFrom<pb::AutoOsCalls> for AutoOsCalls {
    type Error = ProtoConvertError;

    fn try_from(calls: pb::AutoOsCalls) -> Result<Self, ProtoConvertError> {
        let defaults = Self::default();
        let datetime = match calls.datetime {
            None => defaults.datetime,
            Some(Datetime::System(_)) => DateTimeSource::System,
            Some(Datetime::CallHost(_)) => DateTimeSource::CallHost,
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
            Some(Zone::Utc(_)) => SandboxTimeZone::utc(),
            Some(Zone::Named(name)) => {
                SandboxTimeZone::named(&name).map_err(|err| ProtoConvertError::InvalidValue {
                    field: "SandboxTimeZone.named",
                    reason: err.to_string(),
                })?
            }
            Some(Zone::Fixed(fixed)) => {
                if !(MIN_TIMEZONE_OFFSET_SECONDS..=MAX_TIMEZONE_OFFSET_SECONDS).contains(&fixed.offset_seconds) {
                    return Err(ProtoConvertError::InvalidValue {
                        field: "TimeZone.offset_seconds",
                        reason: format!(
                            "{} is outside the range {MIN_TIMEZONE_OFFSET_SECONDS}..={MAX_TIMEZONE_OFFSET_SECONDS}",
                            fixed.offset_seconds
                        ),
                    });
                }
                SandboxTimeZone::Fixed {
                    offset_seconds: fixed.offset_seconds,
                    name: fixed.name,
                }
            }
        };
        let sleep = match calls.sleep.and_then(|sleep| sleep.mode) {
            None => defaults.sleep,
            Some(Mode::System(system)) => {
                SleepMode::System(system.max_micros.map_or(SleepMode::DEFAULT_MAX, Duration::from_micros))
            }
            Some(Mode::CallHost(_)) => SleepMode::CallHost,
            Some(Mode::Zero(_)) => SleepMode::Zero,
        };
        let random_start = match calls.random_start {
            None => defaults.random_start,
            Some(WireRandomStart::RandomSystem(_)) => RandomStart::System,
            Some(WireRandomStart::RandomCallHost(_)) => RandomStart::CallHost,
            Some(WireRandomStart::Seed(seed)) => RandomStart::Seed(seed.try_into()?),
        };
        let process_time = match calls.process_time {
            None => defaults.process_time,
            Some(WireProcessTime::Zero(_)) => ProcessTime::Zero,
            Some(WireProcessTime::Elapsed(_)) => ProcessTime::Elapsed,
        };
        Ok(Self {
            datetime,
            timezone,
            sleep,
            process_time,
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
