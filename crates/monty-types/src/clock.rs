//! The host clock capability: [`HostClock`], which answers `date.today()` and
//! `datetime.now()` in-process instead of suspending to the host for them.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeDelta, Timelike};

use crate::{
    object::{MontyDate, MontyDateTime, MontyObject},
    os::OsFunctionCall,
};

/// Where `date.today()` and `datetime.now()` read the time under standard
/// (non-suspending) execution, which has no host to deliver their
/// [`OsFunctionCall`] to.
///
/// Only standard execution consults it — under suspend/resume the host answers
/// both calls and a clock set here is ignored, so this is absent from the wire
/// protocol. Deliberately not [`Default`]: a fresh runner gets
/// [`System`](Self::System) (see `monty`'s `default_clock`), which a type-level
/// [`Denied`](Self::Denied) would quietly contradict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HostClock {
    /// No clock: both calls raise `NotImplementedError`, as every other OS
    /// call does under standard execution.
    Denied,
    /// The process's own clock, in its local timezone.
    System,
    /// A frozen instant, for runs that have to be reproducible.
    ///
    /// An instant outside `datetime`'s 1..=9999 year range reads as
    /// [`Denied`](Self::Denied) rather than failing some other way.
    Fixed {
        /// Seconds since the Unix epoch, UTC.
        unix_seconds: i64,
        /// Sub-second component, 0..=999_999. Anything larger reads as
        /// [`Denied`](Self::Denied).
        microsecond: u32,
        /// Offset of the clock's local timezone from UTC, in seconds. Naive
        /// `datetime.now()` and `date.today()` are read in this zone.
        local_offset_seconds: i32,
    },
}

impl HostClock {
    /// Answers `call` if it is a clock call this clock can serve.
    ///
    /// `None` means the caller should treat the call as unserviced — the clock
    /// is [`Denied`](Self::Denied), the call is not a clock call, or the
    /// instant is unrepresentable as a Python `datetime`.
    ///
    /// The clock is read per arm rather than up front so that a host passing
    /// every OS call through here — as the CLI does — pays nothing for the
    /// filesystem calls, which are the overwhelming majority.
    #[must_use]
    pub fn resolve(self, call: &OsFunctionCall) -> Option<MontyObject> {
        match call {
            OsFunctionCall::DateToday => {
                let (utc, local_offset_seconds) = self.instant()?;
                let local = shift(utc, local_offset_seconds)?;
                Some(MontyObject::Date(MontyDate {
                    year: local.year(),
                    month: u8::try_from(local.month()).ok()?,
                    day: u8::try_from(local.day()).ok()?,
                }))
            }
            // Naive `now()` is local wall clock; `now(tz)` is the same instant
            // read in `tz`, both matching CPython.
            OsFunctionCall::DateTimeNow(tz) => {
                let (utc, local_offset_seconds) = self.instant()?;
                let offset_seconds = tz.as_ref().map_or(local_offset_seconds, |tz| tz.offset_seconds);
                let local = shift(utc, offset_seconds)?;
                Some(MontyObject::DateTime(MontyDateTime {
                    year: local.year(),
                    month: u8::try_from(local.month()).ok()?,
                    day: u8::try_from(local.day()).ok()?,
                    hour: u8::try_from(local.hour()).ok()?,
                    minute: u8::try_from(local.minute()).ok()?,
                    second: u8::try_from(local.second()).ok()?,
                    microsecond: local.nanosecond() / 1_000,
                    offset_seconds: tz.as_ref().map(|tz| tz.offset_seconds),
                    timezone_name: tz.as_ref().and_then(|tz| tz.name.clone()),
                }))
            }
            _ => None,
        }
    }

    /// This clock's instant as `(UTC wall clock, local offset in seconds)`.
    ///
    /// Both variants reduce to the same pair so the two calls above share one
    /// conversion; `None` is [`Denied`](Self::Denied) or an unrepresentable
    /// fixed instant.
    fn instant(self) -> Option<(NaiveDateTime, i32)> {
        match self {
            Self::Denied => None,
            Self::System => {
                let now = Local::now();
                Some((now.naive_utc(), now.offset().local_minus_utc()))
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
                let utc = DateTime::from_timestamp(unix_seconds, nanoseconds)?;
                Some((utc.naive_utc(), local_offset_seconds))
            }
        }
    }
}

/// Shifts a UTC wall clock by `offset_seconds`, rejecting anything
/// outside the 1..=9999 years Python's `datetime` can hold.
fn shift(utc: NaiveDateTime, offset_seconds: i32) -> Option<NaiveDateTime> {
    let shifted = utc.checked_add_signed(TimeDelta::seconds(i64::from(offset_seconds)))?;
    (1..=9999).contains(&shifted.year()).then_some(shifted)
}
