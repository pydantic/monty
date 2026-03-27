//! JSON serialization support for `json.dumps()`.
//!
//! This module owns encoder keyword parsing, CPython-compatible string/float
//! formatting, and recursive serialization of Monty values.

use std::{
    fmt::{Display, Write},
    mem,
};

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData, HeapGuard, HeapId, HeapReadOutput},
    intern::StaticStrings,
    resource::ResourceTracker,
    sorting::sort_indices,
    types::{PyTrait, long_int::check_bits_str_digits_limit, str::allocate_string},
    value::Value,
};

/// Serializer configuration derived from `json.dumps()` keyword arguments.
///
/// The struct stores only the subset of encoder configuration that this module
/// actually uses while serializing. Unsupported or not-yet-implemented kwargs
/// still raise during parsing so call sites do not silently lose behavior.
struct JsonDumpsConfig {
    indent: Option<String>,
    item_separator: String,
    key_separator: String,
    flags: u8,
}

impl Default for JsonDumpsConfig {
    /// Returns the CPython default `json.dumps()` configuration.
    ///
    /// Compact output uses `", "` between items and `": "` between keys and
    /// values, ASCII escaping is enabled, NaN and infinity are emitted as
    /// `NaN`/`Infinity`, and invalid dict keys raise immediately.
    fn default() -> Self {
        Self {
            indent: None,
            item_separator: ", ".to_owned(),
            key_separator: ": ".to_owned(),
            flags: Self::ENSURE_ASCII | Self::ALLOW_NAN,
        }
    }
}

impl JsonDumpsConfig {
    /// Bit flag storing the `sort_keys` option.
    const SORT_KEYS: u8 = 1 << 0;
    /// Bit flag storing the `ensure_ascii` option.
    const ENSURE_ASCII: u8 = 1 << 1;
    /// Bit flag storing the `allow_nan` option.
    const ALLOW_NAN: u8 = 1 << 2;
    /// Bit flag storing the `skipkeys` option.
    const SKIPKEYS: u8 = 1 << 3;

    /// Returns whether `sort_keys=True` is enabled.
    fn sort_keys(&self) -> bool {
        self.flags & Self::SORT_KEYS != 0
    }

    /// Returns whether non-ASCII characters must be escaped.
    fn ensure_ascii(&self) -> bool {
        self.flags & Self::ENSURE_ASCII != 0
    }

    /// Returns whether NaN and infinity may be emitted as JSON tokens.
    fn allow_nan(&self) -> bool {
        self.flags & Self::ALLOW_NAN != 0
    }

    /// Returns whether unsupported dict keys should be skipped.
    fn skipkeys(&self) -> bool {
        self.flags & Self::SKIPKEYS != 0
    }

    /// Parses `json.dumps()` keyword arguments into serializer configuration.
    ///
    /// Unsupported keyword names and not-yet-implemented CPython kwargs raise
    /// immediately so typos or dropped behavior do not go unnoticed.
    fn parse_kwargs(kwargs: KwargsValues, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Self> {
        let kwargs_iter = kwargs.into_iter();
        defer_drop_mut!(kwargs_iter, vm);

        let mut config = Self::default();
        let mut seen_indent = false;
        let mut seen_sort_keys = false;
        let mut seen_ensure_ascii = false;
        let mut seen_allow_nan = false;
        let mut seen_separators = false;
        let mut seen_skipkeys = false;

        for (key, value) in kwargs_iter {
            defer_drop!(key, vm);
            let Some(keyword_name) = key.as_either_str(vm.heap) else {
                value.drop_with_heap(vm);
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            let Some(keyword_static) = keyword_name.static_string() else {
                value.drop_with_heap(vm);
                return Err(ExcType::type_error_unexpected_keyword(
                    "dumps",
                    keyword_name.as_str(vm.interns),
                ));
            };

            match keyword_static {
                StaticStrings::Indent => {
                    if seen_indent {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "indent"));
                    }
                    seen_indent = true;
                    config.indent = parse_indent_value(value, vm)?;
                }
                StaticStrings::SortKeys => {
                    if seen_sort_keys {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "sort_keys"));
                    }
                    seen_sort_keys = true;
                    if value.py_bool(vm) {
                        config.flags |= Self::SORT_KEYS;
                    } else {
                        config.flags &= !Self::SORT_KEYS;
                    }
                    value.drop_with_heap(vm);
                }
                StaticStrings::EnsureAscii => {
                    if seen_ensure_ascii {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "ensure_ascii"));
                    }
                    seen_ensure_ascii = true;
                    if value.py_bool(vm) {
                        config.flags |= Self::ENSURE_ASCII;
                    } else {
                        config.flags &= !Self::ENSURE_ASCII;
                    }
                    value.drop_with_heap(vm);
                }
                StaticStrings::AllowNan => {
                    if seen_allow_nan {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "allow_nan"));
                    }
                    seen_allow_nan = true;
                    if value.py_bool(vm) {
                        config.flags |= Self::ALLOW_NAN;
                    } else {
                        config.flags &= !Self::ALLOW_NAN;
                    }
                    value.drop_with_heap(vm);
                }
                StaticStrings::Separators => {
                    if seen_separators {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "separators"));
                    }
                    seen_separators = true;
                    if let Some((item, key)) = parse_separators_value(value, vm)? {
                        config.item_separator = item;
                        config.key_separator = key;
                    }
                }
                StaticStrings::Skipkeys => {
                    if seen_skipkeys {
                        value.drop_with_heap(vm);
                        return Err(ExcType::type_error_duplicate_arg("dumps", "skipkeys"));
                    }
                    seen_skipkeys = true;
                    if value.py_bool(vm) {
                        config.flags |= Self::SKIPKEYS;
                    } else {
                        config.flags &= !Self::SKIPKEYS;
                    }
                    value.drop_with_heap(vm);
                }
                _ => {
                    value.drop_with_heap(vm);
                    return Err(ExcType::type_error_unexpected_keyword(
                        "dumps",
                        vm.interns.get_str(keyword_static.into()),
                    ));
                }
            }
        }

        if config.indent.is_some() && !seen_separators {
            ",".clone_into(&mut config.item_separator);
            ": ".clone_into(&mut config.key_separator);
        }

        Ok(config)
    }
}

/// Implements `json.dumps(obj, **kwargs)`.
///
/// Only the first argument may be positional. Supported keyword arguments mirror
/// the high-value subset of CPython's encoder configuration. Unknown keywords
/// and not-yet-implemented options such as `cls`, `default`, and
/// `check_circular` raise immediately.
pub(super) fn call_dumps(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();

    let Some(obj) = pos.next() else {
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_missing_positional_with_names("dumps", &["obj"]));
    };
    if pos.len() != 0 {
        let actual = pos.len() + 1;
        obj.drop_with_heap(vm);
        pos.drop_with_heap(vm);
        kwargs.drop_with_heap(vm);
        return Err(ExcType::type_error_too_many_positional("dumps", 1, actual, 0));
    }

    let mut obj_guard = HeapGuard::new(obj, vm);
    let config = JsonDumpsConfig::parse_kwargs(kwargs, obj_guard.heap())?;

    let mut output = String::new();
    let mut active_containers = Vec::new();
    {
        let (obj, vm) = obj_guard.as_parts_mut();
        serialize_value(obj, &mut output, &config, 0, &mut active_containers, vm)?;
    }

    let (obj, vm) = obj_guard.into_parts();
    obj.drop_with_heap(vm);
    allocate_string(output, vm.heap)
}

/// Parses the `indent=` value for `json.dumps()`.
///
/// `None` keeps compact mode, integers switch to pretty mode using that many
/// spaces per nesting level (with negative values behaving like zero), and
/// strings are repeated once per depth level exactly like CPython.
fn parse_indent_value(value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<String>> {
    let mut value_guard = HeapGuard::new(value, vm);
    let (value, vm) = value_guard.as_parts_mut();

    match value {
        Value::None => Ok(None),
        Value::Bool(flag) => Ok(Some(" ".repeat(usize::from(*flag)))),
        Value::Int(count) => spaces_from_indent_count(*count),
        Value::InternString(string_id) => Ok(Some(vm.interns.get_str(*string_id).to_owned())),
        Value::Ref(heap_id) => match vm.heap.read(*heap_id) {
            HeapReadOutput::Str(string) => Ok(Some(string.get(vm.heap).as_str().to_owned())),
            HeapReadOutput::LongInt(long_int) => spaces_from_indent_count(
                long_int
                    .get(vm.heap)
                    .to_i64()
                    .ok_or_else(ExcType::overflow_shift_count)?,
            ),
            _ => Err(ExcType::type_error("indent must be None, an integer or a string")),
        },
        _ => Err(ExcType::type_error("indent must be None, an integer or a string")),
    }
}

/// Converts an integer indent width into the repeated-space string used per level.
///
/// Negative values behave like zero, which matches CPython's newline-only pretty
/// printer behavior for `indent=0` and `indent<0`.
fn spaces_from_indent_count(count: i64) -> RunResult<Option<String>> {
    if count <= 0 {
        Ok(None)
    } else {
        match usize::try_from(count) {
            Ok(count) => Ok(Some(" ".repeat(count))),
            Err(_) => Err(ExcType::overflow_shift_count()),
        }
    }
}

/// Parses the `separators=` value for `json.dumps()`.
///
/// `None` leaves the default separators intact. Otherwise the value must be a
/// two-item list or tuple of strings representing the item and key separators.
fn parse_separators_value(
    value: Value,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Option<(String, String)>> {
    let mut value_guard = HeapGuard::new(value, vm);
    let (value, vm) = value_guard.as_parts_mut();

    if matches!(value, Value::None) {
        return Ok(None);
    }

    let pair = match value {
        Value::Ref(heap_id) => match vm.heap.read(*heap_id) {
            HeapReadOutput::Tuple(tuple) => {
                let items = tuple.get(vm.heap).as_slice();
                if items.len() != 2 {
                    return Err(ExcType::type_error("separators must be a sequence of length 2"));
                }
                (
                    json_string_value_to_owned(&items[0], vm)?,
                    json_string_value_to_owned(&items[1], vm)?,
                )
            }
            HeapReadOutput::List(list) => {
                let items = list.get(vm.heap).as_slice();
                if items.len() != 2 {
                    return Err(ExcType::type_error("separators must be a sequence of length 2"));
                }
                (
                    json_string_value_to_owned(&items[0], vm)?,
                    json_string_value_to_owned(&items[1], vm)?,
                )
            }
            _ => return Err(ExcType::type_error("separators must be a sequence of length 2")),
        },
        _ => return Err(ExcType::type_error("separators must be a sequence of length 2")),
    };

    Ok(Some(pair))
}

/// Converts a Monty string value into an owned Rust `String`.
///
/// This helper accepts only Python strings because JSON separator configuration
/// is string-based in CPython as well.
fn json_string_value_to_owned(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<String> {
    match value {
        Value::InternString(string_id) => Ok(vm.interns.get_str(*string_id).to_owned()),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(string) => Ok(string.as_str().to_owned()),
            _ => Err(ExcType::type_error(format!(
                "expected string separator, not {}",
                value.py_type(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "expected string separator, not {}",
            value.py_type(vm)
        ))),
    }
}

/// Serializes a Monty value into JSON text.
///
/// The function handles immediate primitives directly and delegates to
/// heap-specific helpers for strings, long integers, lists, tuples, and dicts.
fn serialize_value(
    value: &Value,
    out: &mut String,
    config: &JsonDumpsConfig,
    depth: usize,
    active_containers: &mut Vec<HeapId>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    match value {
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
        Value::Int(value) => {
            write!(out, "{value}").expect("writing to String cannot fail");
            Ok(())
        }
        Value::Float(value) => serialize_float(*value, out, config),
        Value::InternString(string_id) => {
            write_json_string(vm.interns.get_str(*string_id), out, config.ensure_ascii());
            Ok(())
        }
        Value::InternLongInt(long_int_id) => {
            let value = vm.interns.get_long_int(*long_int_id);
            check_bits_str_digits_limit(value.bits())?;
            write!(out, "{value}").expect("writing to String cannot fail");
            Ok(())
        }
        Value::Ref(heap_id) => match vm.heap.read(*heap_id) {
            HeapReadOutput::Str(string) => {
                write_json_string(string.get(vm.heap).as_str(), out, config.ensure_ascii());
                Ok(())
            }
            HeapReadOutput::LongInt(long_int) => {
                long_int.get(vm.heap).check_str_digits_limit()?;
                write!(out, "{}", long_int.get(vm.heap).inner()).expect("writing to String cannot fail");
                Ok(())
            }
            HeapReadOutput::List(list) => {
                let items: Vec<Value> = list
                    .get(vm.heap)
                    .as_slice()
                    .iter()
                    .map(|value| value.clone_with_heap(vm))
                    .collect();
                let mut items_guard = HeapGuard::new(items, vm);
                let (items, vm) = items_guard.as_parts_mut();
                with_entered_container(active_containers, *heap_id, |active_containers| {
                    serialize_sequence(items.as_slice(), out, config, depth, active_containers, vm)
                })
            }
            HeapReadOutput::Tuple(tuple) => {
                let items: Vec<Value> = tuple
                    .get(vm.heap)
                    .as_slice()
                    .iter()
                    .map(|value| value.clone_with_heap(vm))
                    .collect();
                let mut items_guard = HeapGuard::new(items, vm);
                let (items, vm) = items_guard.as_parts_mut();
                with_entered_container(active_containers, *heap_id, |active_containers| {
                    serialize_sequence(items.as_slice(), out, config, depth, active_containers, vm)
                })
            }
            HeapReadOutput::Dict(dict) => {
                let entries: Vec<(Value, Value)> = dict
                    .get(vm.heap)
                    .iter()
                    .map(|(key, value)| (key.clone_with_heap(vm), value.clone_with_heap(vm)))
                    .collect();
                let mut entries_guard = HeapGuard::new(entries, vm);
                let (entries, vm) = entries_guard.as_parts_mut();
                with_entered_container(active_containers, *heap_id, |active_containers| {
                    serialize_dict(entries, out, config, depth, active_containers, vm)
                })
            }
            _ => Err(ExcType::json_not_serializable_error(value.py_type(vm))),
        },
        _ => Err(ExcType::json_not_serializable_error(value.py_type(vm))),
    }
}

/// Serializes a list or tuple as a JSON array.
///
/// Sequence formatting is shared because JSON does not distinguish tuples from
/// lists, but circular-reference tracking still happens at the container level
/// before this helper is called.
fn serialize_sequence(
    items: &[Value],
    out: &mut String,
    config: &JsonDumpsConfig,
    depth: usize,
    active_containers: &mut Vec<HeapId>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    out.push('[');
    if items.is_empty() {
        out.push(']');
        return Ok(());
    }

    let pretty = config.indent.is_some();
    for (index, item) in items.iter().enumerate() {
        if index != 0 {
            out.push_str(&config.item_separator);
        }
        if pretty {
            out.push('\n');
            write_indent(out, config, depth + 1);
        }
        serialize_value(item, out, config, depth + 1, active_containers, vm)?;
    }
    if pretty {
        out.push('\n');
        write_indent(out, config, depth);
    }
    out.push(']');
    Ok(())
}

/// Serializes a dict as a JSON object.
///
/// Dict keys are validated and optionally skipped before serialization. When
/// `sort_keys=True`, entries are sorted using Python comparison semantics on the
/// original keys so mixed incomparable key types raise the same style of
/// `TypeError` as CPython.
fn serialize_dict(
    entries: &mut Vec<(Value, Value)>,
    out: &mut String,
    config: &JsonDumpsConfig,
    depth: usize,
    active_containers: &mut Vec<HeapId>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    if config.skipkeys() {
        entries.retain(|(key, _)| is_json_key_allowed(key, vm));
    } else if let Some((key, _)) = entries.iter().find(|(key, _)| !is_json_key_allowed(key, vm)) {
        return Err(ExcType::json_invalid_key_error(key.py_type(vm)));
    }

    if config.sort_keys() {
        sort_dict_entries(entries, vm)?;
    }

    out.push('{');
    if entries.is_empty() {
        out.push('}');
        return Ok(());
    }

    let pretty = config.indent.is_some();
    for (index, (key, value)) in entries.iter().enumerate() {
        if index != 0 {
            out.push_str(&config.item_separator);
        }
        if pretty {
            out.push('\n');
            write_indent(out, config, depth + 1);
        }
        write_json_key(key, out, config, vm)?;
        out.push_str(&config.key_separator);
        serialize_value(value, out, config, depth + 1, active_containers, vm)?;
    }
    if pretty {
        out.push('\n');
        write_indent(out, config, depth);
    }
    out.push('}');
    Ok(())
}

/// Sorts dict entries in-place using Python comparison semantics on the keys.
///
/// The implementation mirrors the error style used by `sorted()` and
/// `list.sort()`: when two keys are not orderable, it raises
/// `TypeError: '<' not supported between instances of ...`.
fn sort_dict_entries(entries: &mut Vec<(Value, Value)>, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<()> {
    let mut indices: Vec<usize> = (0..entries.len()).collect();
    let compare_values: Vec<Value> = entries.iter().map(|(key, _)| key.clone_with_heap(vm)).collect();
    let mut compare_values_guard = HeapGuard::new(compare_values, vm);
    let (compare_values, vm) = compare_values_guard.as_parts_mut();
    sort_indices(&mut indices, compare_values.as_slice(), false, vm)?;

    let mut ordered: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
    for index in indices {
        ordered.push((
            entries[index].0.clone_with_heap(vm),
            entries[index].1.clone_with_heap(vm),
        ));
    }

    let old_entries = mem::replace(entries, ordered);
    old_entries.drop_with_heap(vm);
    Ok(())
}

/// Returns whether a value is an allowed JSON object key type.
///
/// CPython accepts strings, integers, floats, booleans, and `None`, then
/// coerces the non-string cases to JSON strings during output.
fn is_json_key_allowed(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
    matches!(
        value,
        Value::None | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::InternString(_)
    ) || matches!(value, Value::Ref(heap_id) if matches!(vm.heap.get(*heap_id), HeapData::Str(_) | HeapData::LongInt(_)))
}

/// Serializes a dict key by applying CPython's JSON key coercions.
///
/// Non-string supported key types are rendered to their JSON string form first,
/// then escaped as a JSON string token.
fn write_json_key(
    key: &Value,
    out: &mut String,
    config: &JsonDumpsConfig,
    vm: &VM<'_, '_, impl ResourceTracker>,
) -> RunResult<()> {
    let ensure_ascii = config.ensure_ascii();
    match key {
        Value::None => write_json_ascii_key("null", out),
        Value::Bool(true) => write_json_ascii_key("true", out),
        Value::Bool(false) => write_json_ascii_key("false", out),
        Value::Int(value) => write_json_display_key(value, out),
        Value::Float(value) => {
            out.push('"');
            write_json_float_text(*value, out);
            out.push('"');
        }
        Value::InternString(string_id) => write_json_string(vm.interns.get_str(*string_id), out, ensure_ascii),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(string) => write_json_string(string.as_str(), out, ensure_ascii),
            HeapData::LongInt(long_int) => {
                long_int.check_str_digits_limit()?;
                write_json_display_key(long_int.inner(), out);
            }
            _ => return Err(ExcType::json_invalid_key_error(key.py_type(vm))),
        },
        _ => return Err(ExcType::json_invalid_key_error(key.py_type(vm))),
    }
    Ok(())
}

/// Writes an already-ASCII JSON object key without going through the string
/// escaper.
///
/// Coerced keys such as `None`, booleans, and numeric reprs are always ASCII
/// and require no escaping, so this avoids building intermediate `String`
/// values on the dict-key hot path.
fn write_json_ascii_key(value: &str, out: &mut String) {
    out.push('"');
    out.push_str(value);
    out.push('"');
}

/// Writes a displayable value as a quoted JSON object key.
///
/// The caller is responsible for ensuring the formatted output is ASCII-safe
/// and does not require JSON string escaping.
fn write_json_display_key(value: impl Display, out: &mut String) {
    out.push('"');
    write!(out, "{value}").expect("writing to String cannot fail");
    out.push('"');
}

/// Serializes a float using JSON's number and NaN rules.
///
/// Finite floats use the same formatting as Monty's Python `repr(float)`, which
/// already matches the `json` module requirement that whole-valued floats keep a
/// decimal point such as `1.0`.
fn serialize_float(value: f64, out: &mut String, config: &JsonDumpsConfig) -> RunResult<()> {
    if value.is_nan() {
        if config.allow_nan() {
            out.push_str("NaN");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("nan"))
        }
    } else if value == f64::INFINITY {
        if config.allow_nan() {
            out.push_str("Infinity");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("inf"))
        }
    } else if value == f64::NEG_INFINITY {
        if config.allow_nan() {
            out.push_str("-Infinity");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("-inf"))
        }
    } else {
        write_json_float_text(value, out);
        Ok(())
    }
}

/// Writes a finite float using Monty's CPython-compatible float repr rules.
///
/// This avoids allocating a temporary `String` when the caller already owns the
/// destination buffer.
fn write_json_float_text(value: f64, out: &mut String) {
    let start = out.len();
    write!(out, "{value}").expect("writing to String cannot fail");
    let wrote_decimal_or_exponent = out.as_bytes()[start..]
        .iter()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'));
    if !wrote_decimal_or_exponent {
        out.push_str(".0");
    }
}

/// Writes indentation for pretty-printed JSON output.
///
/// The `indent` string is repeated once for each nesting level, matching
/// CPython's behavior for both numeric and string indentation.
fn write_indent(out: &mut String, config: &JsonDumpsConfig, depth: usize) {
    if let Some(indent) = &config.indent {
        for _ in 0..depth {
            out.push_str(indent);
        }
    }
}

/// Runs a closure while a container is marked active for cycle detection.
///
/// The helper centralizes the push/pop bookkeeping so every serialization path
/// pops the container again regardless of whether recursive serialization
/// succeeds or returns early with an error.
fn with_entered_container<R>(
    stack: &mut Vec<HeapId>,
    heap_id: HeapId,
    f: impl FnOnce(&mut Vec<HeapId>) -> RunResult<R>,
) -> RunResult<R> {
    if stack.contains(&heap_id) {
        return Err(ExcType::json_circular_reference_error());
    }
    stack.push(heap_id);
    let result = f(stack);
    stack
        .pop()
        .expect("entered container missing from JSON serialization stack");
    result
}

/// Writes a Rust string as a JSON string token.
///
/// The writer escapes control characters, quotes, and backslashes in all modes.
/// When `ensure_ascii` is enabled, non-ASCII code points are emitted as `\uXXXX`
/// escapes using surrogate pairs for supplementary-plane characters.
fn write_json_string(value: &str, out: &mut String, ensure_ascii: bool) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{1F}' => {
                write!(out, "\\u{:04x}", ch as u32).expect("writing to String cannot fail");
            }
            ch if ensure_ascii && !ch.is_ascii() => write_json_escape_for_non_ascii(ch, out),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// Writes a non-ASCII character using JSON `\uXXXX` escapes.
///
/// Code points above `U+FFFF` are encoded as UTF-16 surrogate pairs to match
/// CPython's `ensure_ascii=True` behavior.
fn write_json_escape_for_non_ascii(ch: char, out: &mut String) {
    let code = ch as u32;
    if code <= 0xFFFF {
        write!(out, "\\u{code:04x}").expect("writing to String cannot fail");
    } else {
        let code = code - 0x1_0000;
        let high = 0xD800 + (code >> 10);
        let low = 0xDC00 + (code & 0x3FF);
        write!(out, "\\u{high:04x}\\u{low:04x}").expect("writing to String cannot fail");
    }
}
