//! [`AutoOsCalls`]: which OS calls the sandbox answers itself instead of
//! suspending to the host — the clock, the sleeps and `random`'s first state.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeDelta};
use num_bigint::BigInt;

/// Per-session choice of which OS calls the sandbox serves in-process.
///
/// Applies on every execution path, in-process runs and pool workers alike,
/// for the life of the session. Each field either names an in-sandbox answer
/// or `CallHost`, which suspends the call to the host as any other OS call
/// (under standard execution, where there is no host, such a call raises
/// `NotImplementedError`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutoOsCalls {
    /// What `date.today()`, `datetime.now()` and `time.time()` read.
    pub datetime: DateTimeSource,
    /// What `time.sleep()` and `asyncio.sleep()` do.
    pub sleep: SleepMode,
    /// Longest wait [`SleepMode::SandboxSleep`] performs for one call; a longer
    /// request is cut short, not refused.
    pub sandbox_sleep_clamp: Duration,
    /// Where an unseeded `random` generator gets its first state.
    pub random_start: RandomStart,
}

impl AutoOsCalls {
    /// [`sandbox_sleep_clamp`](Self::sandbox_sleep_clamp) unless a host says otherwise.
    pub const DEFAULT_SANDBOX_SLEEP_CLAMP: Duration = Duration::from_secs(10);
}

impl Default for AutoOsCalls {
    /// The system clock, sleeps performed in the sandbox for at most ten
    /// seconds each, and `random` seeded from OS entropy.
    fn default() -> Self {
        Self {
            datetime: DateTimeSource::System,
            sleep: SleepMode::SandboxSleep,
            sandbox_sleep_clamp: Self::DEFAULT_SANDBOX_SLEEP_CLAMP,
            random_start: RandomStart::Random,
        }
    }
}

/// Where the clock calls read the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DateTimeSource {
    /// Suspend to the host, which answers each call.
    CallHost,
    /// The process's own clock, in its local timezone.
    #[default]
    System,
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
        /// Offset of the clock's local timezone from UTC, in seconds. Naive
        /// `datetime.now()` and `date.today()` are read in this zone.
        local_offset_seconds: i32,
    },
}

impl DateTimeSource {
    /// Reads the clock. `None` is an unrepresentable [`Fixed`](Self::Fixed)
    /// instant; callers handle [`CallHost`](Self::CallHost) before calling.
    #[must_use]
    pub fn read(self) -> Option<DateTimeReading> {
        let reading = match self {
            Self::CallHost => return None,
            Self::System => {
                let now = Local::now();
                DateTimeReading {
                    utc: now.naive_utc(),
                    local_offset_seconds: now.offset().local_minus_utc(),
                }
            }
            Self::Fixed {
                unix_seconds,
                microsecond,
                local_offset_seconds,
            } => {
                // Kept under a full second here rather than left to
                // `from_timestamp`, which on the last second of a minute reads
                // anything above one as a leap second and accepts it, yielding a
                // `microsecond` no Python `datetime` can hold.
                let nanoseconds = microsecond.checked_mul(1_000).filter(|ns| *ns < 1_000_000_000)?;
                DateTimeReading {
                    utc: DateTime::from_timestamp(unix_seconds, nanoseconds)?.naive_utc(),
                    local_offset_seconds,
                }
            }
        };
        // All-or-nothing: an instant `datetime.now()` refuses is refused by
        // `time.time()` too.
        reading.local(0).map(|_| reading)
    }
}

/// One reading of a [`DateTimeSource`]: the UTC wall clock and the local
/// zone's offset, from which each clock call derives its own value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTimeReading {
    /// The instant, as a UTC wall clock.
    pub utc: NaiveDateTime,
    /// Offset of the local timezone from UTC, in seconds.
    pub local_offset_seconds: i32,
}

impl DateTimeReading {
    /// The wall clock in a zone `offset_seconds` from UTC, or `None` outside
    /// the 1..=9999 years Python's `datetime` can hold.
    #[must_use]
    pub fn local(&self, offset_seconds: i32) -> Option<NaiveDateTime> {
        let shifted = self
            .utc
            .checked_add_signed(TimeDelta::seconds(i64::from(offset_seconds)))?;
        (1..=9999).contains(&shifted.year()).then_some(shifted)
    }

    /// The instant as `time.time()` reports it: seconds since the Unix epoch.
    #[must_use]
    pub fn unix_seconds(&self) -> f64 {
        let epoch = self.utc.and_utc();
        epoch.timestamp() as f64 + f64::from(epoch.timestamp_subsec_micros()) / 1_000_000.0
    }
}

/// What the sleep calls do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SleepMode {
    /// Suspend to the host, which performs (or declines) the wait.
    CallHost,
    /// Return at once without waiting.
    Zero,
    /// Wait in the sandbox, each call cut to
    /// [`AutoOsCalls::sandbox_sleep_clamp`]. The wait is not execution time
    /// and not a suspension, so only the host's turn deadline bounds a loop
    /// of them.
    #[default]
    SandboxSleep,
}

/// Where an unseeded `random` generator gets its first state.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum RandomStart {
    /// From the sandbox's own OS entropy, never the host.
    #[default]
    Random,
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
