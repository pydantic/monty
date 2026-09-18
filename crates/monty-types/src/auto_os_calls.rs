//! [`AutoOsCalls`]: which OS calls the sandbox answers itself instead of
//! suspending to the host — the clock, the sleeps and `random`'s first state.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeDelta, TimeZone};
use num_bigint::BigInt;

/// Per-session choice of which OS calls the sandbox serves in-process, on
/// every execution path. Each field names an in-sandbox answer or `CallHost`,
/// which suspends the call to the host as any other OS call (with no host,
/// `NotImplementedError`). The default answers everything in the sandbox:
/// system clock and zone, sleeps of at most ten seconds, entropy-seeded `random`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AutoOsCalls {
    /// The instant `date.today()`, `datetime.now()` and `time.time()` read.
    pub datetime: DateTimeSource,
    /// The local zone naive `datetime.now()` and `date.today()` read that
    /// instant in.
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
    /// A frozen instant, for runs that have to be reproducible: every call
    /// reads the same time.
    ///
    /// An instant outside `datetime`'s 1..=9999 year range raises
    /// `OverflowError` in the sandbox rather than failing some other way.
    Fixed {
        /// Seconds since the Unix epoch, UTC.
        unix_seconds: i64,
        /// Sub-second component, 0..=999_999. Anything larger raises.
        microsecond: u32,
    },
}

impl DateTimeSource {
    /// Reads the instant as a UTC wall clock. `None` is an unrepresentable
    /// [`Fixed`](Self::Fixed) instant; callers handle
    /// [`CallHost`](Self::CallHost) before calling.
    #[must_use]
    pub fn read(self) -> Option<NaiveDateTime> {
        let utc = match self {
            Self::System => Local::now().naive_utc(),
            Self::CallHost => return None,
            Self::Fixed {
                unix_seconds,
                microsecond,
            } => {
                // Kept under a full second here rather than left to
                // `from_timestamp`, which on the last second of a minute reads
                // anything above one as a leap second and accepts it, yielding a
                // `microsecond` no Python `datetime` can hold.
                let nanoseconds = microsecond.checked_mul(1_000).filter(|ns| *ns < 1_000_000_000)?;
                DateTime::from_timestamp(unix_seconds, nanoseconds)?.naive_utc()
            }
        };
        // All-or-nothing: an instant `datetime.now()` refuses is refused by
        // `time.time()` too.
        local_wall_clock(utc, 0).map(|_| utc)
    }
}

/// The local zone naive `datetime.now()` and `date.today()` read in, and
/// what `astimezone()`, `time.tzname` and `%Z` will report once implemented.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SandboxTimeZone {
    /// The process's own local zone, at its offset for the instant read.
    #[default]
    System,
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

impl SandboxTimeZone {
    /// The zone's offset from UTC at `utc`, in seconds; `None` is
    /// [`CallHost`](Self::CallHost).
    #[must_use]
    pub fn offset_seconds(&self, utc: NaiveDateTime) -> Option<i32> {
        match self {
            Self::System => Some(Local.offset_from_utc_datetime(&utc).local_minus_utc()),
            Self::CallHost => None,
            Self::Fixed { offset_seconds, .. } => Some(*offset_seconds),
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
    /// Wait in the sandbox, each call cut to the maximum given (a longer
    /// request is cut short, not refused). The wait is not execution time;
    /// `ResourceLimits::max_total_sleep` bounds the sum of them.
    System(Duration),
    /// Suspend to the host, which performs (or declines) the wait.
    CallHost,
    /// Return at once without waiting.
    Zero,
}

impl SleepMode {
    /// The maximum [`System`](Self::System) starts with.
    pub const DEFAULT_MAX: Duration = Duration::from_secs(10);
}

impl Default for SleepMode {
    /// Sleeps performed in the sandbox for at most [`DEFAULT_MAX`](Self::DEFAULT_MAX) each.
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
    /// Suspend the first draw with an `os.urandom` call for one state vector
    /// (2496 bytes), which the host answers.
    CallHost,
    /// The module-level generator starts exactly as `random.seed(seed)` leaves
    /// it; unseeded `random.Random()` instances take deterministic states
    /// derived from the same seed.
    Seed(RandomSeed),
}

/// A seed for [`RandomStart::Seed`]: the types CPython's `random.seed()`
/// accepts, seeded the way it seeds them.
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
