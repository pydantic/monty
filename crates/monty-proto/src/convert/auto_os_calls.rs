//! `AutoOsCalls` ↔ `pb::AutoOsCalls` conversions.
//!
//! Every unset wire arm means that field's default, so an empty message is
//! `AutoOsCalls::default()` and a parent that predates a field still gets the
//! behaviour it had. Rust → proto sets every arm; proto → Rust rejects a
//! microsecond past a second, an unknown sleep mode (a sandbox-policy choice,
//! not a cosmetic one, so it is not silently defaulted) and a non-finite
//! float seed.

use std::time::Duration;

use monty_types::{AutoOsCalls, DateTimeSource, RandomSeed, RandomStart, SleepMode};
use num_bigint::BigInt;

use crate::{
    convert::ProtoConvertError,
    pb::{
        self,
        auto_os_calls::{Datetime, RandomStart as WireRandomStart},
        random_seed::Value,
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
                local_offset_seconds,
            } => Datetime::Fixed(pb::FixedDateTime {
                unix_seconds,
                microsecond,
                local_offset_seconds,
            }),
        };
        let random_start = match &calls.random_start {
            RandomStart::Random => WireRandomStart::Random(pb::Unit {}),
            RandomStart::Seed(seed) => WireRandomStart::Seed(seed.into()),
        };
        Self {
            datetime: Some(datetime),
            sleep: pb::SleepMode::from(calls.sleep).into(),
            sandbox_sleep_clamp_micros: Some(u64::try_from(calls.sandbox_sleep_clamp.as_micros()).unwrap_or(u64::MAX)),
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
                    local_offset_seconds: fixed.local_offset_seconds,
                }
            }
        };
        let sleep = pb::SleepMode::try_from(calls.sleep)
            .map_err(|_| ProtoConvertError::InvalidValue {
                field: "AutoOsCalls.sleep",
                reason: format!("unknown sleep mode {}", calls.sleep),
            })?
            .into();
        let random_start = match calls.random_start {
            None => defaults.random_start,
            Some(WireRandomStart::Random(_)) => RandomStart::Random,
            Some(WireRandomStart::Seed(seed)) => RandomStart::Seed(seed.try_into()?),
        };
        Ok(Self {
            datetime,
            sleep,
            sandbox_sleep_clamp: calls
                .sandbox_sleep_clamp_micros
                .map_or(defaults.sandbox_sleep_clamp, Duration::from_micros),
            random_start,
        })
    }
}

impl From<SleepMode> for pb::SleepMode {
    fn from(mode: SleepMode) -> Self {
        match mode {
            SleepMode::CallHost => Self::CallHost,
            SleepMode::Zero => Self::Zero,
            SleepMode::SandboxSleep => Self::SandboxSleep,
        }
    }
}

impl From<pb::SleepMode> for SleepMode {
    /// `Unspecified` is a parent that never set the field: the default.
    fn from(mode: pb::SleepMode) -> Self {
        match mode {
            pb::SleepMode::CallHost => Self::CallHost,
            pb::SleepMode::Zero => Self::Zero,
            pb::SleepMode::Unspecified | pb::SleepMode::SandboxSleep => Self::SandboxSleep,
        }
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
