//! Implementation of the `json` module.
//!
//! Provides a focused subset of Python's `json` module:
//! - `dumps(obj)`: Serialize supported Python values to JSON text
//! - `loads(s)`: Parse JSON text into Python values
//!
//! The implementation intentionally supports only the primary positional argument
//! for each function. Optional CPython parameters (`indent`, `sort_keys`, `default`,
//! `object_hook`, etc.) are not implemented yet.

use std::{borrow::Cow, fmt::Write, str::FromStr};

use ahash::AHashSet;
use num_bigint::BigInt;
use serde_json::{Number as JsonNumber, Value as JsonValue};

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Dict, List, LongInt, Module, PyTrait, str::allocate_string},
    value::Value,
};

/// `json` module functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum JsonFunctions {
    Dumps,
    Loads,
}

/// Creates the `json` module and allocates it on the heap.
///
/// The module exports `dumps` and `loads`.
///
/// # Returns
/// A HeapId pointing to the newly allocated module.
///
/// # Panics
/// Panics if required strings were not pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Json);

    module.set_attr(
        StaticStrings::Dumps,
        Value::ModuleFunction(ModuleFunctions::Json(JsonFunctions::Dumps)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Loads,
        Value::ModuleFunction(ModuleFunctions::Json(JsonFunctions::Loads)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Dispatches calls to `json` module functions.
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    functions: JsonFunctions,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<AttrCallResult> {
    match functions {
        JsonFunctions::Dumps => dumps(heap, args, interns).map(AttrCallResult::Value),
        JsonFunctions::Loads => loads(heap, args, interns).map(AttrCallResult::Value),
    }
}

/// Implements `json.dumps(obj)`.
///
/// Supported value mappings:
/// - `None` -> `null`
/// - `bool` -> `true` / `false`
/// - `int` / `LongInt` -> JSON number
/// - `float` -> JSON number with CPython-compatible special values (`NaN`, `Infinity`)
/// - `str` -> JSON string (with `ensure_ascii=True` behavior)
/// - `list` / `tuple` -> JSON array
/// - `dict` -> JSON object (restricted key types)
///
/// Unsupported types raise `TypeError`.
/// Circular references in list/tuple/dict raise `ValueError`.
fn dumps(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let value = args.get_one_arg("json.dumps", heap)?;
    defer_drop!(value, heap);

    let mut output = String::new();
    let mut visiting = AHashSet::new();
    write_json_value(value, &mut output, &mut visiting, heap, interns)?;

    allocate_string(output, heap)
}

/// Implements `json.loads(s)`.
///
/// Accepts `str` and `bytes` input. Bytes are decoded as UTF-8.
/// Invalid JSON raises `ValueError` with JSON decode details.
fn loads(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let source = args.get_one_arg("json.loads", heap)?;
    defer_drop!(source, heap);

    let input = extract_json_input(source, heap, interns)?;
    let parsed: JsonValue = serde_json::from_str(&input)
        .map_err(|err| ExcType::value_error_json_decode(format_json_decode_error(&input, &err)))?;

    json_value_to_monty(parsed, heap, interns)
}

/// Extracts JSON text from `json.loads` input.
///
/// Supports Python `str` and `bytes`.
fn extract_json_input<'a>(
    source: &'a Value,
    heap: &'a Heap<impl ResourceTracker>,
    interns: &'a Interns,
) -> RunResult<Cow<'a, str>> {
    match source {
        Value::InternString(id) => Ok(Cow::Borrowed(interns.get_str(*id))),
        Value::InternBytes(id) => {
            let bytes = interns.get_bytes(*id);
            std::str::from_utf8(bytes)
                .map(Cow::Borrowed)
                .map_err(|_| ExcType::unicode_decode_error_invalid_utf8())
        }
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Ok(Cow::Borrowed(s.as_str())),
            HeapData::Bytes(bytes) => std::str::from_utf8(bytes.as_slice())
                .map(Cow::Borrowed)
                .map_err(|_| ExcType::unicode_decode_error_invalid_utf8()),
            _ => Err(ExcType::type_error_json_loads_input(source.py_type(heap))),
        },
        _ => Err(ExcType::type_error_json_loads_input(source.py_type(heap))),
    }
}

/// Converts a parsed `serde_json::Value` into a Monty runtime `Value`.
fn json_value_to_monty(value: JsonValue, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    match value {
        JsonValue::Null => Ok(Value::None),
        JsonValue::Bool(b) => Ok(Value::Bool(b)),
        JsonValue::Number(number) => json_number_to_monty(&number, heap),
        JsonValue::String(s) => allocate_string(s, heap),
        JsonValue::Array(items) => {
            let token = heap.incr_recursion_depth()?;
            defer_drop!(token, heap);

            let mut values = Vec::with_capacity(items.len());
            for (idx, item) in items.into_iter().enumerate() {
                if idx.is_multiple_of(32) {
                    heap.check_time()?;
                }
                values.push(json_value_to_monty(item, heap, interns)?);
            }
            let list_id = heap.allocate(HeapData::List(List::new(values)))?;
            Ok(Value::Ref(list_id))
        }
        JsonValue::Object(map) => {
            let token = heap.incr_recursion_depth()?;
            defer_drop!(token, heap);

            let mut pairs = Vec::with_capacity(map.len());
            for (idx, (key, value)) in map.into_iter().enumerate() {
                if idx.is_multiple_of(32) {
                    heap.check_time()?;
                }
                let key_value = allocate_string(key, heap)?;
                let value_value = json_value_to_monty(value, heap, interns)?;
                pairs.push((key_value, value_value));
            }
            let dict = Dict::from_pairs(pairs, heap, interns)?;
            let dict_id = heap.allocate(HeapData::Dict(dict))?;
            Ok(Value::Ref(dict_id))
        }
    }
}

/// Converts a JSON number into a Monty numeric value.
///
/// Prefers integers when representable, including `LongInt` for values exceeding i64.
fn json_number_to_monty(number: &JsonNumber, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if let Some(i) = number.as_i64() {
        return Ok(Value::Int(i));
    }
    if let Some(u) = number.as_u64() {
        if let Ok(i) = i64::try_from(u) {
            return Ok(Value::Int(i));
        }
        return Ok(LongInt::new(BigInt::from(u)).into_value(heap)?);
    }
    if let Some(f) = number.as_f64() {
        return Ok(Value::Float(f));
    }

    // Fallback for uncommon number representations.
    let raw = number.to_string();
    if let Ok(big) = BigInt::from_str(&raw) {
        return Ok(LongInt::new(big).into_value(heap)?);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return Ok(Value::Float(f));
    }
    Err(RunError::internal(format!(
        "unrecognized JSON number representation: {raw}"
    )))
}

/// Writes a JSON representation of a Monty value to `out`.
///
/// Tracks container HeapIds in `visiting` for cycle detection.
fn write_json_value(
    value: &Value,
    out: &mut String,
    visiting: &mut AHashSet<HeapId>,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    let token = heap.incr_recursion_depth()?;
    crate::defer_drop_immutable_heap!(token, heap);

    match value {
        Value::Undefined => Err(RunError::internal("undefined value encountered in json.dumps")),
        Value::Ellipsis => Err(ExcType::type_error_json_not_serializable(value.py_type(heap))),
        Value::None => {
            out.push_str("null");
            Ok(())
        }
        Value::Bool(true) => {
            out.push_str("true");
            Ok(())
        }
        Value::Bool(false) => {
            out.push_str("false");
            Ok(())
        }
        Value::Int(i) => {
            write!(out, "{i}").expect("writing to String should not fail");
            Ok(())
        }
        Value::Float(f) => {
            out.push_str(&format_json_float(*f));
            Ok(())
        }
        Value::InternLongInt(id) => {
            write!(out, "{}", interns.get_long_int(*id)).expect("writing to String should not fail");
            Ok(())
        }
        Value::InternString(id) => {
            write_json_string(interns.get_str(*id), out);
            Ok(())
        }
        Value::Ref(id) => write_heap_json(*id, out, visiting, heap, interns),
        Value::InternBytes(_)
        | Value::Builtin(_)
        | Value::ModuleFunction(_)
        | Value::DefFunction(_)
        | Value::ExtFunction(_)
        | Value::Marker(_)
        | Value::Property(_)
        | Value::ExternalFuture(_) => Err(ExcType::type_error_json_not_serializable(value.py_type(heap))),
        #[cfg(feature = "ref-count-panic")]
        Value::Dereferenced => Err(RunError::internal("dereferenced value encountered in json.dumps")),
    }
}

/// Writes a JSON representation for heap-allocated values.
fn write_heap_json(
    id: HeapId,
    out: &mut String,
    visiting: &mut AHashSet<HeapId>,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    match heap.get(id) {
        HeapData::Str(s) => {
            write_json_string(s.as_str(), out);
            Ok(())
        }
        HeapData::LongInt(li) => {
            write!(out, "{}", li.inner()).expect("writing to String should not fail");
            Ok(())
        }
        HeapData::List(list) => write_json_sequence(id, list.as_slice(), out, visiting, heap, interns),
        HeapData::Tuple(tuple) => write_json_sequence(id, tuple.as_slice(), out, visiting, heap, interns),
        HeapData::Dict(dict) => write_json_dict(id, dict, out, visiting, heap, interns),
        _ => Err(ExcType::type_error_json_not_serializable(heap.get(id).py_type(heap))),
    }
}

/// Writes a JSON array from a list/tuple-like sequence.
fn write_json_sequence(
    id: HeapId,
    items: &[Value],
    out: &mut String,
    visiting: &mut AHashSet<HeapId>,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    if !visiting.insert(id) {
        return Err(ExcType::value_error_json_circular_reference());
    }

    let result = (|| {
        out.push('[');
        let mut first = true;
        for item in items {
            heap.check_time()?;
            if first {
                first = false;
            } else {
                out.push_str(", ");
            }
            write_json_value(item, out, visiting, heap, interns)?;
        }
        out.push(']');
        Ok(())
    })();

    visiting.remove(&id);
    result
}

/// Writes a JSON object from a Python dict.
fn write_json_dict(
    id: HeapId,
    dict: &Dict,
    out: &mut String,
    visiting: &mut AHashSet<HeapId>,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    if !visiting.insert(id) {
        return Err(ExcType::value_error_json_circular_reference());
    }

    let result = (|| {
        out.push('{');
        let mut first = true;
        for (key, value) in dict.items() {
            heap.check_time()?;
            if first {
                first = false;
            } else {
                out.push_str(", ");
            }

            write_json_object_key(key, out, heap, interns)?;
            out.push_str(": ");
            write_json_value(value, out, visiting, heap, interns)?;
        }
        out.push('}');
        Ok(())
    })();

    visiting.remove(&id);
    result
}

/// Writes a JSON object key with CPython-compatible key coercion rules.
fn write_json_object_key(
    key: &Value,
    out: &mut String,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<()> {
    match key {
        Value::None => {
            write_json_string("null", out);
            Ok(())
        }
        Value::Bool(true) => {
            write_json_string("true", out);
            Ok(())
        }
        Value::Bool(false) => {
            write_json_string("false", out);
            Ok(())
        }
        Value::Int(i) => {
            write_json_string(&i.to_string(), out);
            Ok(())
        }
        Value::Float(f) => {
            write_json_string(&format_json_float(*f), out);
            Ok(())
        }
        Value::InternLongInt(id) => {
            write_json_string(&interns.get_long_int(*id).to_string(), out);
            Ok(())
        }
        Value::InternString(id) => {
            write_json_string(interns.get_str(*id), out);
            Ok(())
        }
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => {
                write_json_string(s.as_str(), out);
                Ok(())
            }
            HeapData::LongInt(li) => {
                write_json_string(&li.inner().to_string(), out);
                Ok(())
            }
            _ => Err(ExcType::type_error_json_invalid_key(key.py_type(heap))),
        },
        _ => Err(ExcType::type_error_json_invalid_key(key.py_type(heap))),
    }
}

/// Writes a JSON string with `ensure_ascii=True` escaping.
fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => {
                write!(out, "\\u{:04x}", c as u32).expect("writing to String should not fail");
            }
            c if c.is_ascii() => out.push(c),
            c => write_json_unicode_escape(c, out),
        }
    }
    out.push('"');
}

/// Writes `\uXXXX` (or surrogate-pair) escape sequences for a non-ASCII character.
fn write_json_unicode_escape(c: char, out: &mut String) {
    let code = c as u32;
    if code <= 0xFFFF {
        write!(out, "\\u{code:04x}").expect("writing to String should not fail");
        return;
    }

    // Represent non-BMP chars as UTF-16 surrogate pairs.
    let code = code - 0x1_0000;
    let high = 0xD800 + (code >> 10);
    let low = 0xDC00 + (code & 0x3FF);
    write!(out, "\\u{high:04x}\\u{low:04x}").expect("writing to String should not fail");
}

/// Formats a float using JSON/CPython-compatible special value names.
///
/// Finite values use Rust debug float formatting and normalized scientific exponent.
fn format_json_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }

    normalize_float_exponent(format!("{value:?}"))
}

/// Normalizes scientific notation exponent to CPython-style `e+NN` / `e-NN`.
fn normalize_float_exponent(formatted: String) -> String {
    let Some((mantissa, exp)) = formatted.split_once('e') else {
        return formatted;
    };

    let (sign, digits) = match exp.as_bytes().first().copied() {
        Some(b'+') => ("+", &exp[1..]),
        Some(b'-') => ("-", &exp[1..]),
        _ => ("+", exp),
    };
    let padded = if digits.len() >= 2 {
        digits.to_owned()
    } else {
        format!("0{digits}")
    };
    format!("{mantissa}e{sign}{padded}")
}

/// Formats a serde JSON parse error in CPython-like `JSONDecodeError` style.
fn format_json_decode_error(input: &str, error: &serde_json::Error) -> String {
    let raw = error.to_string();
    let message = raw.split_once(" at line ").map_or(raw.as_str(), |(msg, _)| msg).trim();
    let message = capitalize_first(message);

    let line = error.line();
    let column = error.column();
    let char_index = json_error_char_index(input, line, column);

    format!("{message}: line {line} column {column} (char {char_index})")
}

/// Capitalizes the first Unicode scalar in a message.
fn capitalize_first(message: &str) -> String {
    let mut chars = message.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut output = String::new();
    output.extend(first.to_uppercase());
    output.push_str(chars.as_str());
    output
}

/// Converts 1-based line/column to a 0-based byte index for decode errors.
fn json_error_char_index(input: &str, target_line: usize, target_column: usize) -> usize {
    if target_line == 0 || target_column == 0 {
        return 0;
    }

    let mut line = 1usize;
    let mut column = 1usize;

    for (byte_idx, c) in input.char_indices() {
        if line == target_line && column == target_column {
            return byte_idx;
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    input.len()
}
