//! JSON parsing support for `json.loads()`.
//!
//! This module owns conversion from JSON bytes into Monty runtime values,
//! including CPython-compatible `JSONDecodeError` construction.

use std::{borrow::Cow, mem};

use jiter::{Jiter, JiterError, JiterErrorType, JsonErrorType, NumberAny, NumberInt, Peek};

use super::JsonStringCache;
use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{ContainsHeap, DropWithHeap, HeapData, HeapGuard, HeapReader},
    resource::{ResourceError, ResourceTracker},
    types::{
        Dict, List, LongInt, PyTrait,
        long_int::{check_decimal_digit_count, decimal_digit_count_ascii},
        str::allocate_string,
    },
    value::Value,
};

/// Internal error used while building Monty values from streamed JSON.
///
/// JSON syntax failures remain as raw `JiterError` values until
/// `parse_json_bytes()`, while ordinary VM/runtime failures still propagate
/// immediately without being misreported as `JSONDecodeError`.
enum JsonLoadError {
    Parse(JiterError),
    Run(RunError),
}

impl From<JiterError> for JsonLoadError {
    fn from(error: JiterError) -> Self {
        Self::Parse(error)
    }
}

impl From<RunError> for JsonLoadError {
    fn from(error: RunError) -> Self {
        Self::Run(error)
    }
}

impl From<ResourceError> for JsonLoadError {
    fn from(error: ResourceError) -> Self {
        Self::Run(error.into())
    }
}

/// Result type used internally while streaming JSON from `jiter`.
type ParseResult<T> = Result<T, JsonLoadError>;

/// Maximum JSON nesting depth accepted by `json.loads()`.
///
/// This mirrors the default recursion limit used by `jiter`'s `JsonValue`
/// parser so the lower-level iterator path preserves the same safety boundary.
const JSON_RECURSION_LIMIT: usize = 200;

/// Implements `json.loads(s)`.
///
/// The function accepts exactly one positional argument. Input may be `str` or
/// `bytes`; parsed JSON values are converted recursively into Monty `Value`s.
/// Unlike CPython, `NaN`, `Infinity`, and `-Infinity` are always accepted
/// (there is no `parse_constant` parameter).
///
/// CPython kwargs `cls`, `object_hook`, `parse_float`, `parse_int`,
/// `parse_constant`, and `object_pairs_hook` are intentionally unsupported
/// and will raise `TypeError` if passed.
pub(super) fn call_loads(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    if let Some((key, value)) = kwargs.into_iter().next() {
        defer_drop!(key, vm);
        defer_drop!(value, vm);
        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        pos.drop_with_heap(vm);
        return Err(ExcType::type_error_unexpected_keyword(
            "loads",
            keyword_name.as_str(vm.interns),
        ));
    }

    let Some(data) = pos.next() else {
        return Err(ExcType::type_error_missing_positional_with_names("loads", &["s"]));
    };
    if pos.len() != 0 {
        let actual = pos.len() + 1;
        data.drop_with_heap(vm);
        pos.drop_with_heap(vm);
        return Err(ExcType::type_error_too_many_positional("loads", 1, actual, 0));
    }

    let mut data_guard = HeapGuard::new(data, vm);
    let (data, vm) = data_guard.as_parts_mut();
    parse_json_input(data, vm)
}

/// Parses a `json.loads()` input value and converts it into a Monty value.
///
/// The parser works directly on the underlying byte slice. Decoded strings from
/// `jiter` are copied into Monty's heap immediately before any further parser
/// movement so borrowed tape-backed data never escapes.
fn parse_json_input(value: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let bytes: Cow<'_, [u8]> = match value {
        Value::InternString(string_id) => Cow::Borrowed(vm.interns.get_str(*string_id).as_bytes()),
        Value::InternBytes(bytes_id) => Cow::Borrowed(vm.interns.get_bytes(*bytes_id)),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(s) => Cow::Owned(s.as_str().as_bytes().to_vec()),
            HeapData::Bytes(b) => Cow::Owned(b.as_slice().to_vec()),
            _ => return Err(ExcType::json_loads_type_error(value.py_type(vm))),
        },
        _ => return Err(ExcType::json_loads_type_error(value.py_type(vm))),
    };
    parse_json_bytes(bytes.as_ref(), vm)
}

/// Parses raw JSON bytes using `jiter` and converts the result to a Monty value.
///
/// The VM's per-run string cache is temporarily extracted so that it can be
/// passed alongside the VM through the recursive parse functions without
/// borrow conflicts. It is always restored before returning.
///
/// Syntax errors are wrapped in `json.JSONDecodeError` using the same
/// line/column/character suffix as CPython.
fn parse_json_bytes(bytes: &[u8], vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let mut jiter = Jiter::new(bytes).with_allow_inf_nan();
    // Take the cache out of the VM so we can pass it alongside &mut VM
    // without conflicting borrows. `mem::take` leaves `Default` in its place.
    let mut cache = mem::take(&mut vm.json_string_cache);
    let result = parse_json_value(&mut jiter, 0, &mut cache, vm);
    // Always restore the cache before returning, regardless of success/failure.
    vm.json_string_cache = cache;
    let value = result.map_err(|error| match error {
        JsonLoadError::Parse(error) => json_number_out_of_range_to_run_error(&error, bytes)
            .unwrap_or_else(|| json_error_to_run_error(&error, &jiter, bytes)),
        JsonLoadError::Run(error) => error,
    })?;
    // The successfully parsed `value` must be dropped via `drop_with_heap` if
    // `finish()` detects trailing data — a plain `?` would leak its refcount.
    if let Err(error) = jiter.finish() {
        value.drop_with_heap(vm);
        return Err(json_error_to_run_error(&error, &jiter, bytes));
    }
    Ok(value)
}

/// Parses the next JSON value from a `Jiter` and converts it into a Monty value.
///
/// Strings are allocated immediately because `jiter` may reuse its internal
/// tape buffer on the next parser step.
fn parse_json_value(
    jiter: &mut Jiter<'_>,
    depth: usize,
    cache: &mut JsonStringCache,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ParseResult<Value> {
    let peek = jiter.peek()?;
    parse_json_value_from_peek(peek, jiter, depth, cache, vm)
}

/// Converts a peeked JSON token into a Monty value.
///
/// The caller provides the `Peek` so container iteration can avoid reparsing
/// the next token after `array_step()`.
fn parse_json_value_from_peek(
    peek: Peek,
    jiter: &mut Jiter<'_>,
    depth: usize,
    cache: &mut JsonStringCache,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ParseResult<Value> {
    match peek {
        Peek::Null => {
            jiter.known_null()?;
            Ok(Value::None)
        }
        Peek::True | Peek::False => jiter.known_bool(peek).map(Value::Bool).map_err(Into::into),
        Peek::String => allocate_cached_string(parse_json_string(jiter)?, cache, vm.heap),
        Peek::Array => parse_json_array(jiter, depth, cache, vm),
        Peek::Object => parse_json_object(jiter, depth, cache, vm),
        _ if peek.is_num() => parse_json_number(peek, jiter, vm),
        _ => Err(JsonLoadError::Parse(JiterError {
            error_type: JiterErrorType::JsonError(JsonErrorType::ExpectedSomeValue),
            index: jiter.current_index(),
        })),
    }
}

/// Allocates a string using the cache when eligible, falling back to direct
/// allocation for empty/single-char strings (already interned by
/// `allocate_string`).
fn allocate_cached_string(
    s: String,
    cache: &mut JsonStringCache,
    heap: &HeapReader<'_, impl ResourceTracker>,
) -> ParseResult<Value> {
    if s.len() < 2 {
        // Empty and single-char strings are interned by allocate_string.
        Ok(allocate_string(s, heap.heap())?)
    } else {
        Ok(cache.get_or_allocate(s, heap)?)
    }
}

/// Parses a JSON number into the corresponding Monty numeric value.
///
/// Integer tokens are counted directly from the source bytes so
/// `INT_MAX_STR_DIGITS` errors report the same digit count as CPython without
/// first allocating an oversized decimal string.
fn parse_json_number(
    peek: Peek,
    jiter: &mut Jiter<'_>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ParseResult<Value> {
    let start = jiter.current_index();
    match jiter.known_number(peek) {
        Ok(NumberAny::Int(NumberInt::Int(value))) => Ok(Value::Int(value)),
        Ok(NumberAny::Int(NumberInt::BigInt(value))) => {
            let digit_count = decimal_digit_count_ascii(jiter.slice_to_current(start));
            check_decimal_digit_count(digit_count).map_err(JsonLoadError::Run)?;
            Ok(LongInt::new(value).into_value(vm.heap)?)
        }
        Ok(NumberAny::Float(value)) => Ok(Value::Float(value)),
        Err(error) => Err(error.into()),
    }
}

/// Parses a JSON array and allocates the resulting Monty list directly.
///
/// Elements are converted one by one as the iterator advances, avoiding any
/// intermediate `JsonValue` tree.
fn parse_json_array(
    jiter: &mut Jiter<'_>,
    depth: usize,
    cache: &mut JsonStringCache,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ParseResult<Value> {
    check_json_recursion_limit(jiter, depth)?;

    let Some(mut next) = jiter.known_array()? else {
        let list_id = vm.heap.allocate(HeapData::List(List::new(Vec::new())))?;
        return Ok(Value::Ref(list_id));
    };

    let values = Vec::new();
    let mut values_guard = HeapGuard::new(values, vm);
    {
        let (values, vm) = values_guard.as_parts_mut();
        loop {
            values.push(parse_json_value_from_peek(next, jiter, depth + 1, cache, vm)?);
            let Some(array_peek) = jiter.array_step()? else {
                break;
            };
            next = array_peek;
        }
    }

    let values = values_guard.into_inner();
    let list_id = vm.heap.allocate(HeapData::List(List::new(values)))?;
    Ok(Value::Ref(list_id))
}

/// Parses a JSON string and immediately copies it into an owned `String`.
///
/// `Jiter` may reuse its internal tape for the next decoded string, so callers
/// must not hold onto the borrowed `&str` beyond the current parser step.
fn parse_json_string(jiter: &mut Jiter<'_>) -> ParseResult<String> {
    Ok(jiter.known_str().map(ToOwned::to_owned)?)
}

/// Parses a JSON object and allocates the resulting Monty dict directly.
///
/// Keys are allocated before parsing their values because `jiter` may reuse its
/// temporary string storage on the next parser step.
fn parse_json_object(
    jiter: &mut Jiter<'_>,
    depth: usize,
    cache: &mut JsonStringCache,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ParseResult<Value> {
    check_json_recursion_limit(jiter, depth)?;

    let Some(mut key) = parse_first_object_key(jiter)? else {
        let dict_id = vm.heap.allocate(HeapData::Dict(Dict::new()))?;
        return Ok(Value::Ref(dict_id));
    };

    let mut dict_guard = HeapGuard::new(Dict::new(), vm);
    {
        let (dict, vm) = dict_guard.as_parts_mut();
        loop {
            let key_value = allocate_cached_string(key, cache, vm.heap)?;
            let value = parse_json_value(jiter, depth + 1, cache, vm)?;
            if let Some(old_value) = dict.set_json_string_key(key_value, value, vm)? {
                old_value.drop_with_heap(vm);
            }

            let Some(next_key) = parse_next_object_key(jiter)? else {
                break;
            };
            key = next_key;
        }
    }

    let dict = dict_guard.into_inner();
    let dict_id = vm.heap.allocate(HeapData::Dict(dict))?;
    Ok(Value::Ref(dict_id))
}

/// Parses the first key of a JSON object and copies it into owned storage.
///
/// Returning `None` indicates an empty object.
fn parse_first_object_key(jiter: &mut Jiter<'_>) -> ParseResult<Option<String>> {
    Ok(jiter.known_object().map(|key| key.map(ToOwned::to_owned))?)
}

/// Parses the next key of a JSON object and copies it into owned storage.
///
/// Returning `None` indicates that the object has no more items.
fn parse_next_object_key(jiter: &mut Jiter<'_>) -> ParseResult<Option<String>> {
    Ok(jiter.next_key().map(|key| key.map(ToOwned::to_owned))?)
}

/// Rejects JSON that nests arrays/objects more deeply than Monty supports.
///
/// The limit matches `jiter`'s `JsonValue` parser so this lower-level iterator
/// implementation preserves the same stack-safety boundary.
fn check_json_recursion_limit(jiter: &Jiter<'_>, depth: usize) -> ParseResult<()> {
    if depth >= JSON_RECURSION_LIMIT {
        return Err(JsonLoadError::Parse(JiterError {
            error_type: JiterErrorType::JsonError(JsonErrorType::RecursionLimitExceeded),
            index: jiter.current_index(),
        }));
    }
    Ok(())
}

/// Converts `jiter`'s oversized-integer parse error into CPython's digit-limit
/// `ValueError` when the offending token is a decimal integer literal.
fn json_number_out_of_range_to_run_error(error: &JiterError, bytes: &[u8]) -> Option<RunError> {
    if error.error_type != JiterErrorType::JsonError(JsonErrorType::NumberOutOfRange) {
        return None;
    }

    let token = slice_json_number_around(bytes, error.index);
    if !is_json_integer_token(token) {
        return None;
    }

    let digit_count = decimal_digit_count_ascii(token);
    check_decimal_digit_count(digit_count).err()
}

/// Returns whether a raw JSON number token is an integer literal rather than a
/// float or exponent form.
fn is_json_integer_token(token: &[u8]) -> bool {
    !token.is_empty() && !token.contains(&b'.') && !token.contains(&b'e') && !token.contains(&b'E')
}

/// Returns the JSON number token that surrounds `index`.
///
/// `jiter` reports `NumberOutOfRange` at or just after the failing position, so
/// this scans outward to recover the original token for CPython-compatible
/// integer digit-limit handling.
fn slice_json_number_around(bytes: &[u8], index: usize) -> &[u8] {
    let mut start = index.min(bytes.len());
    while start > 0 && is_json_number_byte(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = index.min(bytes.len());
    while end < bytes.len() && is_json_number_byte(bytes[end]) {
        end += 1;
    }

    &bytes[start..end]
}

/// Returns whether `byte` can appear in a JSON number token.
fn is_json_number_byte(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E')
}

/// Converts a `jiter` parse error into `json.JSONDecodeError`.
///
/// `jiter` exposes the error byte index plus a helper for computing line and
/// column, which is enough to reproduce CPython's message suffix exactly.
fn json_error_to_run_error(error: &JiterError, jiter: &Jiter<'_>, bytes: &[u8]) -> RunError {
    let (message, index, column_offset) = match &error.error_type {
        JiterErrorType::JsonError(JsonErrorType::KeyMustBeAString) => (
            "Expecting property name enclosed in double quotes".to_owned(),
            error.index,
            0,
        ),
        JiterErrorType::JsonError(JsonErrorType::TrailingComma) => {
            let comma_index = find_trailing_comma_index(bytes, error.index).unwrap_or(error.index);
            let message = match bytes
                .get(comma_index.saturating_add(1)..)
                .and_then(|rest| rest.iter().copied().find(|byte| !byte.is_ascii_whitespace()))
            {
                Some(b'}') => "Illegal trailing comma before end of object",
                Some(b']') => "Illegal trailing comma before end of array",
                _ => "trailing comma",
            };
            (message.to_owned(), comma_index, 0)
        }
        JiterErrorType::JsonError(JsonErrorType::EofWhileParsingString) => (
            "Unterminated string starting at".to_owned(),
            find_unterminated_string_start(bytes, error.index).unwrap_or(error.index),
            0,
        ),
        JiterErrorType::JsonError(JsonErrorType::EofWhileParsingValue | JsonErrorType::ExpectedSomeValue) => {
            ("Expecting value".to_owned(), error.index, 0)
        }
        JiterErrorType::JsonError(JsonErrorType::ExpectedColon) => {
            ("Expecting ':' delimiter".to_owned(), error.index, 0)
        }
        JiterErrorType::JsonError(
            JsonErrorType::ExpectedListCommaOrEnd
            | JsonErrorType::ExpectedObjectCommaOrEnd
            | JsonErrorType::EofWhileParsingList
            | JsonErrorType::EofWhileParsingObject,
        ) => ("Expecting ',' delimiter".to_owned(), error.index, 1),
        JiterErrorType::JsonError(JsonErrorType::InvalidEscape) => {
            let escape_index = find_string_escape_start(bytes, error.index).unwrap_or(error.index);
            let is_unicode_escape = bytes.get(escape_index.saturating_add(1)) == Some(&b'u');
            let message = if is_unicode_escape {
                "Invalid \\uXXXX escape"
            } else {
                "Invalid \\escape"
            };
            let index = if is_unicode_escape {
                escape_index.saturating_add(1)
            } else {
                escape_index
            };
            (message.to_owned(), index, 0)
        }
        JiterErrorType::JsonError(JsonErrorType::TrailingCharacters) => ("Extra data".to_owned(), error.index, 0),
        JiterErrorType::JsonError(error_type) => (error_type.to_string(), error.index, 0),
        JiterErrorType::WrongType { .. } => (error.error_type.to_string(), error.index, 0),
    };
    let mut position = jiter.error_position(index);
    if position.column == 0 {
        position.column = 1;
    }
    position.column += column_offset;
    ExcType::json_decode_error(&message, position.line, position.column, index)
}

/// Finds the opening quote for an unterminated JSON string.
///
/// `jiter` reports the error at the EOF position, but CPython formats this
/// specific error using the location of the starting quote instead.
fn find_unterminated_string_start(bytes: &[u8], end_index: usize) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut string_start = None;

    for (index, byte) in bytes.iter().copied().enumerate().take(end_index) {
        if !in_string {
            if byte == b'"' {
                in_string = true;
                string_start = Some(index);
            }
            continue;
        }

        if escaped {
            escaped = false;
            continue;
        }

        match byte {
            b'\\' => escaped = true,
            b'"' => {
                in_string = false;
                string_start = None;
            }
            _ => {}
        }
    }

    if in_string { string_start } else { None }
}

/// Finds the comma that triggered a trailing-comma error.
///
/// `jiter` reports the error at the closing delimiter after any intervening
/// whitespace, while CPython points at the comma itself.
fn find_trailing_comma_index(bytes: &[u8], end_index: usize) -> Option<usize> {
    bytes
        .get(..end_index)?
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .filter(|&index| bytes[index] == b',')
}

/// Finds the start of the current JSON escape sequence.
///
/// CPython reports invalid escape errors at the `\` for generic escapes and at
/// the `u` for malformed `\uXXXX` escapes.
fn find_string_escape_start(bytes: &[u8], end_index: usize) -> Option<usize> {
    find_unterminated_string_start(bytes, end_index).and_then(|string_start| {
        bytes
            .get(string_start.saturating_add(1)..end_index)?
            .iter()
            .rposition(|byte| *byte == b'\\')
            .map(|relative_index| string_start + 1 + relative_index)
    })
}
