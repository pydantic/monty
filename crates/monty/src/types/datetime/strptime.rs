//! Python `strptime` parsing without Jiff's hour and leap-second normalization.

use jiff::{
    civil::{Date as CivilDate, DateTime as CivilDateTime, Time as CivilTime},
    fmt::strtime::BrokenDownTime,
};
use monty_types::OsPolicy;

use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    types::{date, str::StringRepr, timezone::tzname_string},
};

/// Parsed clock fields, retaining seconds 60 and 61 for `time.struct_time`.
/// `wall` caps those seconds at 59; datetime callers must reject them instead.
pub(crate) struct ParsedTime {
    pub wall: CivilDateTime,
    pub second: i8,
    pub offset_seconds: Option<i32>,
}

/// Parses with Python's defaults, zone names and trailing-input diagnostics.
pub(crate) fn parse(input: &str, format: &str, policy: &OsPolicy) -> RunResult<ParsedTime> {
    let mismatch = || format_mismatch(input, format);
    let expanded = expand_format(input, format)?;
    let zones = if expanded.contains("%Z") {
        policy
            .timezone
            .constants(policy.datetime.read().map(|utc| i32::from(utc.year())))
    } else {
        None
    };
    let names = zones.map(|zones| {
        [zones.standard, zones.daylight_zone].map(|zone| tzname_string(zone.offset_seconds, zone.name.as_deref()))
    });
    let matched = match_fields(input, &expanded, names.as_ref()).ok_or_else(mismatch)?;
    if !matched.remaining.is_empty() {
        return Err(ExcType::value_error(format_args!(
            "unconverted data remains: {}",
            matched.remaining
        )));
    }
    if let Some(offset) = matched.inconsistent_offset {
        return Err(ExcType::value_error(format_args!("Inconsistent use of : in {offset}")));
    }
    let parsed = BrokenDownTime::parse(&matched.format, &matched.input).map_err(|_| mismatch())?;
    let hour = match matched.hour {
        Some(ClockHour::TwentyFour(hour)) => hour,
        Some(ClockHour::Twelve(hour)) => hour % 12 + if matched.pm { 12 } else { 0 },
        None => parsed.hour().unwrap_or(0),
    };
    let second = matched.second.or(parsed.second()).unwrap_or(0);
    let time = CivilTime::new(
        hour,
        parsed.minute().unwrap_or(0),
        second.min(59),
        parsed.subsec_nanosecond().unwrap_or(0),
    )
    .expect("parser validates clock components");
    Ok(ParsedTime {
        wall: parsed_date(parsed).ok_or_else(mismatch)??.to_datetime(time),
        second,
        offset_seconds: matched.offset_seconds,
    })
}

/// A format match with normalized literals in place of independently parsed fields.
#[derive(Default)]
struct MatchedFields<'a> {
    format: String,
    input: String,
    remaining: &'a str,
    hour: Option<ClockHour>,
    pm: bool,
    second: Option<i8>,
    offset_seconds: Option<i32>,
    inconsistent_offset: Option<&'a str>,
}

/// The last hour directive wins; only a 12-hour value reads the meridiem.
enum ClockHour {
    TwentyFour(i8),
    Twelve(i8),
}

/// Matches raw clock and zone fields before Jiff can discard their original values.
/// Placeholders keep neighboring fields and whitespace separate in the final parse.
fn match_fields<'a>(mut input: &'a str, mut format: &str, zones: Option<&[String; 2]>) -> Option<MatchedFields<'a>> {
    let mut matched = MatchedFields::default();
    while let Some((before, field, after)) = split_field(format) {
        let (_, consumed) = BrokenDownTime::parse_prefix(before, input).ok()?;
        matched.format.push_str(before);
        matched.input.push_str(&input[..consumed]);
        input = &input[consumed..];
        let consumed = match field {
            'H' | 'I' | 'S' => {
                let (low, high) = match field {
                    'H' => (0, 23),
                    'I' => (1, 12),
                    _ => (0, 61),
                };
                let (value, len) = clock_number(input, low, high)?;
                match field {
                    'H' => matched.hour = Some(ClockHour::TwentyFour(value)),
                    'I' => matched.hour = Some(ClockHour::Twelve(value)),
                    _ => matched.second = Some(value),
                }
                len
            }
            'p' => {
                let meridiem = input.get(..2)?;
                if !meridiem.eq_ignore_ascii_case("am") && !meridiem.eq_ignore_ascii_case("pm") {
                    return None;
                }
                matched.pm = meridiem.eq_ignore_ascii_case("pm");
                2
            }
            'Z' => zone_length(input, zones)?,
            'z' => {
                let token = offset_at(input)?;
                matched.offset_seconds = Some(token.offset_seconds);
                if !token.colons_agree {
                    matched.inconsistent_offset = Some(&input[..token.len]);
                }
                token.len
            }
            _ => unreachable!("split_field only returns supported clock and zone fields"),
        };
        matched.format.push('@');
        matched.input.push('@');
        input = &input[consumed..];
        format = after;
    }
    let (_, consumed) = BrokenDownTime::parse_prefix(format, input).ok()?;
    matched.format.push_str(format);
    matched.input.push_str(&input[..consumed]);
    matched.remaining = &input[consumed..];
    Some(matched)
}

/// Finds the next clock or zone directive, skipping escaped percents.
fn split_field(format: &str) -> Option<(&str, char, &str)> {
    let mut chars = format.char_indices();
    while let Some((at, c)) = chars.next() {
        if c == '%' {
            let (_, field) = chars.next()?;
            if matches!(field, 'H' | 'I' | 'S' | 'p' | 'z' | 'Z') {
                return Some((&format[..at], field, &format[at + 2..]));
            }
        }
    }
    None
}

/// Matches Python's one- or two-digit clock fields, preferring a valid pair.
fn clock_number(input: &str, low: i8, high: i8) -> Option<(i8, usize)> {
    let first = i8::try_from(input.as_bytes().first()?.checked_sub(b'0')?).ok()?;
    if first > 9 {
        return None;
    }
    if let Some(second) = input.as_bytes().get(1).filter(|c| c.is_ascii_digit()) {
        let value = first * 10 + i8::try_from(second - b'0').ok()?;
        if (low..=high).contains(&value) {
            return Some((value, 2));
        }
    }
    (low..=high).contains(&first).then_some((first, 1))
}

/// Matches UTC/GMT or a session zone abbreviation without consuming a following literal.
fn zone_length(input: &str, zones: Option<&[String; 2]>) -> Option<usize> {
    [
        Some("UTC"),
        Some("GMT"),
        zones.map(|names| names[0].as_str()),
        zones.map(|names| names[1].as_str()),
    ]
    .into_iter()
    .flatten()
    .filter(|name| {
        !name.is_empty()
            && input
                .get(..name.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
    })
    .map(str::len)
    .max()
}

/// Expands clock aliases so their seconds and meridiem also use Python semantics.
fn expand_format(input: &str, format: &str) -> RunResult<String> {
    let mut out = String::with_capacity(format.len());
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            out.push_str(match chars.next() {
                Some('c') => "%a %b %e %H:%M:%S %Y",
                Some('x') => "%m/%d/%y",
                Some('X' | 'T') => "%H:%M:%S",
                Some('r') => "%I:%M:%S %p",
                Some('R') => "%H:%M",
                Some('f') => "%6f",
                Some('Q' | 'N') => return Err(format_mismatch(input, format)),
                Some(':') => {
                    return Err(ExcType::value_error(format_args!(
                        "':' is a bad directive in format {}",
                        StringRepr(format)
                    )));
                }
                Some(other) => {
                    out.push('%');
                    out.push(other);
                    continue;
                }
                None => "%",
            });
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Python's generic error for an input whose format did not match.
fn format_mismatch(input: &str, format: &str) -> RunError {
    ExcType::value_error(format_args!(
        "time data {} does not match format {}",
        StringRepr(input),
        StringRepr(format)
    ))
}

/// Supplies missing date fields without masking ordinal or week dates.
fn parsed_date(mut parsed: BrokenDownTime) -> Option<RunResult<CivilDate>> {
    let year = parsed.year().unwrap_or(1900);
    let has_week_date = parsed.iso_week_year().is_some()
        || (parsed.weekday().is_some()
            && (parsed.sunday_based_week().is_some() || parsed.monday_based_week().is_some()));
    if parsed.day_of_year().is_some() || has_week_date {
        parsed.set_year(Some(year)).ok()?;
        Some(Ok(parsed.to_date().ok()?))
    } else {
        Some(
            date::from_ymd(
                i32::from(year),
                i32::from(parsed.month().unwrap_or(1)),
                i32::from(parsed.day().unwrap_or(1)),
            )
            .map(|date| date.0),
        )
    }
}

/// A `%z` token, with separator validation deferred until the entire input matches.
struct ZoneToken {
    offset_seconds: i32,
    len: usize,
    colons_agree: bool,
}

/// Matches whole-second offsets; fractional offsets remain unsupported.
fn offset_at(input: &str) -> Option<ZoneToken> {
    let bytes = input.as_bytes();
    if bytes.first() == Some(&b'Z') {
        return Some(ZoneToken {
            offset_seconds: 0,
            len: 1,
            colons_agree: true,
        });
    }
    let sign = match bytes.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hours, at) = two_digits(bytes, 1)?;
    let minute_colon = bytes.get(at) == Some(&b':');
    let (minutes, at) = colon_pair(bytes, at)?;
    if minutes > 59 {
        return None;
    }
    let second_colon = bytes.get(at) == Some(&b':');
    let (seconds, at, colons_agree) = match colon_pair(bytes, at) {
        Some((seconds, next)) if seconds <= 59 => (seconds, next, second_colon == minute_colon),
        _ => (0, at, true),
    };
    Some(ZoneToken {
        offset_seconds: sign * i32::try_from(hours * 3600 + minutes * 60 + seconds).ok()?,
        len: at,
        colons_agree,
    })
}

/// Reads two ASCII decimal digits and the index just past them.
fn two_digits(bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    pair.iter()
        .all(u8::is_ascii_digit)
        .then(|| (u32::from(pair[0] - b'0') * 10 + u32::from(pair[1] - b'0'), at + 2))
}

/// Reads the optional colon and digit pair separating offset components.
fn colon_pair(bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    two_digits(bytes, if bytes.get(at) == Some(&b':') { at + 1 } else { at })
}
