//! Text codec registry and implementations backing `str.encode()` and
//! `bytes.decode()`.
//!
//! Monty implements a fixed, small set of codecs (UTF-8, ASCII, UTF-16 and
//! UTF-32 families) rather than CPython's full `codecs`/`encodings` registry.
//! [`Codec::find`] resolves user-supplied encoding names using CPython's
//! normalization rules and alias table; [`Codec::encode`] / [`Codec::decode`]
//! implement the conversions with CPython-matching error handler semantics
//! and exception messages. Divergences are documented in
//! `limitations/encoding.md` — most notably the `surrogateescape` /
//! `surrogatepass` decode handlers, which would put lone surrogates in the
//! result and therefore raise `NotImplementedError` (Monty strings are strict
//! UTF-8).

use std::{
    fmt::{self, Write},
    str,
};

use crate::{
    exception_private::{ExcType, RunError, RunResult},
    resource::ResourceTracker,
    string_builder::StringBuilder,
};

/// A supported text codec, resolved from a user-supplied encoding name via
/// [`Codec::find`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    Utf8,
    Ascii,
    /// `None` is the bare `utf-16` variant: encode prepends a BOM, decode
    /// consumes one (defaulting to little-endian when absent — see
    /// `limitations/encoding.md`). `Some` variants never touch a BOM.
    Utf16(Option<Endian>),
    /// Same BOM semantics as [`Codec::Utf16`], with 4-byte units.
    Utf32(Option<Endian>),
}

/// Byte order for the UTF-16/UTF-32 codec families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endian {
    Little,
    Big,
}

impl Codec {
    /// Resolves an encoding name to a codec, applying CPython's name
    /// normalization and the alias table from `Lib/encodings/aliases.py`
    /// (restricted to the codecs Monty supports). Returns `None` for unknown
    /// names — the caller raises `LookupError` with the *original* name.
    pub(crate) fn find(name: &str) -> Option<Self> {
        // Fast path for the exact spellings virtually all callers use —
        // including the `"utf-8"` default of `str.encode`/`bytes.decode` —
        // so the hot path skips `normalize_encoding`'s String allocation.
        match name {
            "utf-8" | "utf8" => Some(Self::Utf8),
            "ascii" => Some(Self::Ascii),
            "utf-16" => Some(Self::Utf16(None)),
            "utf-32" => Some(Self::Utf32(None)),
            _ => Self::find_normalized(name),
        }
    }

    /// Slow path of [`Codec::find`]: normalizes `name` (allocating) and
    /// matches it against the normalized names and aliases.
    fn find_normalized(name: &str) -> Option<Self> {
        match normalize_encoding(name).as_str() {
            "utf_8" | "utf8" | "utf" | "u8" | "cp65001" | "utf8_ucs2" | "utf8_ucs4" => Some(Self::Utf8),
            "ascii" | "646" | "us" | "us_ascii" | "cp367" | "ibm367" | "csascii" | "ansi_x3.4_1968"
            | "ansi_x3_4_1968" | "ansi_x3.4_1986" | "iso646_us" | "iso_646.irv_1991" | "iso_ir_6" => Some(Self::Ascii),
            "utf_16" | "u16" | "utf16" => Some(Self::Utf16(None)),
            "utf_16_le" | "utf_16le" | "unicodelittleunmarked" => Some(Self::Utf16(Some(Endian::Little))),
            "utf_16_be" | "utf_16be" | "unicodebigunmarked" => Some(Self::Utf16(Some(Endian::Big))),
            "utf_32" | "u32" | "utf32" => Some(Self::Utf32(None)),
            "utf_32_le" | "utf_32le" => Some(Self::Utf32(Some(Endian::Little))),
            "utf_32_be" | "utf_32be" => Some(Self::Utf32(Some(Endian::Big))),
            _ => None,
        }
    }

    /// Encodes `s` to bytes. The `errors` handler name is validated lazily —
    /// only consulted when a character actually can't be encoded, matching
    /// CPython's `codecs.lookup_error` semantics. Only the ASCII codec can
    /// fail: UTF-8 is the native representation, and every Monty string is
    /// encodable as UTF-16/32 (no lone surrogates can exist).
    pub(crate) fn encode(self, s: &str, errors: &str, tracker: &impl ResourceTracker) -> RunResult<Vec<u8>> {
        match self {
            Self::Utf8 => Ok(s.as_bytes().to_vec()),
            Self::Ascii => encode_ascii(s, errors, tracker),
            // Bare utf-16/utf-32 write a BOM and use little-endian (CPython
            // uses the platform's native order — identical on all LE hosts).
            Self::Utf16(endian) => Ok(encode_utf16(s, endian.unwrap_or(Endian::Little), endian.is_none())),
            Self::Utf32(endian) => Ok(encode_utf32(s, endian.unwrap_or(Endian::Little), endian.is_none())),
        }
    }

    /// Decodes `bytes` to a string. The `errors` handler name is validated
    /// lazily, as in [`Codec::encode`].
    ///
    /// Every decode path bounds its output by a small constant multiple of
    /// the (already resource-tracked) input — at most 4x, from
    /// `backslashreplace`'s `\xNN` escape per byte — so plain `String`
    /// accumulators are safe without `StringBuilder` tracking.
    pub(crate) fn decode(self, bytes: &[u8], errors: &str) -> RunResult<String> {
        match self {
            Self::Utf8 => decode_utf8(bytes, errors),
            Self::Ascii => decode_ascii(bytes, errors),
            Self::Utf16(Some(endian)) => decode_utf16(bytes, 0, endian, errors),
            Self::Utf32(Some(endian)) => decode_utf32(bytes, 0, endian, errors),
            // The bare variants consume a leading BOM and adopt its byte
            // order; with no BOM, CPython assumes the platform's native order
            // while Monty always uses little-endian for cross-platform
            // determinism (see limitations/encoding.md). Error positions are
            // offsets into the full input, so the payload start index is
            // passed through rather than slicing the BOM off.
            Self::Utf16(None) => match bytes {
                [0xFF, 0xFE, ..] => decode_utf16(bytes, 2, Endian::Little, errors),
                [0xFE, 0xFF, ..] => decode_utf16(bytes, 2, Endian::Big, errors),
                _ => decode_utf16(bytes, 0, Endian::Little, errors),
            },
            Self::Utf32(None) => match bytes {
                [0xFF, 0xFE, 0x00, 0x00, ..] => decode_utf32(bytes, 4, Endian::Little, errors),
                [0x00, 0x00, 0xFE, 0xFF, ..] => decode_utf32(bytes, 4, Endian::Big, errors),
                _ => decode_utf32(bytes, 0, Endian::Little, errors),
            },
        }
    }
}

/// Normalizes an encoding name for lookup, mirroring the combination of
/// CPython's C-level lower-casing and `encodings.normalize_encoding`:
/// ASCII-lowercase; collapse each run of non-alphanumeric characters to a
/// single `_` (dropping leading/trailing runs); keep `.` (needed for aliases
/// like `iso_646.irv_1990`); drop non-ASCII characters.
fn normalize_encoding(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_sep = false;
    for c in name.chars() {
        if c.is_alphanumeric() || c == '.' {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            if c.is_ascii() {
                out.push(c.to_ascii_lowercase());
            }
        } else {
            pending_sep = true;
        }
    }
    out
}

// ============================================================================
// Error handlers
// ============================================================================

/// CPython's built-in error handler names. Custom handlers
/// (`codecs.register_error`) don't exist in Monty — there is no `codecs`
/// module — so this set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorHandler {
    Strict,
    Ignore,
    Replace,
    Backslashreplace,
    Xmlcharrefreplace,
    Namereplace,
    Surrogateescape,
    Surrogatepass,
}

impl ErrorHandler {
    /// Resolves a handler name, raising CPython's
    /// `LookupError: unknown error handler name '{name}'` for unknown names.
    fn lookup(name: &str) -> RunResult<Self> {
        match name {
            "strict" => Ok(Self::Strict),
            "ignore" => Ok(Self::Ignore),
            "replace" => Ok(Self::Replace),
            "backslashreplace" => Ok(Self::Backslashreplace),
            "xmlcharrefreplace" => Ok(Self::Xmlcharrefreplace),
            "namereplace" => Ok(Self::Namereplace),
            "surrogateescape" => Ok(Self::Surrogateescape),
            "surrogatepass" => Ok(Self::Surrogatepass),
            _ => Err(ExcType::lookup_error_unknown_error_handler(name)),
        }
    }
}

/// Lazily-resolved error handler: the name is only looked up (and validated)
/// on the first actual encode/decode error, matching CPython's lazy
/// `codecs.lookup_error` — `b'ok'.decode('ascii', 'bogus')` succeeds because
/// the handler is never needed. Caches the result so repeated errors don't
/// re-match the name.
struct LazyHandler<'a> {
    name: &'a str,
    cached: Option<ErrorHandler>,
}

impl<'a> LazyHandler<'a> {
    fn new(name: &'a str) -> Self {
        Self { name, cached: None }
    }

    fn get(&mut self) -> RunResult<ErrorHandler> {
        if let Some(handler) = self.cached {
            Ok(handler)
        } else {
            let handler = ErrorHandler::lookup(self.name)?;
            self.cached = Some(handler);
            Ok(handler)
        }
    }
}

/// Applies a decode error handler to one undecodable unit `bad` (a maximal
/// UTF-8 subpart, a UTF-16/32 code unit, or a single non-ASCII byte),
/// appending any substitution to `out`.
///
/// `is_surrogate` says whether `surrogatepass` would successfully decode this
/// unit to a lone surrogate in CPython: if so Monty raises
/// `NotImplementedError` (lone surrogates are unrepresentable), otherwise
/// `surrogatepass` re-raises the strict error exactly like CPython.
/// `strict_err` builds the codec-specific `UnicodeDecodeError`.
fn handle_decode_error(
    handler: ErrorHandler,
    bad: &[u8],
    out: &mut String,
    is_surrogate: bool,
    strict_err: impl FnOnce() -> RunError,
) -> RunResult<()> {
    match handler {
        ErrorHandler::Strict => Err(strict_err()),
        ErrorHandler::Ignore => Ok(()),
        ErrorHandler::Replace => {
            out.push('\u{FFFD}');
            Ok(())
        }
        ErrorHandler::Backslashreplace => {
            for &byte in bad {
                write!(out, "\\x{byte:02x}").expect("writing to a String is infallible");
            }
            Ok(())
        }
        // Encode-only handlers: CPython's real callbacks raise TypeError when
        // invoked for a decode error.
        ErrorHandler::Xmlcharrefreplace | ErrorHandler::Namereplace => Err(ExcType::type_error_decode_error_callback()),
        ErrorHandler::Surrogateescape => Err(ExcType::not_implemented_surrogate_handler_decode("surrogateescape")),
        ErrorHandler::Surrogatepass => {
            if is_surrogate {
                Err(ExcType::not_implemented_surrogate_handler_decode("surrogatepass"))
            } else {
                Err(strict_err())
            }
        }
    }
}

// ============================================================================
// ASCII
// ============================================================================

/// Encodes `s` as ASCII bytes, applying `errors` to any non-ASCII characters.
///
/// `ignore`, `replace`, `backslashreplace`, `xmlcharrefreplace`, and
/// `namereplace` substitute, while `strict`, `surrogateescape`, and
/// `surrogatepass` raise `UnicodeEncodeError` — the latter two only
/// special-case lone surrogates, which a Monty string (strict UTF-8) can
/// never contain, so they re-raise exactly like `strict`, matching CPython.
///
/// Builds into a tracker-protected [`StringBuilder`] rather than a `Vec<u8>` —
/// every byte pushed is itself valid ASCII, so the accumulator is always valid
/// UTF-8 and `into_bytes()` at the end is free. The tracking matters because
/// `namereplace` amplifies a single character into up to ~90 bytes
/// (`\N{LONGEST UNICODE NAME...}`), so an untracked accumulator could grow
/// far past the memory limit before the final allocation was checked.
fn encode_ascii(s: &str, errors: &str, tracker: &impl ResourceTracker) -> RunResult<Vec<u8>> {
    // Fast path for the overwhelmingly common all-ASCII case: `is_ascii` is
    // SIMD-vectorized in std, and the output is byte-for-byte the (already
    // tracked) input, so a bulk copy needs no StringBuilder.
    if s.is_ascii() {
        return Ok(s.as_bytes().to_vec());
    }
    let mut handler = LazyHandler::new(errors);
    let mut out = StringBuilder::with_capacity(s.len(), tracker)?;
    let mut chars = s.chars().enumerate().peekable();
    while let Some((idx, c)) = chars.next() {
        if c.is_ascii() {
            out.push(c)?;
            continue;
        }
        match handler.get()? {
            ErrorHandler::Ignore => {}
            ErrorHandler::Replace => out.push('?')?,
            // A failed `write!` stashes the tracker error on the builder and
            // short-circuits later writes; `finish_raw` below surfaces it.
            ErrorHandler::Backslashreplace => {
                let _ = write_backslash_escape(&mut out, c);
            }
            ErrorHandler::Xmlcharrefreplace => {
                let _ = write!(out, "&#{};", c as u32);
            }
            ErrorHandler::Namereplace => {
                // Characters without a Unicode name (e.g. C1 controls) fall
                // back to backslash escapes, matching CPython.
                let _ = match unicode_names2::name(c) {
                    Some(name) => write!(out, "\\N{{{name}}}"),
                    None => write_backslash_escape(&mut out, c),
                };
            }
            // surrogateescape/surrogatepass only substitute lone surrogates,
            // which can't occur here, so they re-raise like strict.
            ErrorHandler::Strict | ErrorHandler::Surrogateescape | ErrorHandler::Surrogatepass => {
                // CPython reports a single position for a lone bad character,
                // but merges a contiguous run of unencodable characters into
                // one `position start-end` range.
                let mut end = idx + 1;
                while let Some(&(_, next_c)) = chars.peek() {
                    if next_c.is_ascii() {
                        break;
                    }
                    chars.next();
                    end += 1;
                }
                return Err(ExcType::unicode_encode_error(
                    "ascii",
                    s,
                    c,
                    idx,
                    end,
                    "ordinal not in range(128)",
                ));
            }
        }
    }
    Ok(out.finish_raw()?.into_bytes())
}

/// Writes the `backslashreplace` escape for a non-ASCII character: `\xNN` for
/// codepoints <= 0xFF, `\uNNNN` up to the BMP, `\UNNNNNNNN` beyond. Shared by
/// the `backslashreplace` handler and `namereplace`'s unnamed-character
/// fallback in [`encode_ascii`].
fn write_backslash_escape(out: &mut impl fmt::Write, c: char) -> fmt::Result {
    let code = c as u32;
    if code <= 0xFF {
        write!(out, "\\x{code:02x}")
    } else if code <= 0xFFFF {
        write!(out, "\\u{code:04x}")
    } else {
        write!(out, "\\U{code:08x}")
    }
}

/// Decodes `bytes` as ASCII text, applying `errors` to any byte >= 0x80.
/// `surrogatepass` re-raises like `strict` (with the ASCII codec it only
/// special-cases surrogate sequences in the UTF codecs), matching CPython.
fn decode_ascii(bytes: &[u8], errors: &str) -> RunResult<String> {
    // Fast path for the overwhelmingly common all-clean case: `is_ascii` is
    // SIMD-vectorized in std, and all-ASCII bytes are valid UTF-8 as-is.
    if bytes.is_ascii() {
        return Ok(str::from_utf8(bytes)
            .expect("all-ASCII bytes are valid UTF-8")
            .to_owned());
    }
    let mut handler = LazyHandler::new(errors);
    let mut out = String::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte.is_ascii() {
            out.push(byte as char);
        } else {
            handle_decode_error(handler.get()?, &bytes[idx..=idx], &mut out, false, || {
                ExcType::unicode_decode_error("ascii", bytes, idx, idx + 1, "ordinal not in range(128)")
            })?;
        }
    }
    Ok(out)
}

// ============================================================================
// UTF-8
// ============================================================================

/// Decodes `bytes` as UTF-8, applying `errors` to each maximal invalid
/// subpart (the same "substitution of maximal subparts" policy CPython uses,
/// which `str::from_utf8`'s `error_len` reports directly — so `replace`
/// yields exactly one U+FFFD per subpart in both).
fn decode_utf8(bytes: &[u8], errors: &str) -> RunResult<String> {
    let mut handler = LazyHandler::new(errors);
    let mut out = String::with_capacity(bytes.len());
    let mut pos = 0;
    while pos < bytes.len() {
        match str::from_utf8(&bytes[pos..]) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(err) => {
                let bad_start = pos + err.valid_up_to();
                let bad_end = match err.error_len() {
                    Some(len) => bad_start + len,
                    None => bytes.len(),
                };
                out.push_str(str::from_utf8(&bytes[pos..bad_start]).expect("prefix validated by from_utf8"));
                let reason = utf8_error_reason(bytes[bad_start], err.error_len());
                // CPython's surrogatepass decodes a CESU-8 surrogate triple
                // (ED A0..BF 80..BF) to a lone surrogate; it re-raises for
                // any other invalid sequence.
                let is_surrogate = is_cesu8_surrogate(&bytes[bad_start..]);
                handle_decode_error(
                    handler.get()?,
                    &bytes[bad_start..bad_end],
                    &mut out,
                    is_surrogate,
                    || ExcType::unicode_decode_error("utf-8", bytes, bad_start, bad_end, reason),
                )?;
                pos = bad_end;
            }
        }
    }
    Ok(out)
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

/// Returns true if `rest` starts with a complete CESU-8-encoded surrogate
/// (`ED A0..BF 80..BF`), the only invalid-UTF-8 shape CPython's
/// `surrogatepass` handler accepts.
fn is_cesu8_surrogate(rest: &[u8]) -> bool {
    matches!(rest, [0xED, b1, b2, ..] if (0xA0..=0xBF).contains(b1) && (0x80..=0xBF).contains(b2))
}

// ============================================================================
// UTF-16 / UTF-32
// ============================================================================

/// Encodes `s` as UTF-16 in the given byte order, optionally prepending a
/// BOM (the bare `utf-16` codec writes one even for an empty string,
/// matching CPython). Infallible: every char has a UTF-16 encoding.
///
/// The exact output size is computed up front (2 bytes per UTF-16 code unit,
/// at most 4 bytes per input char), so the untracked `Vec` is bounded by a
/// small constant multiple of the already-tracked input.
fn encode_utf16(s: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let units: usize = s.chars().map(char::len_utf16).sum::<usize>() + usize::from(with_bom);
    let mut out = Vec::with_capacity(units * 2);
    if with_bom {
        push_u16(&mut out, 0xFEFF, endian);
    }
    let mut buf = [0u16; 2];
    for c in s.chars() {
        for &unit in c.encode_utf16(&mut buf).iter() {
            push_u16(&mut out, unit, endian);
        }
    }
    out
}

/// Encodes `s` as UTF-32 (4 bytes per char), optionally prepending a BOM.
/// Infallible; same size-bounding rationale as [`encode_utf16`].
fn encode_utf32(s: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity((s.chars().count() + usize::from(with_bom)) * 4);
    if with_bom {
        push_u32(&mut out, 0xFEFF, endian);
    }
    for c in s.chars() {
        push_u32(&mut out, c as u32, endian);
    }
    out
}

/// Decodes UTF-16 payload starting at byte index `start` (2 when a BOM was
/// consumed, so error positions include it, matching CPython).
///
/// Error units follow CPython's taxonomy: a lone trailing byte is `truncated
/// data`; a high surrogate with fewer than 2 bytes left for its pair makes
/// the whole 2–3 byte tail ONE `unexpected end of data` unit; a high
/// surrogate followed by a non-low-surrogate unit is an `illegal UTF-16
/// surrogate`; a lone low surrogate is an `illegal encoding`.
fn decode_utf16(bytes: &[u8], start: usize, endian: Endian, errors: &str) -> RunResult<String> {
    let codec = match endian {
        Endian::Little => "utf-16-le",
        Endian::Big => "utf-16-be",
    };
    let mut handler = LazyHandler::new(errors);
    let mut out = String::with_capacity(bytes.len());
    let mut i = start;
    while i < bytes.len() {
        if bytes.len() - i == 1 {
            handle_decode_error(handler.get()?, &bytes[i..], &mut out, false, || {
                ExcType::unicode_decode_error(codec, bytes, i, i + 1, "truncated data")
            })?;
            break;
        }
        let unit = read_u16(bytes, i, endian);
        if !(0xD800..0xE000).contains(&unit) {
            out.push(char::from_u32(u32::from(unit)).expect("non-surrogate BMP code unit is a valid char"));
            i += 2;
        } else if unit >= 0xDC00 {
            // low surrogate with no preceding high surrogate
            handle_decode_error(handler.get()?, &bytes[i..i + 2], &mut out, true, || {
                ExcType::unicode_decode_error(codec, bytes, i, i + 2, "illegal encoding")
            })?;
            i += 2;
        } else if bytes.len() - i < 4 {
            // high surrogate but the input ends before its pair: the whole
            // 2-3 byte tail is a single error unit (one U+FFFD under replace)
            let end = bytes.len();
            handle_decode_error(handler.get()?, &bytes[i..end], &mut out, true, || {
                ExcType::unicode_decode_error(codec, bytes, i, end, "unexpected end of data")
            })?;
            break;
        } else {
            let low = read_u16(bytes, i + 2, endian);
            if (0xDC00..0xE000).contains(&low) {
                let code = 0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                out.push(char::from_u32(code).expect("surrogate pair decodes to a valid char"));
                i += 4;
            } else {
                handle_decode_error(handler.get()?, &bytes[i..i + 2], &mut out, true, || {
                    ExcType::unicode_decode_error(codec, bytes, i, i + 2, "illegal UTF-16 surrogate")
                })?;
                i += 2;
            }
        }
    }
    Ok(out)
}

/// Decodes UTF-32 payload starting at byte index `start` (4 when a BOM was
/// consumed). Error units are single 4-byte code points (surrogate or
/// out-of-range values) or the truncated 1–3 byte tail.
fn decode_utf32(bytes: &[u8], start: usize, endian: Endian, errors: &str) -> RunResult<String> {
    let codec = match endian {
        Endian::Little => "utf-32-le",
        Endian::Big => "utf-32-be",
    };
    let mut handler = LazyHandler::new(errors);
    let mut out = String::with_capacity(bytes.len());
    let mut i = start;
    while i < bytes.len() {
        if bytes.len() - i < 4 {
            let end = bytes.len();
            handle_decode_error(handler.get()?, &bytes[i..], &mut out, false, || {
                ExcType::unicode_decode_error(codec, bytes, i, end, "truncated data")
            })?;
            break;
        }
        let code = read_u32(bytes, i, endian);
        if let Some(c) = char::from_u32(code) {
            out.push(c);
        } else {
            let (reason, is_surrogate) = if (0xD800..0xE000).contains(&code) {
                ("code point in surrogate code point range(0xd800, 0xe000)", true)
            } else {
                ("code point not in range(0x110000)", false)
            };
            handle_decode_error(handler.get()?, &bytes[i..i + 4], &mut out, is_surrogate, || {
                ExcType::unicode_decode_error(codec, bytes, i, i + 4, reason)
            })?;
        }
        i += 4;
    }
    Ok(out)
}

/// Appends one UTF-16 code unit in the given byte order.
fn push_u16(out: &mut Vec<u8>, unit: u16, endian: Endian) {
    out.extend_from_slice(&match endian {
        Endian::Little => unit.to_le_bytes(),
        Endian::Big => unit.to_be_bytes(),
    });
}

/// Appends one UTF-32 code unit in the given byte order.
fn push_u32(out: &mut Vec<u8>, code: u32, endian: Endian) {
    out.extend_from_slice(&match endian {
        Endian::Little => code.to_le_bytes(),
        Endian::Big => code.to_be_bytes(),
    });
}

/// Reads the 2-byte code unit at `i` (caller guarantees bounds).
fn read_u16(bytes: &[u8], i: usize, endian: Endian) -> u16 {
    let pair = [bytes[i], bytes[i + 1]];
    match endian {
        Endian::Little => u16::from_le_bytes(pair),
        Endian::Big => u16::from_be_bytes(pair),
    }
}

/// Reads the 4-byte code unit at `i` (caller guarantees bounds).
fn read_u32(bytes: &[u8], i: usize, endian: Endian) -> u32 {
    let quad = [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
    match endian {
        Endian::Little => u32::from_le_bytes(quad),
        Endian::Big => u32::from_be_bytes(quad),
    }
}
