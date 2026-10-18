//! TOML parsing for `tomllib`: a port of CPython's `Lib/tomllib/_parser.py`.
//!
//! The parser walks the UTF-8 bytes of the `\r\n`-normalised source — every
//! structural TOML character is ASCII, so byte positions are always safe to
//! cut at — and builds a [`TomlTable`] tree without touching the heap. Each
//! function mirrors its CPython namesake so messages, the position they are
//! reported at and the order the checks run in stay identical. Positions in a
//! [`TomlError::Decode`] are byte offsets; `super` converts them to character
//! indexes when raising.
//!
//! The tree is allocator-visible but costs far more per element than the
//! source text (`1,` becomes a [`TomlValue`]), so the element loops poll the
//! resource tracker and every array, table and flags map preflights its
//! doubling, turning a huge document into a graceful `MemoryError`.

use std::{borrow::Cow, collections::HashMap, mem, vec};

use chrono::NaiveDate;
use indexmap::{IndexMap, map};
use monty_types::{ResourceError, ResourceTracker};
use num_bigint::BigInt;

use crate::{
    exception_private::RunError,
    types::{long_int::check_decimal_digit_count, str::string_repr_fmt},
};

/// CPython caps a dotted key at `sys.getrecursionlimit()` parts (1000 by
/// default) because the key names a recursive structure; Monty uses the
/// default as a fixed limit.
const MAX_KEY_PARTS: usize = 1000;

/// Nesting depth of inline arrays and tables before `RecursionError`.
///
/// CPython's recursive descent hits its own recursion limit at about 330
/// nested inline tables or 500 nested arrays; the parser recurses per level
/// too, with frames large enough in debug builds that this depth is what a
/// 2 MiB thread stack comfortably holds.
const MAX_NESTING: usize = 100;

/// `Flags::FROZEN`: an inline array or table, immutable from then on.
const FROZEN: u8 = 1;
/// `Flags::EXPLICIT_NEST`: a table created explicitly, which `[table]` syntax
/// may not open again.
const EXPLICIT_NEST: u8 = 2;

/// Bytes per entry a full [`Flags`] map allocates when it grows: hashbrown
/// rehashes into twice the slots, each a `(String, FlagNode)` and a control byte.
const FLAG_ENTRY_GROWTH: usize = 2 * (mem::size_of::<(String, FlagNode)>() + 1);

/// Bytes per entry a full [`TomlTable`] allocates when it grows: the
/// `(hash, key, value)` bucket plus two index slots, since the rehashed
/// index table (a `usize` and a control byte per slot) doubles too.
const TABLE_ENTRY_GROWTH: usize = mem::size_of::<(usize, String, TomlValue)>() + 2 * (mem::size_of::<usize>() + 1);

/// A dotted key, one `String` per part.
pub(super) type Key = Vec<String>;

/// A parsed TOML value, converted to a Monty value by [`super::value`].
pub(super) enum TomlValue {
    Str(String),
    Int(TomlInt),
    Float(f64),
    Bool(bool),
    Date(TomlDate),
    Time(TomlTime),
    /// A local date-time (`offset_seconds` `None`) or an offset date-time.
    DateTime {
        date: TomlDate,
        time: TomlTime,
        offset_seconds: Option<i32>,
    },
    Array(Vec<Self>),
    Table(TomlTable),
}

impl TomlValue {
    /// Whether this value freezes its namespace: CPython's
    /// `isinstance(value, (dict, list))`.
    fn is_container(&self) -> bool {
        matches!(self, Self::Array(_) | Self::Table(_))
    }
}

/// A TOML integer: `i64` when it fits, else a big integer.
pub(super) enum TomlInt {
    Small(i64),
    Big(BigInt),
}

/// A calendar date already validated against `datetime.date`'s ranges.
#[derive(Clone, Copy)]
pub(super) struct TomlDate {
    pub(super) year: i32,
    pub(super) month: i32,
    pub(super) day: i32,
}

/// A wall-clock time; the regex that produced it guarantees the ranges.
#[derive(Clone, Copy)]
pub(super) struct TomlTime {
    pub(super) hour: i32,
    pub(super) minute: i32,
    pub(super) second: i32,
    pub(super) microsecond: i32,
}

/// A TOML table in insertion order (CPython's `dict`).
#[derive(Default)]
pub(super) struct TomlTable(pub(super) IndexMap<String, TomlValue>);

impl TomlTable {
    /// Takes the entries out, leaving the `Drop`-implementing shell empty.
    pub(super) fn into_entries(mut self) -> IndexMap<String, TomlValue> {
        mem::take(&mut self.0)
    }
}

impl Drop for TomlTable {
    /// Drops the subtree iteratively: `[a.b.c…]` headers nest tables up to
    /// [`MAX_KEY_PARTS`] deep, beyond what recursive drop glue can rely on.
    /// The stack holds one iterator per level, so cleanup after a
    /// `MemoryError` allocates in proportion to depth, never width.
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }
        let mut stack = vec![Children::Table(mem::take(&mut self.0).into_values())];
        while let Some(children) = stack.last_mut() {
            match children.next() {
                Some(TomlValue::Array(items)) => stack.push(Children::Array(items.into_iter())),
                Some(TomlValue::Table(mut table)) => {
                    stack.push(Children::Table(mem::take(&mut table.0).into_values()));
                }
                Some(_) => {}
                None => {
                    stack.pop();
                }
            }
        }
    }
}

/// The not-yet-dropped children of one container, for [`TomlTable`]'s drop.
enum Children {
    Array(vec::IntoIter<TomlValue>),
    Table(map::IntoValues<String, TomlValue>),
}

impl Iterator for Children {
    type Item = TomlValue;

    fn next(&mut self) -> Option<TomlValue> {
        match self {
            Self::Array(items) => items.next(),
            Self::Table(values) => values.next(),
        }
    }
}

/// Why parsing stopped.
pub(super) enum TomlError {
    /// `TOMLDecodeError` with CPython's message, at a byte offset into the source.
    Decode { msg: String, pos: usize },
    /// `RecursionError`: a key with too many parts, or containers nested too deep.
    Recursion(String),
    /// An exception a conversion raised, e.g. `int()`'s digit limit (`ValueError`).
    /// Boxed to keep the recursive parser's frames small.
    Run(Box<RunError>),
}

impl From<ResourceError> for TomlError {
    fn from(error: ResourceError) -> Self {
        Self::Run(Box::new(RunError::from(error)))
    }
}

type ParseResult<T> = Result<T, TomlError>;

/// Shorthand for raising a `TOMLDecodeError` at `pos`.
fn decode_error<T>(msg: impl Into<String>, pos: usize) -> ParseResult<T> {
    Err(TomlError::Decode { msg: msg.into(), pos })
}

/// Parses a whole document, one statement (usually one line) at a time.
pub(super) fn parse(src: &str, tracker: &ResourceTracker) -> ParseResult<TomlTable> {
    let bytes = src.as_bytes();
    let mut out = Output {
        data: TomlTable::default(),
        flags: Flags::default(),
        tracker,
    };
    let mut header: Key = Vec::new();
    let mut pos = 0;
    let mut statements = 0;
    loop {
        pos = skip_chars(bytes, pos, is_ws);
        let Some(&first) = bytes.get(pos) else { break };
        if first == b'\n' {
            pos += 1;
            continue;
        }
        // Every statement but a comment adds to the tree (`[[a]]` a whole table).
        check_memory_time_every(out.tracker, statements)?;
        statements += 1;
        if is_key_initial_char(first) {
            pos = key_value_rule(src, pos, &mut out, &header)?;
            pos = skip_chars(bytes, pos, is_ws);
        } else if first == b'[' {
            out.flags.finalize_pending(out.tracker)?;
            (pos, header) = if bytes.get(pos + 1) == Some(&b'[') {
                create_list_rule(src, pos, &mut out)?
            } else {
                create_dict_rule(src, pos, &mut out)?
            };
            pos = skip_chars(bytes, pos, is_ws);
        } else if first != b'#' {
            return decode_error("Invalid statement", pos);
        }
        pos = skip_comment(bytes, pos)?;
        let Some(&next) = bytes.get(pos) else { break };
        if next != b'\n' {
            return decode_error("Expected newline or end of document after a statement", pos);
        }
        pos += 1;
    }
    Ok(out.data)
}

/// The document being built: its root table plus the namespace flags.
struct Output<'t> {
    data: TomlTable,
    flags: Flags,
    /// Polled per statement and per container element, see the module docs.
    tracker: &'t ResourceTracker,
}

/// `[table]` header: creates (or re-enters) the table the following key/value
/// pairs land in.
fn create_dict_rule(src: &str, pos: usize, out: &mut Output<'_>) -> ParseResult<(usize, Key)> {
    let bytes = src.as_bytes();
    let pos = skip_chars(bytes, pos + 1, is_ws);
    let (pos, key) = parse_key(src, pos)?;
    if out.flags.is(&key, EXPLICIT_NEST) || out.flags.is(&key, FROZEN) {
        return decode_error(format!("Cannot declare {} twice", format_key(&key)), pos);
    }
    out.flags.set(&key, EXPLICIT_NEST, false, out.tracker)?;
    if out.data.get_or_create_nest(&key, true, out.tracker)?.is_err() {
        return decode_error("Cannot overwrite a value", pos);
    }
    if bytes.get(pos) != Some(&b']') {
        return decode_error("Expected ']' at the end of a table declaration", pos);
    }
    Ok((pos + 1, key))
}

/// `[[array]]` header: appends a new table to the array at `key`.
fn create_list_rule(src: &str, pos: usize, out: &mut Output<'_>) -> ParseResult<(usize, Key)> {
    let bytes = src.as_bytes();
    let pos = skip_chars(bytes, pos + 2, is_ws);
    let (pos, key) = parse_key(src, pos)?;
    if out.flags.is(&key, FROZEN) {
        return decode_error(format!("Cannot mutate immutable namespace {}", format_key(&key)), pos);
    }
    // Free the namespace now that it points to another empty list item,
    // but this key precisely is still prohibited from table declaration.
    out.flags.unset_all(&key);
    out.flags.set(&key, EXPLICIT_NEST, false, out.tracker)?;
    if out.data.append_nest_to_list(&key, out.tracker)?.is_err() {
        return decode_error("Cannot overwrite a value", pos);
    }
    if !bytes[pos..].starts_with(b"]]") {
        return decode_error("Expected ']]' at the end of an array declaration", pos);
    }
    Ok((pos + 2, key))
}

/// A top-level `key = value` statement under the current `[header]`.
fn key_value_rule(src: &str, pos: usize, out: &mut Output<'_>, header: &[String]) -> ParseResult<usize> {
    let (pos, key, value) = parse_key_value_pair(src, pos, 0, out.tracker)?;
    let (key_stem, key_parent) = key.split_last().expect("parse_key yields at least one part");
    let abs_key_parent: Key = header.iter().chain(key_parent).cloned().collect();
    let Output { data, flags, tracker } = out;

    for i in 1..key.len() {
        let cont_key: Key = header.iter().chain(&key[..i]).cloned().collect();
        // Dotted key syntax may not redefine an existing table...
        if flags.is(&cont_key, EXPLICIT_NEST) {
            return decode_error(format!("Cannot redefine namespace {}", format_key(&cont_key)), pos);
        }
        // ...and the containers on its path can't be opened with the table
        // syntax or dotted key/value syntax in following table sections.
        flags.add_pending(cont_key, EXPLICIT_NEST, tracker)?;
    }

    if flags.is(&abs_key_parent, FROZEN) {
        return decode_error(
            format!("Cannot mutate immutable namespace {}", format_key(&abs_key_parent)),
            pos,
        );
    }
    let Ok(nest) = data.get_or_create_nest(&abs_key_parent, true, tracker)? else {
        return decode_error("Cannot overwrite a value", pos);
    };
    if nest.0.contains_key(key_stem) {
        return decode_error("Cannot overwrite a value", pos);
    }
    // Inline table and array namespaces become recursively immutable.
    if value.is_container() {
        let full_key: Key = header.iter().chain(&key).cloned().collect();
        flags.set(&full_key, FROZEN, true, tracker)?;
    }
    nest.insert(key_stem.clone(), value, tracker)?;
    Ok(pos)
}

/// `key = value`, returning the position after the value.
fn parse_key_value_pair(
    src: &str,
    pos: usize,
    depth: usize,
    tracker: &ResourceTracker,
) -> ParseResult<(usize, Key, TomlValue)> {
    let bytes = src.as_bytes();
    let (pos, key) = parse_key(src, pos)?;
    if bytes.get(pos) != Some(&b'=') {
        return decode_error("Expected '=' after a key in a key/value pair", pos);
    }
    let pos = skip_chars(bytes, pos + 1, is_ws);
    let (pos, value) = parse_value(src, pos, depth, tracker)?;
    Ok((pos, key, value))
}

/// A possibly dotted key, with whitespace allowed around the dots.
fn parse_key(src: &str, pos: usize) -> ParseResult<(usize, Key)> {
    let bytes = src.as_bytes();
    let (pos, part) = parse_key_part(src, pos)?;
    let mut key = vec![part];
    let mut pos = skip_chars(bytes, pos, is_ws);
    while bytes.get(pos) == Some(&b'.') {
        pos = skip_chars(bytes, pos + 1, is_ws);
        let (next, part) = parse_key_part(src, pos)?;
        key.push(part);
        if key.len() > MAX_KEY_PARTS {
            return Err(TomlError::Recursion(format!(
                "TOML key has more than the allowed {MAX_KEY_PARTS} parts"
            )));
        }
        pos = skip_chars(bytes, next, is_ws);
    }
    Ok((pos, key))
}

/// One bare, literal-quoted or basic-quoted key part.
fn parse_key_part(src: &str, pos: usize) -> ParseResult<(usize, String)> {
    let bytes = src.as_bytes();
    match bytes.get(pos) {
        Some(&first) if is_bare_key_char(first) => {
            let end = skip_chars(bytes, pos, is_bare_key_char);
            Ok((end, src[pos..end].to_owned()))
        }
        Some(b'\'') => parse_literal_str(src, pos),
        Some(b'"') => parse_basic_str(src, pos + 1, false),
        _ => decode_error("Invalid initial character for a key part", pos),
    }
}

/// Any value in a key/value pair or array; `depth` counts enclosing inline
/// containers for the nesting limit.
fn parse_value(src: &str, pos: usize, depth: usize, tracker: &ResourceTracker) -> ParseResult<(usize, TomlValue)> {
    let bytes = src.as_bytes();
    let rest = &bytes[pos.min(bytes.len())..];
    // Ordered as in CPython: strings and booleans, containers, then the
    // regex-shaped scalars, with numbers after dates since `1979-05-27` would
    // otherwise match as an integer.
    match rest.first() {
        Some(b'"') => {
            let (end, text) = if rest.starts_with(b"\"\"\"") {
                parse_multiline_str(src, pos, false)?
            } else {
                parse_basic_str(src, pos + 1, false)?
            };
            return Ok((end, TomlValue::Str(text)));
        }
        Some(b'\'') => {
            let (end, text) = if rest.starts_with(b"'''") {
                parse_multiline_str(src, pos, true)?
            } else {
                parse_literal_str(src, pos)?
            };
            return Ok((end, TomlValue::Str(text)));
        }
        Some(b't') if rest.starts_with(b"true") => return Ok((pos + 4, TomlValue::Bool(true))),
        Some(b'f') if rest.starts_with(b"false") => return Ok((pos + 5, TomlValue::Bool(false))),
        Some(b'[') => return parse_array(src, pos, nested(depth)?, tracker),
        Some(b'{') => return parse_inline_table(src, pos, nested(depth)?, tracker),
        _ => {}
    }

    if let Some((end, date, time)) = match_datetime(bytes, pos) {
        if !date.is_valid() {
            return decode_error("Invalid date or datetime", pos);
        }
        let value = match time {
            Some((time, offset_seconds)) => TomlValue::DateTime {
                date,
                time,
                offset_seconds,
            },
            None => TomlValue::Date(date),
        };
        return Ok((end, value));
    }
    if let Some((end, time)) = match_time(bytes, pos) {
        return Ok((end, TomlValue::Time(time)));
    }
    if let Some((end, is_float)) = match_number(bytes, pos) {
        return parse_number(&src[pos..end], is_float, pos).map(|number| (end, number));
    }

    let special = [
        (&b"inf"[..], f64::INFINITY),
        (b"nan", f64::NAN),
        (b"-inf", f64::NEG_INFINITY),
        (b"+inf", f64::INFINITY),
        // `float('-nan')` keeps the sign bit, visible through `math.copysign`.
        (b"-nan", -f64::NAN),
        (b"+nan", f64::NAN),
    ];
    for (literal, value) in special {
        if rest.starts_with(literal) {
            return Ok((pos + literal.len(), TomlValue::Float(value)));
        }
    }
    decode_error("Invalid value", pos)
}

/// [`ResourceTracker::check_memory_time_every`] as a parse error.
fn check_memory_time_every(tracker: &ResourceTracker, i: usize) -> ParseResult<()> {
    Ok(tracker.check_memory_time_every(i)?)
}

/// The depth for a container opened at `depth`, or `RecursionError` at the limit.
fn nested(depth: usize) -> ParseResult<usize> {
    if depth >= MAX_NESTING {
        Err(TomlError::Recursion("maximum recursion depth exceeded".to_owned()))
    } else {
        Ok(depth + 1)
    }
}

/// `[a, b, ...]`, with comments and newlines allowed between elements.
fn parse_array(src: &str, pos: usize, depth: usize, tracker: &ResourceTracker) -> ParseResult<(usize, TomlValue)> {
    let bytes = src.as_bytes();
    let mut array = Vec::new();
    let mut pos = skip_comments_and_array_ws(bytes, pos + 1)?;
    if bytes.get(pos) == Some(&b']') {
        return Ok((pos + 1, TomlValue::Array(array)));
    }
    loop {
        check_memory_time_every(tracker, array.len())?;
        let (next, value) = parse_value(src, pos, depth, tracker)?;
        // The poll above sees only what is allocated; a doubling of a long
        // array is preflighted so it cannot jump straight past the limit.
        tracker.check_growth(array.len(), array.capacity(), mem::size_of::<TomlValue>())?;
        array.push(value);
        pos = skip_comments_and_array_ws(bytes, next)?;
        match bytes.get(pos) {
            Some(b']') => return Ok((pos + 1, TomlValue::Array(array))),
            Some(b',') => {}
            _ => return decode_error("Unclosed array", pos),
        }
        pos = skip_comments_and_array_ws(bytes, pos + 1)?;
        if bytes.get(pos) == Some(&b']') {
            return Ok((pos + 1, TomlValue::Array(array)));
        }
    }
}

/// `{k = v, ...}`: single-line, with its own namespace flags since dotted keys
/// inside it may not reopen an inline container.
fn parse_inline_table(
    src: &str,
    pos: usize,
    depth: usize,
    tracker: &ResourceTracker,
) -> ParseResult<(usize, TomlValue)> {
    let bytes = src.as_bytes();
    let mut table = TomlTable::default();
    let mut flags = Flags::default();
    let mut pos = skip_chars(bytes, pos + 1, is_ws);
    if bytes.get(pos) == Some(&b'}') {
        return Ok((pos + 1, TomlValue::Table(table)));
    }
    // Counts pairs, not root entries: dotted keys grow the tree beneath one.
    let mut pairs = 0;
    loop {
        check_memory_time_every(tracker, pairs)?;
        pairs += 1;
        let (next, key, value) = parse_key_value_pair(src, pos, depth, tracker)?;
        pos = next;
        let (key_stem, key_parent) = key.split_last().expect("parse_key yields at least one part");
        if flags.is(&key, FROZEN) {
            return decode_error(format!("Cannot mutate immutable namespace {}", format_key(&key)), pos);
        }
        let Ok(nest) = table.get_or_create_nest(key_parent, false, tracker)? else {
            return decode_error("Cannot overwrite a value", pos);
        };
        if nest.0.contains_key(key_stem) {
            return decode_error(format!("Duplicate inline table key {}", repr_str(key_stem)), pos);
        }
        let is_container = value.is_container();
        nest.insert(key_stem.clone(), value, tracker)?;
        pos = skip_chars(bytes, pos, is_ws);
        match bytes.get(pos) {
            Some(b'}') => return Ok((pos + 1, TomlValue::Table(table))),
            Some(b',') => {}
            _ => return decode_error("Unclosed inline table", pos),
        }
        if is_container {
            flags.set(&key, FROZEN, true, tracker)?;
        }
        pos = skip_chars(bytes, pos + 1, is_ws);
    }
}

/// `'...'`: no escapes, ends at the next apostrophe.
fn parse_literal_str(src: &str, pos: usize) -> ParseResult<(usize, String)> {
    let start = pos + 1;
    let end = skip_until(src.as_bytes(), start, b"'", is_illegal_literal_str_char, true)?;
    Ok((end + 1, src[start..end].to_owned()))
}

/// `"""..."""` or `'''...'''`, with the newline right after the opening
/// delimiter trimmed and up to two extra closing quotes kept as content.
fn parse_multiline_str(src: &str, pos: usize, literal: bool) -> ParseResult<(usize, String)> {
    let bytes = src.as_bytes();
    let mut pos = pos + 3;
    if bytes.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    let (mut pos, mut result, delim) = if literal {
        let end = skip_until(bytes, pos, b"'''", is_illegal_multiline_literal_str_char, true)?;
        (end + 3, src[pos..end].to_owned(), b'\'')
    } else {
        let (end, result) = parse_basic_str(src, pos, true)?;
        (end, result, b'"')
    };
    // A closing sequence of four or five quotes leaves one or two in the value.
    for _ in 0..2 {
        if bytes.get(pos) != Some(&delim) {
            break;
        }
        pos += 1;
        result.push(char::from(delim));
    }
    Ok((pos, result))
}

/// The body of a `"..."` or `"""..."""` string, `pos` just past the opening
/// quote(s); resolves escapes and rejects control characters.
fn parse_basic_str(src: &str, pos: usize, multiline: bool) -> ParseResult<(usize, String)> {
    let bytes = src.as_bytes();
    let is_illegal = if multiline {
        is_illegal_multiline_basic_str_char
    } else {
        is_illegal_basic_str_char
    };
    let mut result = String::new();
    let mut start = pos;
    let mut pos = pos;
    loop {
        let Some(&byte) = bytes.get(pos) else {
            return decode_error("Unterminated string", pos);
        };
        match byte {
            b'"' => {
                if !multiline {
                    result.push_str(&src[start..pos]);
                    return Ok((pos + 1, result));
                }
                if bytes[pos..].starts_with(b"\"\"\"") {
                    result.push_str(&src[start..pos]);
                    return Ok((pos + 3, result));
                }
                pos += 1;
            }
            b'\\' => {
                result.push_str(&src[start..pos]);
                let (next, escaped) = parse_basic_str_escape(src, pos, multiline)?;
                result.extend(escaped);
                pos = next;
                start = pos;
            }
            byte if is_illegal(byte) => {
                return decode_error(format!("Illegal character {}", repr_ctrl_char(byte)), pos);
            }
            _ => pos += 1,
        }
    }
}

/// One backslash escape at `pos`; `None` is the multiline line-ending
/// backslash, which contributes nothing.
fn parse_basic_str_escape(src: &str, pos: usize, multiline: bool) -> ParseResult<(usize, Option<char>)> {
    let bytes = src.as_bytes();
    // CPython's `pos += 2` steps over two characters: past the end for a lone
    // trailing backslash, and never into the middle of a multibyte one.
    let escaped_len = src[pos + 1..].chars().next().map_or(1, char::len_utf8);
    let escape = &bytes[pos..(pos + 1 + escaped_len).min(bytes.len())];
    let mut pos = pos + 1 + escaped_len;
    if multiline && matches!(escape, b"\\ " | b"\\\t" | b"\\\n") {
        // Skip whitespace until the next non-whitespace character or the end
        // of the document; anything but a newline before that is an error.
        if escape != b"\\\n" {
            pos = skip_chars(bytes, pos, is_ws);
            let Some(&next) = bytes.get(pos) else {
                return Ok((pos, None));
            };
            if next != b'\n' {
                return decode_error("Unescaped '\\' in a string", pos);
            }
            pos += 1;
        }
        return Ok((skip_chars(bytes, pos, is_ws_or_newline), None));
    }
    let replacement = match escape {
        b"\\u" => return parse_hex_char(src, pos, 4),
        b"\\U" => return parse_hex_char(src, pos, 8),
        b"\\b" => '\u{8}',
        b"\\t" => '\t',
        b"\\n" => '\n',
        b"\\f" => '\u{c}',
        b"\\r" => '\r',
        b"\\\"" => '"',
        b"\\\\" => '\\',
        _ => return decode_error("Unescaped '\\' in a string", pos),
    };
    Ok((pos, Some(replacement)))
}

/// The `hex_len` digits of a `\u` / `\U` escape, which must name a scalar value.
fn parse_hex_char(src: &str, pos: usize, hex_len: usize) -> ParseResult<(usize, Option<char>)> {
    let bytes = src.as_bytes();
    let hex = bytes
        .get(pos..pos + hex_len)
        .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit));
    let Some(hex) = hex else {
        return decode_error("Invalid hex value", pos);
    };
    let pos = pos + hex_len;
    let code = hex.iter().fold(0u32, |acc, &digit| {
        acc * 16 + char::from(digit).to_digit(16).expect("validated hex digit")
    });
    // `from_u32` rejects exactly the non-scalar values: surrogates and > U+10FFFF.
    match char::from_u32(code) {
        Some(c) => Ok((pos, Some(c))),
        None => decode_error("Escaped character is not a Unicode scalar value", pos),
    }
}

/// The number token matched by [`match_number`], as `float()` or `int(s, 0)`
/// would convert it (both accept the `_` separators).
fn parse_number(text: &str, is_float: bool, pos: usize) -> ParseResult<TomlValue> {
    // Allocates only when there are `_` separators to strip.
    let digits = if text.contains('_') {
        Cow::Owned(text.replace('_', ""))
    } else {
        Cow::Borrowed(text)
    };
    if is_float {
        return match digits.parse::<f64>() {
            Ok(value) => Ok(TomlValue::Float(value)),
            Err(_) => decode_error("Invalid value", pos),
        };
    }
    let (radix, body, negative) = if let Some(hex) = digits.strip_prefix("0x") {
        (16, hex, false)
    } else if let Some(octal) = digits.strip_prefix("0o") {
        (8, octal, false)
    } else if let Some(binary) = digits.strip_prefix("0b") {
        (2, binary, false)
    } else if let Some(magnitude) = digits.strip_prefix('-') {
        (10, magnitude, true)
    } else {
        (10, digits.strip_prefix('+').unwrap_or(&digits), false)
    };
    if radix == 10 {
        // `int()` refuses decimal literals past `sys.int_info.str_digits_check_threshold`.
        check_decimal_digit_count(body.len()).map_err(|error| TomlError::Run(Box::new(error)))?;
    }
    // Most literals fit `i64`; the rest (and `i64::MIN`'s magnitude) go through `BigInt`.
    let int = if let Ok(magnitude) = i64::from_str_radix(body, radix) {
        TomlInt::Small(if negative { -magnitude } else { magnitude })
    } else {
        let magnitude = BigInt::parse_bytes(body.as_bytes(), radix).expect("match_number validated the digits");
        let value = if negative { -magnitude } else { magnitude };
        match i64::try_from(&value) {
            Ok(small) => TomlInt::Small(small),
            Err(_) => TomlInt::Big(value),
        }
    };
    Ok(TomlValue::Int(int))
}

/// A `RE_DATETIME` match: its end, the date, and the time with its UTC offset
/// in seconds when the document had them.
type DateTimeMatch = (usize, TomlDate, Option<(TomlTime, Option<i32>)>);

/// `RE_DATETIME` at `pos`: a date, optionally followed by a time and offset.
///
/// The time and offset groups are optional, so a date followed by something
/// that is almost a time still matches as a bare date, like the regex.
fn match_datetime(bytes: &[u8], pos: usize) -> Option<DateTimeMatch> {
    let year = bytes
        .get(pos..pos + 4)
        .filter(|digits| digits.iter().all(u8::is_ascii_digit))?
        .iter()
        .fold(0, |acc, &digit| acc * 10 + i32::from(digit - b'0'));
    if bytes.get(pos + 4) != Some(&b'-') {
        return None;
    }
    let month = match_digit_pair(bytes, pos + 5, |tens, ones| {
        (tens == 0 && ones >= 1) || (tens == 1 && ones <= 2)
    })?;
    if bytes.get(pos + 7) != Some(&b'-') {
        return None;
    }
    let day = match_digit_pair(bytes, pos + 8, |tens, ones| {
        (tens == 0 && ones >= 1) || tens == 1 || tens == 2 || (tens == 3 && ones <= 1)
    })?;
    let date = TomlDate { year, month, day };
    let date_end = pos + 10;

    if !matches!(bytes.get(date_end), Some(b'T' | b't' | b' ')) {
        return Some((date_end, date, None));
    }
    let Some((time_end, time)) = match_time(bytes, date_end + 1) else {
        return Some((date_end, date, None));
    };
    let (end, offset_seconds) = match bytes.get(time_end) {
        Some(b'Z' | b'z') => (time_end + 1, Some(0)),
        Some(&sign @ (b'+' | b'-')) => {
            let hours = match_digit_pair(bytes, time_end + 1, is_hour_pair);
            let colon = bytes.get(time_end + 3) == Some(&b':');
            let minutes = match_digit_pair(bytes, time_end + 4, |tens, _| tens <= 5);
            match (hours, colon, minutes) {
                (Some(hours), true, Some(minutes)) => {
                    let seconds = hours * 3600 + minutes * 60;
                    (time_end + 6, Some(if sign == b'-' { -seconds } else { seconds }))
                }
                _ => (time_end, None),
            }
        }
        _ => (time_end, None),
    };
    Some((end, date, Some((time, offset_seconds))))
}

/// `RE_LOCALTIME` at `pos`: `HH:MM:SS` with an optional fraction, of which the
/// first six digits are kept.
fn match_time(bytes: &[u8], pos: usize) -> Option<(usize, TomlTime)> {
    let hour = match_digit_pair(bytes, pos, is_hour_pair)?;
    if bytes.get(pos + 2) != Some(&b':') {
        return None;
    }
    let minute = match_digit_pair(bytes, pos + 3, |tens, _| tens <= 5)?;
    if bytes.get(pos + 5) != Some(&b':') {
        return None;
    }
    let second = match_digit_pair(bytes, pos + 6, |tens, _| tens <= 5)?;
    let mut end = pos + 8;
    let mut microsecond = 0;
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        let digits_start = end + 1;
        end = skip_chars(bytes, digits_start, |byte| byte.is_ascii_digit());
        let kept = &bytes[digits_start..(digits_start + 6).min(end)];
        let padding = 10_i32.pow(u32::try_from(6 - kept.len()).expect("at most six digits kept"));
        microsecond = kept.iter().fold(0, |acc, &digit| acc * 10 + i32::from(digit - b'0')) * padding;
    }
    Some((
        end,
        TomlTime {
            hour,
            minute,
            second,
            microsecond,
        },
    ))
}

/// `[01][0-9]|2[0-3]` as a (tens, ones) predicate.
fn is_hour_pair(tens: u8, ones: u8) -> bool {
    tens <= 1 || (tens == 2 && ones <= 3)
}

/// Two ASCII digits at `pos` whose values satisfy `valid`, as a number.
fn match_digit_pair(bytes: &[u8], pos: usize, valid: impl Fn(u8, u8) -> bool) -> Option<i32> {
    let tens = bytes.get(pos).filter(|byte| byte.is_ascii_digit())? - b'0';
    let ones = bytes.get(pos + 1).filter(|byte| byte.is_ascii_digit())? - b'0';
    valid(tens, ones).then(|| i32::from(tens) * 10 + i32::from(ones))
}

/// `RE_NUMBER` at `pos`: `(end, is_float)`, where `is_float` is CPython's
/// non-empty `floatpart` group.
fn match_number(bytes: &[u8], pos: usize) -> Option<(usize, bool)> {
    // Prefixed integers come first in the regex; without a digit after the
    // prefix the alternative fails and the plain `0` below matches instead.
    if bytes.get(pos) == Some(&b'0') {
        let radix_digit: Option<fn(u8) -> bool> = match bytes.get(pos + 1) {
            Some(b'x') => Some(|byte| byte.is_ascii_hexdigit()),
            Some(b'o') => Some(|byte| matches!(byte, b'0'..=b'7')),
            Some(b'b') => Some(|byte| matches!(byte, b'0' | b'1')),
            _ => None,
        };
        if let Some(is_digit) = radix_digit
            && let Some(end) = match_digit_run(bytes, pos + 2, is_digit)
        {
            return Some((end, false));
        }
    }
    let mut pos = pos;
    if matches!(bytes.get(pos), Some(b'+' | b'-')) {
        pos += 1;
    }
    let mut end = match bytes.get(pos)? {
        b'0' => pos + 1,
        b'1'..=b'9' => match_digit_run(bytes, pos, |byte| byte.is_ascii_digit())?,
        _ => return None,
    };
    let mut is_float = false;
    if bytes.get(end) == Some(&b'.')
        && let Some(fraction_end) = match_digit_run(bytes, end + 1, |byte| byte.is_ascii_digit())
    {
        end = fraction_end;
        is_float = true;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if let Some(exponent_end) = match_digit_run(bytes, exponent, |byte| byte.is_ascii_digit()) {
            end = exponent_end;
            is_float = true;
        }
    }
    Some((end, is_float))
}

/// `D(?:_?D)*` at `pos`: at least one digit, underscores only between digits.
fn match_digit_run(bytes: &[u8], pos: usize, is_digit: impl Fn(u8) -> bool) -> Option<usize> {
    if !bytes.get(pos).is_some_and(|&byte| is_digit(byte)) {
        return None;
    }
    let mut end = pos + 1;
    loop {
        match bytes.get(end) {
            Some(&byte) if is_digit(byte) => end += 1,
            Some(b'_') if bytes.get(end + 1).is_some_and(|&byte| is_digit(byte)) => end += 2,
            _ => return Some(end),
        }
    }
}

/// Advances past every byte satisfying `pred`.
fn skip_chars(bytes: &[u8], mut pos: usize, pred: impl Fn(u8) -> bool) -> usize {
    while bytes.get(pos).is_some_and(|&byte| pred(byte)) {
        pos += 1;
    }
    pos
}

/// Position of the next `expect` at or after `pos` (the end of input when
/// absent and `error_on_eof` is unset), rejecting any `error_on` byte before it.
fn skip_until(
    bytes: &[u8],
    pos: usize,
    expect: &[u8],
    error_on: impl Fn(u8) -> bool,
    error_on_eof: bool,
) -> ParseResult<usize> {
    let found = (pos..=bytes.len().saturating_sub(expect.len())).find(|&i| bytes[i..].starts_with(expect));
    let new_pos = match found {
        Some(found) => found,
        None if error_on_eof => return decode_error(format!("Expected {}", repr_delimiter(expect)), bytes.len()),
        None => bytes.len(),
    };
    match bytes[pos..new_pos].iter().position(|&byte| error_on(byte)) {
        Some(offset) => decode_error(
            format!("Found invalid character {}", repr_ctrl_char(bytes[pos + offset])),
            pos + offset,
        ),
        None => Ok(new_pos),
    }
}

/// Skips a `#` comment up to (not including) the newline.
fn skip_comment(bytes: &[u8], pos: usize) -> ParseResult<usize> {
    if bytes.get(pos) == Some(&b'#') {
        skip_until(bytes, pos + 1, b"\n", is_illegal_comment_char, false)
    } else {
        Ok(pos)
    }
}

/// Skips the whitespace, newlines and comments allowed between array elements.
fn skip_comments_and_array_ws(bytes: &[u8], mut pos: usize) -> ParseResult<usize> {
    loop {
        let before = pos;
        pos = skip_chars(bytes, pos, is_ws_or_newline);
        pos = skip_comment(bytes, pos)?;
        if pos == before {
            return Ok(pos);
        }
    }
}

/// `TOML_WS`.
fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

/// `TOML_WS_AND_NEWLINE`.
fn is_ws_or_newline(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n')
}

/// `BARE_KEY_CHARS`: ASCII letters, digits, `-` and `_`.
fn is_bare_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

/// `KEY_INITIAL_CHARS`: a bare key character or either quote.
fn is_key_initial_char(byte: u8) -> bool {
    is_bare_key_char(byte) || matches!(byte, b'"' | b'\'')
}

/// `ILLEGAL_BASIC_STR_CHARS`: the ASCII controls other than tab. Also the
/// literal-string and comment sets, which CPython defines identically.
fn is_illegal_basic_str_char(byte: u8) -> bool {
    (byte < 0x20 && byte != b'\t') || byte == 0x7f
}

/// `ILLEGAL_MULTILINE_BASIC_STR_CHARS`: the controls other than tab and newline.
fn is_illegal_multiline_basic_str_char(byte: u8) -> bool {
    is_illegal_basic_str_char(byte) && byte != b'\n'
}

/// `ILLEGAL_LITERAL_STR_CHARS`.
fn is_illegal_literal_str_char(byte: u8) -> bool {
    is_illegal_basic_str_char(byte)
}

/// `ILLEGAL_MULTILINE_LITERAL_STR_CHARS`.
fn is_illegal_multiline_literal_str_char(byte: u8) -> bool {
    is_illegal_multiline_basic_str_char(byte)
}

/// `ILLEGAL_COMMENT_CHARS`.
fn is_illegal_comment_char(byte: u8) -> bool {
    is_illegal_basic_str_char(byte)
}

/// Python's `repr()` of a one-character control string, the only characters
/// the `Illegal character` / `Found invalid character` messages report.
fn repr_ctrl_char(byte: u8) -> String {
    match byte {
        b'\n' => "'\\n'".to_owned(),
        b'\r' => "'\\r'".to_owned(),
        _ => format!("'\\x{byte:02x}'"),
    }
}

/// Python's `repr()` of a closing delimiter (`'` or `'''`), which it
/// double-quotes because of the apostrophes.
fn repr_delimiter(delimiter: &[u8]) -> String {
    format!("\"{}\"", String::from_utf8_lossy(delimiter))
}

/// Python's `repr()` of a `str`.
fn repr_str(s: &str) -> String {
    let mut out = String::new();
    string_repr_fmt(s, &mut out).expect("writing to a String cannot fail");
    out
}

/// Python's `repr()` of a key tuple: `('a',)` or `('a', 'b')`.
fn format_key(key: &[String]) -> String {
    let mut out = String::from("(");
    for (i, part) in key.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        string_repr_fmt(part, &mut out).expect("writing to a String cannot fail");
    }
    if key.len() == 1 {
        out.push(',');
    }
    out.push(')');
    out
}

impl TomlDate {
    /// Whether `datetime.date(year, month, day)` would accept these fields.
    fn is_valid(self) -> bool {
        (1..=9999).contains(&self.year)
            && NaiveDate::from_ymd_opt(
                self.year,
                u32::try_from(self.month).expect("regex yields 01..12"),
                u32::try_from(self.day).expect("regex yields 01..31"),
            )
            .is_some()
    }
}

/// Table lookup that could not be satisfied: a value stands where a table or
/// array of tables is needed (CPython's `KeyError` inside `NestedDict`).
struct NestError;

impl TomlTable {
    /// The table at `key`, creating empty tables on the way. With
    /// `access_lists`, an array of tables on the path stands for its last table.
    /// The outer error is a `MemoryError` from growing a table.
    fn get_or_create_nest(
        &mut self,
        key: &[String],
        access_lists: bool,
        tracker: &ResourceTracker,
    ) -> ParseResult<Result<&mut Self, NestError>> {
        let mut cont = self;
        for part in key {
            if !cont.0.contains_key(part) {
                cont.insert(part.clone(), TomlValue::Table(Self::default()), tracker)?;
            }
            cont = match cont.0.get_mut(part).expect("inserted above") {
                TomlValue::Table(table) => table,
                TomlValue::Array(items) if access_lists => match items.last_mut() {
                    Some(TomlValue::Table(table)) => table,
                    _ => return Ok(Err(NestError)),
                },
                _ => return Ok(Err(NestError)),
            };
        }
        Ok(Ok(cont))
    }

    /// Appends an empty table to the array at `key`, creating the array when absent.
    fn append_nest_to_list(&mut self, key: &[String], tracker: &ResourceTracker) -> ParseResult<Result<(), NestError>> {
        let (last, parents) = key.split_last().expect("parse_key yields at least one part");
        let Ok(cont) = self.get_or_create_nest(parents, true, tracker)? else {
            return Ok(Err(NestError));
        };
        Ok(match cont.0.get_mut(last) {
            Some(TomlValue::Array(items)) => {
                tracker.check_growth(items.len(), items.capacity(), mem::size_of::<TomlValue>())?;
                items.push(TomlValue::Table(Self::default()));
                Ok(())
            }
            Some(_) => Err(NestError),
            None => {
                let items = vec![TomlValue::Table(Self::default())];
                cont.insert(last.clone(), TomlValue::Array(items), tracker)?;
                Ok(())
            }
        })
    }

    /// Inserts a new key, preflighting the doubling of a full table: the poll
    /// in the element loops sees only what is already allocated.
    fn insert(&mut self, key: String, value: TomlValue, tracker: &ResourceTracker) -> ParseResult<()> {
        tracker.check_growth(self.0.len(), self.0.capacity(), TABLE_ENTRY_GROWTH)?;
        self.0.insert(key, value);
        Ok(())
    }
}

/// Namespace flags keyed by dotted path: which tables were created explicitly
/// and which inline containers are frozen (CPython's `Flags`).
#[derive(Default)]
struct Flags {
    root: HashMap<String, FlagNode>,
    /// Flags a dotted key/value pair sets on its path once the current
    /// `[table]` section ends.
    pending: Vec<(Key, u8)>,
}

/// One key part's flags: those on the part itself, those inherited by
/// everything beneath it, and the parts nested under it.
#[derive(Default)]
struct FlagNode {
    flags: u8,
    recursive_flags: u8,
    nested: HashMap<String, Self>,
}

impl Drop for FlagNode {
    /// Drops the subtree iteratively: it mirrors the table nesting, so it can
    /// be [`MAX_KEY_PARTS`] deep, whether the whole tree goes or `unset_all`
    /// removes one node.
    fn drop(&mut self) {
        let mut pending: Vec<Self> = self.nested.drain().map(|(_, node)| node).collect();
        while let Some(mut node) = pending.pop() {
            pending.extend(node.nested.drain().map(|(_, node)| node));
        }
    }
}

impl Flags {
    /// Queues a flag for [`Self::finalize_pending`], preflighting the queue's
    /// doubling: a section of many dotted keys grows it by one entry per part.
    fn add_pending(&mut self, key: Key, flag: u8, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        tracker.check_growth(self.pending.len(), self.pending.capacity(), mem::size_of::<(Key, u8)>())?;
        self.pending.push((key, flag));
        Ok(())
    }

    fn finalize_pending(&mut self, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        for (key, flag) in mem::take(&mut self.pending) {
            self.set(&key, flag, false, tracker)?;
        }
        Ok(())
    }

    /// Drops every flag at and beneath `key`.
    fn unset_all(&mut self, key: &[String]) {
        let Some((stem, parents)) = key.split_last() else {
            return;
        };
        let mut cont = &mut self.root;
        for part in parents {
            match cont.get_mut(part) {
                Some(node) => cont = &mut node.nested,
                None => return,
            }
        }
        cont.remove(stem);
    }

    fn set(
        &mut self,
        key: &[String],
        flag: u8,
        recursive: bool,
        tracker: &ResourceTracker,
    ) -> Result<(), ResourceError> {
        let Some((stem, parents)) = key.split_last() else {
            return Ok(());
        };
        let mut cont = &mut self.root;
        for part in parents {
            cont = &mut flag_node(cont, part, tracker)?.nested;
        }
        let node = flag_node(cont, stem, tracker)?;
        if recursive {
            node.recursive_flags |= flag;
        } else {
            node.flags |= flag;
        }
        Ok(())
    }

    /// Whether `key` carries `flag`, directly or inherited from an ancestor's
    /// recursive flags. The document root has no flags.
    fn is(&self, key: &[String], flag: u8) -> bool {
        let Some((stem, parents)) = key.split_last() else {
            return false;
        };
        let mut cont = &self.root;
        for part in parents {
            let Some(inner) = cont.get(part) else { return false };
            if inner.recursive_flags & flag != 0 {
                return true;
            }
            cont = &inner.nested;
        }
        cont.get(stem)
            .is_some_and(|node| (node.flags | node.recursive_flags) & flag != 0)
    }
}

/// The node for `part`, created if absent after preflighting the rehash a
/// full map would do; a document of many tables grows one map by thousands.
fn flag_node<'m>(
    map: &'m mut HashMap<String, FlagNode>,
    part: &str,
    tracker: &ResourceTracker,
) -> Result<&'m mut FlagNode, ResourceError> {
    tracker.check_growth(map.len(), map.capacity(), FLAG_ENTRY_GROWTH)?;
    Ok(map.entry(part.to_owned()).or_default())
}
