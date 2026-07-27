//! Pure CPython-compatible formatting helpers shared by the boundary types:
//! string/bytes `repr()` escaping, shortest-round-trip float rendering, and
//! timezone-offset `timedelta` reprs.

use std::fmt::{self, Write};

use unicode_general_category::{GeneralCategory, get_general_category};
/// Writes a Python `repr()` string for a given string slice to a formatter.
///
/// Quote choice matches CPython: single quotes by default, switching to double
/// quotes only when the string contains a `'` but no `"` (so the quote needn't
/// be escaped). Backslash, the active quote, and `\n`/`\t`/`\r` use the short
/// escapes; any other **non-printable** character is escaped numerically
/// (`\xNN`/`\uNNNN`/`\UNNNNNNNN`), e.g. `repr('\x00') == "'\\x00'"` and
/// `repr('\xa0') == "'\\xa0'"`.
///
/// "Non-printable" matches CPython's `str.isprintable` (see
/// `repr_needs_escape`): Unicode categories `C*` and `Z*`, except the ASCII
/// space. Category data comes from `unicode-general-category`, whose Unicode
/// version may differ slightly from CPython's, affecting only recently
/// (re)assigned code points.
pub fn string_repr_fmt(s: &str, f: &mut impl Write) -> fmt::Result {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    f.write_char(quote)?;
    for c in s.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            '\r' => f.write_str("\\r")?,
            _ if c == quote => {
                f.write_char('\\')?;
                f.write_char(quote)?;
            }
            _ if repr_needs_escape(c) => write_char_escape(c, f)?,
            _ => f.write_char(c)?,
        }
    }
    f.write_char(quote)
}

/// Whether `c` is escaped numerically in a Python `repr` — i.e. it is not
/// "printable" in CPython's sense.
///
/// Non-printable = Unicode general categories `Other` (`Cc`, `Cf`, `Cs`, `Co`,
/// `Cn`) and `Separator` (`Zl`, `Zp`, `Zs`), with the sole exception of the
/// ASCII space `U+0020`. The `\t`/`\n`/`\r` short escapes are handled by the
/// caller before this is consulted.
fn repr_needs_escape(c: char) -> bool {
    c != ' '
        && matches!(
            get_general_category(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::SpaceSeparator
        )
}

/// Writes the numeric repr escape for a single character, matching CPython's
/// width selection: `\xNN` for code points `<= 0xFF`, `\uNNNN` for `<= 0xFFFF`,
/// otherwise `\UNNNNNNNN`.
fn write_char_escape(c: char, f: &mut impl Write) -> fmt::Result {
    let cp = c as u32;
    if cp <= 0xFF {
        write!(f, "\\x{cp:02x}")
    } else if cp <= 0xFFFF {
        write!(f, "\\u{cp:04x}")
    } else {
        write!(f, "\\U{cp:08x}")
    }
}

/// Formatter for a Python repr() string.
#[derive(Debug)]
pub struct StringRepr<'a>(pub &'a str);

impl fmt::Display for StringRepr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        string_repr_fmt(self.0, f)
    }
}
/// Writes a CPython-compatible repr string for bytes to a formatter.
///
/// Format: `b'...'` or `b"..."` depending on content.
/// - Uses single quotes by default
/// - Switches to double quotes if bytes contain `'` but not `"`
/// - Escapes: `\\`, `\t`, `\n`, `\r`, `\xNN` for non-printable bytes
pub fn bytes_repr_fmt(bytes: &[u8], f: &mut impl Write) -> fmt::Result {
    // Determine quote character: use double quotes if single quote present but not double
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let quote = if has_single && !has_double { '"' } else { '\'' };

    f.write_char('b')?;
    f.write_char(quote)?;

    for &byte in bytes {
        match byte {
            b'\\' => f.write_str("\\\\")?,
            b'\t' => f.write_str("\\t")?,
            b'\n' => f.write_str("\\n")?,
            b'\r' => f.write_str("\\r")?,
            b'\'' if quote == '\'' => f.write_str("\\'")?,
            b'"' if quote == '"' => f.write_str("\\\"")?,
            // Printable ASCII (32-126)
            0x20..=0x7e => f.write_char(byte as char)?,
            // Non-printable: use \xNN format
            _ => write!(f, "\\x{byte:02x}")?,
        }
    }

    f.write_char(quote)
}

/// Returns a CPython-compatible repr string for bytes.
///
/// Convenience wrapper around `bytes_repr_fmt` that returns an owned String.
#[must_use]
#[expect(clippy::missing_panics_doc, reason = "writing to a String cannot fail")]
pub fn bytes_repr(bytes: &[u8]) -> String {
    let mut result = String::new();
    // Writing to String never fails
    bytes_repr_fmt(bytes, &mut result).unwrap();
    result
}
/// A [`Display`](fmt::Display) adapter that writes a float exactly as CPython's
/// `repr()`/`str()` (identical for floats in Python 3): the shortest decimal
/// string that round-trips, switching to scientific notation when the base-10
/// exponent is `< -4` or `>= 16`, and always keeping at least one fractional
/// digit (`1.0`, never `1`) — `1e16` → `"1e+16"`, `1234.5` → `"1234.5"`,
/// `inf`/`nan` lowercased.
///
/// This is the default rendering for a bare `f"{x}"`, `str(x)`, `repr(x)` and
/// floats inside container reprs — *not* the format mini-language (that's
/// `format_float_g` et al, in `monty`). Rust can't do this directly: its `f64` `Display`
/// never uses scientific notation (`1e16` prints as `10000000000000000`) and
/// renders NaN as `"NaN"`.
///
/// As a `Display` adapter it writes straight to the caller's sink with **no
/// heap allocation**: it borrows Rust's *shortest-digits* guarantee via `{:e}`
/// into a small stack buffer (an `f64` `{:e}` is ASCII and ≤ 24 bytes) and
/// re-lays-out those digits per CPython's rules.
pub struct FormatFloat(pub f64);

impl fmt::Display for FormatFloat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        if v.is_nan() {
            return f.write_str("nan");
        }
        if v.is_sign_negative() {
            f.write_char('-')?;
        }
        if v.is_infinite() {
            return f.write_str("inf");
        }
        // Rust's shortest scientific form gives minimal round-tripping digits
        // plus the base-10 exponent (`1234.5` → `"1.2345e3"`, `0.0` → `"0e0"`),
        // captured in a stack buffer so nothing touches the heap.
        let mut sci = StackStr::new();
        write!(sci, "{:e}", v.abs())?;
        let sci = sci.as_str();
        let (mantissa, exp_str) = sci.split_once('e').ok_or(fmt::Error)?;
        // `{:e}` always emits a single leading digit, so the integer part is one
        // char and the fraction (if any) follows the `.`.
        let (int_part, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let exp10: i32 = exp_str.parse().map_err(|_| fmt::Error)?;
        let ndigits = int_part.len() + frac.len();
        // `decpt` = number of digits to the left of the decimal point.
        let decpt = exp10 + 1;

        if !(-4..16).contains(&exp10) {
            // Scientific: leading digit, optional fraction, then `e±NN`.
            f.write_str(int_part)?;
            if !frac.is_empty() {
                f.write_char('.')?;
                f.write_str(frac)?;
            }
            let exp_sign = if exp10 < 0 { '-' } else { '+' };
            write!(f, "e{exp_sign}{:02}", exp10.unsigned_abs())
        } else if decpt <= 0 {
            // `0.00…digits` — `-decpt` leading zeros after the point.
            f.write_str("0.")?;
            for _ in 0..-decpt {
                f.write_char('0')?;
            }
            f.write_str(int_part)?;
            f.write_str(frac)
        } else {
            let decpt = usize::try_from(decpt).expect("decpt is positive in this branch");
            if decpt >= ndigits {
                // Integer-valued: digits, zeros up to the point, then `.0`.
                f.write_str(int_part)?;
                f.write_str(frac)?;
                for _ in 0..decpt - ndigits {
                    f.write_char('0')?;
                }
                f.write_str(".0")
            } else {
                // Point falls inside the digit run. `int_part` is a single digit
                // and `decpt >= 1`, so the split always lands within `frac`.
                f.write_str(int_part)?;
                let split = decpt - int_part.len();
                f.write_str(&frac[..split])?;
                f.write_char('.')?;
                f.write_str(&frac[split..])
            }
        }
    }
}

/// A fixed-capacity [`fmt::Write`] sink backed by a stack array, used to capture
/// a bounded `{:e}` rendering without a heap allocation.
///
/// 32 bytes comfortably holds any `f64` `{:e}` output (the longest is ~24 ASCII
/// bytes, e.g. `2.2250738585072014e-308`). A write that would overflow returns
/// [`fmt::Error`] rather than panicking — unreachable for the bounded `f64`
/// case, but it keeps the type panic-free for any future caller.
struct StackStr {
    buf: [u8; 32],
    len: usize,
}

impl StackStr {
    fn new() -> Self {
        Self { buf: [0; 32], len: 0 }
    }

    fn as_str(&self) -> &str {
        // Only `{:e}` of an `f64` is written here, which is always valid ASCII.
        str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl fmt::Write for StackStr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        let slot = self.buf.get_mut(self.len..end).ok_or(fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
/// Classifies an invalid-UTF-8 error into CPython's reason wording, from the
/// first unexpected byte and `Utf8Error::error_len()`.
///
/// `error_len == None` means the input ended mid-sequence (`unexpected end of
/// data`); otherwise a byte that is a legal multi-byte lead (0xC2–0xF4) was
/// followed by an invalid continuation, and anything else (stray
/// continuation bytes, the overlong leads 0xC0/0xC1, 0xF5–0xFF) is an
/// `invalid start byte`. Public (re-exported at the crate root) so `monty-fs`
/// produces identical wording for text-mode file reads.
#[must_use]
pub fn utf8_error_reason(first_bad_byte: u8, error_len: Option<usize>) -> &'static str {
    if error_len.is_none() {
        "unexpected end of data"
    } else if (0xC2..=0xF4).contains(&first_bad_byte) {
        "invalid continuation byte"
    } else {
        "invalid start byte"
    }
}

/// Formats the canonical `datetime.timedelta(...)` repr for a fixed timezone
/// offset in seconds, normalized like Python's `timedelta` (`days` may be
/// negative, `seconds` in `0..86400`) — e.g. `-18000` →
/// `datetime.timedelta(days=-1, seconds=68400)`. Used by the
/// `datetime.timezone` reprs of [`MontyObject`](crate::object::MontyObject).
#[must_use]
pub fn format_offset_timedelta_repr(offset_seconds: i32) -> String {
    const SECONDS_PER_DAY: i32 = 86_400;
    let days = offset_seconds.div_euclid(SECONDS_PER_DAY);
    let seconds = offset_seconds.rem_euclid(SECONDS_PER_DAY);
    if days == 0 && seconds == 0 {
        "datetime.timedelta(0)".to_owned()
    } else {
        let mut out = String::from("datetime.timedelta(");
        if days != 0 {
            write!(out, "days={days}").expect("writing to String never fails");
        }
        if seconds != 0 {
            if days != 0 {
                out.push_str(", ");
            }
            write!(out, "seconds={seconds}").expect("writing to String never fails");
        }
        out.push(')');
        out
    }
}
