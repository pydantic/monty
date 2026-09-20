//! Session policies for clocks, sleeps and initial random state.

use std::{error::Error, fmt, time::Duration};

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike, Utc};
use jiff::{
    Timestamp, civil,
    tz::{AmbiguousOffset, TimeZone},
};
use num_bigint::BigInt;

use crate::object::MontyTimeZone;

/// Policies for clocks, sleeps and initial random state on every execution path.
/// `CallHost` suspends to the host, or raises `NotImplementedError` without one;
/// the zone is always resolved in the sandbox.
/// Defaults use the system clock, the UTC zone, OS entropy, a zero process clock,
/// and sleeps capped at ten seconds. Hosts perform those sleeps without their
/// `os` handler; standard execution waits inline.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct OsPolicy {
    /// The instant `date.today()`, `datetime.now()` and `time.time()` read.
    pub datetime: DateTimeSource,
    /// The local zone: naive `datetime.now()` and `date.today()` read it,
    /// `astimezone()`, `time.timezone`/`tzname` and `%Z` report it.
    pub timezone: SandboxTimeZone,
    /// What `time.sleep()` and `asyncio.sleep()` do.
    pub sleep: SleepMode,
    /// What `time.process_time()` and `time.thread_time()` report.
    pub process_time: ProcessTime,
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

/// The sandbox's local zone, UTC unless configured.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SandboxTimeZone {
    /// A fixed offset from UTC, with the name `datetime.timezone(offset, name)`
    /// would carry. No DST: every instant has the same offset and name.
    Fixed {
        /// Offset from UTC, in seconds.
        offset_seconds: i32,
        /// The zone's name, if it has one.
        name: Option<String>,
    },
    /// An IANA zone with its transition rules, built only by [`named`](Self::named).
    /// Serialises as its name, so each side of the wire resolves it against its
    /// own database.
    Named(NamedZone),
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

    /// Resolves an IANA zone name such as `Europe/London` against the tz database
    /// the `tzdb` or `tzdb-bundled` feature provides. Without either, or for a
    /// name the database lacks, returns [`UnknownTimeZone`].
    pub fn named(name: &str) -> Result<Self, UnknownTimeZone> {
        NamedZone::resolve(name).map(Self::Named)
    }

    /// The IANA name of a [`Named`](Self::Named) zone; `None` for a fixed offset.
    #[must_use]
    pub fn iana_name(&self) -> Option<&str> {
        match self {
            Self::Fixed { .. } => None,
            Self::Named(zone) => Some(zone.name()),
        }
    }

    /// The offset and name in force at the UTC instant `utc`: the `timezone`
    /// that `astimezone()` attaches and `%Z` prints. A fixed zone's name is
    /// `None` when it was not given one.
    #[must_use]
    pub fn at(&self, utc: NaiveDateTime) -> MontyTimeZone {
        match self {
            Self::Fixed { offset_seconds, name } => MontyTimeZone {
                offset_seconds: *offset_seconds,
                name: name.clone(),
            },
            Self::Named(zone) => {
                let info = zone.zone.to_offset_info(timestamp(utc));
                MontyTimeZone {
                    offset_seconds: info.offset().seconds(),
                    name: Some(info.abbreviation().to_owned()),
                }
            }
        }
    }

    /// The offset in force for a naive wall-clock time in this zone. An ambiguous
    /// time (a DST fold) takes its first occurrence and a skipped time (a gap) the
    /// offset from before it, CPython's `fold=0` reading. `None` only outside the
    /// civil year range, which is wider than `datetime`'s.
    #[must_use]
    pub fn offset_for_local(&self, local: NaiveDateTime) -> Option<i32> {
        match self {
            Self::Fixed { offset_seconds, .. } => Some(*offset_seconds),
            // The offset rather than the instant: jiff's timestamps stop short of the
            // last day a `datetime` holds, and `timestamp()` reaches past it anyway.
            Self::Named(zone) => match zone.zone.to_ambiguous_timestamp(civil_datetime(local)?).offset() {
                AmbiguousOffset::Unambiguous { offset }
                | AmbiguousOffset::Gap { before: offset, .. }
                | AmbiguousOffset::Fold { before: offset, .. } => Some(offset.seconds()),
            },
        }
    }

    /// The UTC instant a naive wall-clock time in this zone denotes, or `None`
    /// outside years 1..=9999.
    #[must_use]
    pub fn utc_from_local(&self, local: NaiveDateTime) -> Option<NaiveDateTime> {
        local_wall_clock(local, self.offset_for_local(local)?.checked_neg()?)
    }

    /// The `time` module's `timezone`, `altzone`, `daylight` and `tzname`, read
    /// as CPython does from the zone's state on 1 January and 1 July of `year`.
    /// A fixed zone ignores `year`; a named one needs it, so `None` gives `None`.
    #[must_use]
    pub fn constants(&self, year: Option<i32>) -> Option<ZoneConstants> {
        let (january, july) = match self {
            Self::Fixed { .. } => {
                let zone = self.at(DateTime::UNIX_EPOCH.naive_utc());
                (zone.clone(), zone)
            }
            Self::Named(_) => {
                let year = year?;
                let at = |month| Some(self.at(NaiveDate::from_ymd_opt(year, month, 1)?.and_time(NaiveTime::MIN)));
                (at(1)?, at(7)?)
            }
        };
        // `time.timezone` is seconds *west* of UTC; standard time is the one
        // further west, which swaps the halves in the southern hemisphere.
        let (standard, daylight) = if january.offset_seconds < july.offset_seconds {
            (january, july)
        } else {
            (july, january)
        };
        Some(ZoneConstants {
            daylight: standard.offset_seconds != daylight.offset_seconds,
            standard,
            daylight_zone: daylight,
        })
    }
}

/// A zone resolved from the tz database by [`SandboxTimeZone::named`], which is
/// the only way to build one: every value carries the database's name for it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NamedZone {
    name: String,
    zone: TimeZone,
}

impl NamedZone {
    /// The zone's IANA name as the database spells it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    fn resolve(name: &str) -> Result<Self, UnknownTimeZone> {
        // The chars IANA keys use, with no `.`/`..` components, so nothing
        // traversal-shaped reaches the database's path lookup.
        let valid_part = |part: &str| {
            !matches!(part, "" | "." | "..") && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        };
        let unknown = || UnknownTimeZone(name.to_owned());
        if name.len() > 100 || !name.split('/').all(valid_part) {
            return Err(unknown());
        }
        let zone = TimeZone::get(name).map_err(|_| unknown())?;
        // `Etc/Unknown` resolves to jiff's nameless placeholder zone.
        let name = zone.iana_name().ok_or_else(unknown)?.to_owned();
        Ok(Self { name, zone })
    }
}

impl TryFrom<String> for NamedZone {
    type Error = UnknownTimeZone;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::resolve(&name)
    }
}

impl From<NamedZone> for String {
    fn from(zone: NamedZone) -> Self {
        zone.name
    }
}

/// A zone name [`SandboxTimeZone::named`] could not resolve: malformed, or
/// absent from the tz database available to this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTimeZone(pub String);

impl fmt::Display for UnknownTimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown timezone '{}'", self.0)
    }
}

impl Error for UnknownTimeZone {}

/// The zone's standard and daylight halves as the `time` module reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneConstants {
    /// Standard time: `-offset_seconds` is `time.timezone`, `name` is `tzname[0]`.
    pub standard: MontyTimeZone,
    /// Daylight time, equal to `standard` when the zone has none: `time.altzone`, `tzname[1]`.
    pub daylight_zone: MontyTimeZone,
    /// `time.daylight`: whether the two halves differ.
    pub daylight: bool,
}

/// `utc` as a jiff instant, clamped to jiff's range: its timestamps end about a
/// day before `datetime`'s do, so that every offset still renders as a civil
/// datetime. No zone changes offset on 31 December, so the clamp reports the
/// same offset and name the instant itself would.
fn timestamp(utc: NaiveDateTime) -> Timestamp {
    let utc = utc.and_utc();
    let seconds = utc.timestamp();
    let nanoseconds = i32::try_from(utc.timestamp_subsec_nanos()).unwrap_or(0);
    Timestamp::new(seconds, nanoseconds).unwrap_or(if seconds < 0 { Timestamp::MIN } else { Timestamp::MAX })
}

/// `local` as a jiff civil datetime, `None` outside jiff's year range.
fn civil_datetime(local: NaiveDateTime) -> Option<civil::DateTime> {
    civil::DateTime::new(
        i16::try_from(local.year()).ok()?,
        i8::try_from(local.month()).ok()?,
        i8::try_from(local.day()).ok()?,
        i8::try_from(local.hour()).ok()?,
        i8::try_from(local.minute()).ok()?,
        i8::try_from(local.second()).ok()?,
        i32::try_from(local.nanosecond()).ok()?,
    )
    .ok()
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

/// What `time.process_time()` and `time.thread_time()` report.
///
/// Separate from [`DateTimeSource`] because these clocks exclude sleeps and
/// time suspended on the host, which no wall clock can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ProcessTime {
    /// Always zero, so a [`DateTimeSource::Fixed`] session stays blind to time passing.
    #[default]
    Zero,
    /// The session's accumulated execution time, as `ResourceTracker::elapsed`
    /// measures it. Wall time while running, not CPU time.
    Elapsed,
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
