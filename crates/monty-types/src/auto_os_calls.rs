//! Session policies for clocks, sleeps and initial random state.

use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDateTime, TimeDelta, Utc};
use num_bigint::BigInt;

/// Policies for clocks, sleeps and initial random state on every execution path.
/// `CallHost` suspends to the host, or raises `NotImplementedError` without one.
/// Defaults use the system clock, the UTC zone, OS entropy, and sleeps capped at
/// ten seconds. Hosts perform those sleeps without their `os` handler; standard
/// execution waits inline.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AutoOsCalls {
    /// The instant `date.today()`, `datetime.now()` and `time.time()` read.
    pub datetime: DateTimeSource,
    /// The local zone: naive `datetime.now()` and `date.today()` read it,
    /// `astimezone()`, `time.timezone`/`tzname` and `%Z` report it.
    pub timezone: SandboxTimeZone,
    /// What `time.sleep()` and `asyncio.sleep()` do.
    pub sleep: SleepMode,
    /// Where an unseeded `random` generator gets its first state.
    pub random_start: RandomStart,
}

/// Where the clock calls read the instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DateTimeSource {
    /// The process's own clock.
    #[default]
    System,
    /// Suspend to the host, which answers each call.
    CallHost,
    /// Every call reads this instant. Years outside 1..=9999 raise `OverflowError`.
    Fixed {
        /// Seconds since the Unix epoch, UTC.
        unix_seconds: i64,
        /// Sub-second component, 0..=999_999. Anything larger raises.
        microsecond: u32,
    },
}

impl DateTimeSource {
    /// Reads the instant in UTC. Returns `None` for [`CallHost`](Self::CallHost)
    /// or a [`Fixed`](Self::Fixed) instant Python's `datetime` cannot represent.
    #[must_use]
    pub fn read(self) -> Option<NaiveDateTime> {
        let utc = match self {
            Self::System => Utc::now().naive_utc(),
            Self::CallHost => return None,
            Self::Fixed {
                unix_seconds,
                microsecond,
            } => {
                // Chrono accepts leap seconds; Python requires microseconds below 1_000_000.
                let nanoseconds = microsecond.checked_mul(1_000).filter(|ns| *ns < 1_000_000_000)?;
                DateTime::from_timestamp(unix_seconds, nanoseconds)?.naive_utc()
            }
        };
        // Apply datetime's year range to time.time() too.
        local_wall_clock(utc, 0).map(|_| utc)
    }
}

/// The sandbox's local zone. UTC unless configured; the host's own zone is
/// never read, so nothing about the host leaks through the clock.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SandboxTimeZone {
    /// Suspend the calls that need the zone to the host, which answers them.
    CallHost,
    /// A fixed offset from UTC, with the name `datetime.timezone(offset, name)`
    /// would carry. Not an IANA zone: there are no DST rules in the sandbox.
    Fixed {
        /// Offset from UTC, in seconds.
        offset_seconds: i32,
        /// The zone's name, if it has one.
        name: Option<String>,
    },
}

impl Default for SandboxTimeZone {
    fn default() -> Self {
        Self::utc()
    }
}

impl SandboxTimeZone {
    /// UTC named `UTC`, as CPython reports the zone under `TZ=UTC`.
    #[must_use]
    pub fn utc() -> Self {
        Self::Fixed {
            offset_seconds: 0,
            name: Some("UTC".to_owned()),
        }
    }

    /// The zone's offset from UTC in seconds; `None` is [`CallHost`](Self::CallHost).
    #[must_use]
    pub fn offset_seconds(&self) -> Option<i32> {
        match self {
            Self::CallHost => None,
            Self::Fixed { offset_seconds, .. } => Some(*offset_seconds),
        }
    }

    /// The configured zone name; `None` for [`CallHost`](Self::CallHost) or an unnamed offset.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::CallHost => None,
            Self::Fixed { name, .. } => name.as_deref(),
        }
    }
}

/// The wall clock `offset_seconds` from UTC at `utc`, or `None` outside the
/// 1..=9999 years Python's `datetime` can hold.
#[must_use]
pub fn local_wall_clock(utc: NaiveDateTime, offset_seconds: i32) -> Option<NaiveDateTime> {
    let shifted = utc.checked_add_signed(TimeDelta::seconds(i64::from(offset_seconds)))?;
    (1..=9999).contains(&shifted.year()).then_some(shifted)
}

/// The instant as `time.time()` reports it: seconds since the Unix epoch.
#[must_use]
pub fn unix_seconds(utc: NaiveDateTime) -> f64 {
    let epoch = utc.and_utc();
    epoch.timestamp() as f64 + f64::from(epoch.timestamp_subsec_micros()) / 1_000_000.0
}

/// What the sleep calls do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SleepMode {
    /// Cap each delay at this duration, then suspend to the host to wait without
    /// its `os` handler. The host charges `ResourceLimits::max_total_sleep` before
    /// waiting; waits do not count as execution time. Standard execution waits
    /// inline without a cumulative limit. Longer delays are capped, not rejected.
    System(Duration),
    /// Delegate to the host's `os` handler without capping or charging the delay.
    CallHost,
    /// Return at once without waiting.
    Zero,
}

impl SleepMode {
    /// Default per-call cap for [`System`](Self::System).
    pub const DEFAULT_MAX: Duration = Duration::from_secs(10);
}

impl Default for SleepMode {
    fn default() -> Self {
        Self::System(Self::DEFAULT_MAX)
    }
}

/// Where an unseeded `random` generator gets its first state.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum RandomStart {
    /// From the sandbox's own OS entropy.
    #[default]
    System,
    /// Request one state vector (2496 bytes) from the host via `os.urandom` on the first draw.
    CallHost,
    /// The module-level generator starts exactly as `random.seed(seed)` leaves
    /// it; unseeded `random.Random()` instances take deterministic states
    /// derived from the same seed.
    Seed(RandomSeed),
}

/// Explicit seeds for [`RandomStart::Seed`], using CPython's `random.seed()` semantics.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RandomSeed {
    /// Any size; CPython seeds from the absolute value.
    Int(BigInt),
    /// Seeded from the float's CPython `hash()`.
    Float(f64),
    /// Seeded from the SHA-512-extended text, as `seed(str)` does.
    Str(String),
    /// Seeded from the SHA-512-extended bytes, as `seed(bytes)` does.
    Bytes(Vec<u8>),
}
