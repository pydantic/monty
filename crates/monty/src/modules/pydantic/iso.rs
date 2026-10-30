//! ISO 8601 text for temporal snapshots, as pydantic's JSON mode writes them.
//!
//! pydantic-core formats through `speedate`'s `Display` impls: a `Z` suffix for
//! a zero offset, `+HH:MM` otherwise, fractional seconds only when the
//! microseconds are non-zero, and durations as `P1DT1H0.0005S`.

use std::fmt::Write;

use monty_types::{MontyDate, MontyDateTime, MontyTime, MontyTimeDelta};

/// `YYYY-MM-DD`.
pub(crate) fn date(date: &MontyDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]`.
pub(crate) fn datetime(dt: &MontyDateTime) -> String {
    let mut out = format!("{:04}-{:02}-{:02}T", dt.year, dt.month, dt.day);
    clock(
        &mut out,
        dt.hour,
        dt.minute,
        dt.second,
        dt.microsecond,
        dt.offset_seconds,
    );
    out
}

/// `HH:MM:SS[.ffffff][Z|±HH:MM]`.
pub(crate) fn time(time: &MontyTime) -> String {
    let mut out = String::new();
    clock(
        &mut out,
        time.hour,
        time.minute,
        time.second,
        time.microsecond,
        time.offset_seconds,
    );
    out
}

/// The time of day and offset shared by [`datetime`] and [`time`].
fn clock(out: &mut String, hour: u8, minute: u8, second: u8, microsecond: u32, offset_seconds: Option<i32>) {
    write!(out, "{hour:02}:{minute:02}:{second:02}").expect("writing to String cannot fail");
    if microsecond != 0 {
        write!(out, ".{microsecond:06}").expect("writing to String cannot fail");
    }
    match offset_seconds {
        Some(0) => out.push('Z'),
        Some(offset) => {
            let sign = if offset < 0 { '-' } else { '+' };
            let total_minutes = offset.unsigned_abs() / 60;
            write!(out, "{sign}{:02}:{:02}", total_minutes / 60, total_minutes % 60)
                .expect("writing to String cannot fail");
        }
        None => {}
    }
}

/// `speedate`'s duration form: `[-]P[nY][nD][T[nH][nM][n[.f]S]]`, `PT0S` for zero.
pub(crate) fn timedelta(delta: &MontyTimeDelta) -> String {
    let total_micros =
        i64::from(delta.days) * 86_400_000_000 + i64::from(delta.seconds) * 1_000_000 + i64::from(delta.microseconds);
    let positive = total_micros >= 0;
    let total_micros = total_micros.unsigned_abs();
    let micros = total_micros % 1_000_000;
    let total_seconds = total_micros / 1_000_000;
    let days = total_seconds / 86_400;
    let seconds = total_seconds % 86_400;
    let mut out = String::new();
    if !positive {
        out.push('-');
    }
    out.push('P');
    if days != 0 {
        let years = days / 365;
        if years != 0 {
            write!(out, "{years}Y").expect("writing to String cannot fail");
        }
        let days = days % 365;
        if days != 0 {
            write!(out, "{days}D").expect("writing to String cannot fail");
        }
    }
    if seconds != 0 || micros != 0 {
        out.push('T');
        let (hours, minutes, secs) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
        if hours != 0 {
            write!(out, "{hours}H").expect("writing to String cannot fail");
        }
        if minutes != 0 {
            write!(out, "{minutes}M").expect("writing to String cannot fail");
        }
        if secs != 0 || micros != 0 {
            write!(out, "{secs}").expect("writing to String cannot fail");
            if micros != 0 {
                let fraction = format!("{micros:06}");
                write!(out, ".{}", fraction.trim_end_matches('0')).expect("writing to String cannot fail");
            }
            out.push('S');
        }
    }
    if seconds == 0 && micros == 0 && days == 0 {
        out.push_str("T0S");
    }
    out
}
