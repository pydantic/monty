//! JSON serialization support for `json.dumps()`.
//!
//! This module owns encoder keyword parsing, CPython-compatible string/float
//! formatting, and recursive serialization of Monty values.

use std::{
    cmp::Ordering,
    fmt::{Display, Write},
};

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, HeapData, HeapGuard, HeapId, HeapReadOutput},
    intern::StaticStrings,
    resource::ResourceTracker,
    sorting::{apply_permutation, sort_indices},
    types::{PyTrait, long_int::check_bigint_str_digits_limit, str::allocate_string},
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
                    "JSONEncoder.__init__",
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
                    if let Some((item, key)) = parse_separators_value(value, vm)? {
                        config.item_separator = item;
                        config.key_separator = key;
                        seen_separators = true;
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
                        "JSONEncoder.__init__",
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
/// the high-value subset of CPython's encoder configuration: `indent`,
/// `sort_keys`, `ensure_ascii`, `allow_nan`, `separators`, and `skipkeys`.
///
/// CPython kwargs `cls`, `default`, and `check_circular` are intentionally
/// unsupported and will raise `TypeError` if passed.
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
/// spaces per nesting level (with zero and negative values enabling newline-
/// only pretty printing), and
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
/// Zero and negative values return an empty indent string, which keeps pretty
/// printing enabled while omitting leading spaces on each line like CPython.
fn spaces_from_indent_count(count: i64) -> RunResult<Option<String>> {
    if count <= 0 {
        Ok(Some(String::new()))
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
                check_separators_length(items.len())?;
                (
                    json_separator_to_string(&items[0], "item_separator", vm)?,
                    json_separator_to_string(&items[1], "key_separator", vm)?,
                )
            }
            HeapReadOutput::List(list) => {
                let items = list.get(vm.heap).as_slice();
                check_separators_length(items.len())?;
                (
                    json_separator_to_string(&items[0], "item_separator", vm)?,
                    json_separator_to_string(&items[1], "key_separator", vm)?,
                )
            }
            _ => {
                return Err(ExcType::type_error(format!(
                    "cannot unpack non-iterable {} object",
                    value.py_type(vm)
                )));
            }
        },
        _ => {
            return Err(ExcType::type_error(format!(
                "cannot unpack non-iterable {} object",
                value.py_type(vm)
            )));
        }
    };

    Ok(Some(pair))
}

/// Validates that the separators sequence has exactly two elements.
///
/// Raises `ValueError` with the same unpacking-style message as CPython when
/// the length does not match the expected two elements.
fn check_separators_length(len: usize) -> RunResult<()> {
    match len.cmp(&2) {
        Ordering::Greater => Err(ExcType::value_error(format!(
            "too many values to unpack (expected 2, got {len})"
        ))),
        Ordering::Less => Err(ExcType::value_error(format!(
            "not enough values to unpack (expected 2, got {len})"
        ))),
        Ordering::Equal => Ok(()),
    }
}

/// Converts a Monty value to a string for use as a JSON separator.
///
/// CPython's C encoder validates separators as strings and refers to them by
/// their positional argument index in `make_encoder()`. The `role` parameter
/// selects the matching CPython argument number (6 for `item_separator`,
/// 5 for `key_separator`) so the error message matches CPython exactly.
fn json_separator_to_string(value: &Value, role: &str, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<String> {
    let arg_num = if role == "item_separator" { 6 } else { 5 };
    match value {
        Value::InternString(string_id) => Ok(vm.interns.get_str(*string_id).to_owned()),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(string) => Ok(string.as_str().to_owned()),
            _ => Err(ExcType::type_error(format!(
                "make_encoder() argument {arg_num} must be str, not {}",
                value.py_type(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "make_encoder() argument {arg_num} must be str, not {}",
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
            check_bigint_str_digits_limit(value)?;
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
        skip_disallowed_dict_keys(entries, vm);
    } else if let Some((key, _)) = entries.iter().find(|(key, _)| !is_json_key_allowed(key, vm)) {
        return Err(ExcType::json_invalid_key_error(key.py_type(vm)));
    }

    if config.sort_keys() {
        sort_dict_entries(entries, vm)?;
    }

    out.push('{');

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
    if pretty && !entries.is_empty() {
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
    apply_permutation(entries.as_mut_slice(), &mut indices);
    Ok(())
}

/// Removes dict entries whose keys are not JSON-serializable, preserving order.
///
/// `skipkeys=True` must drop invalid entries without disturbing the relative
/// order of the retained pairs. A two-pointer compaction avoids the repeated
/// shifting cost of `Vec::remove(i)` while still cleaning up skipped `Value`
/// references with `drop_with_heap`.
fn skip_disallowed_dict_keys(entries: &mut Vec<(Value, Value)>, vm: &mut VM<'_, '_, impl ResourceTracker>) {
    let mut write = 0;
    for read in 0..entries.len() {
        if is_json_key_allowed(&entries[read].0, vm) {
            if write != read {
                entries.swap(write, read);
            }
            write += 1;
        }
    }

    for (key, value) in entries.drain(write..) {
        key.drop_with_heap(vm);
        value.drop_with_heap(vm);
    }
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
            serialize_float_key(*value, out, config)?;
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

/// Serializes a float value as a quoted JSON object key, respecting `allow_nan`.
///
/// Non-finite values (NaN, +/-Infinity) are emitted as their Python repr
/// (`NaN`, `Infinity`, `-Infinity`) when `allow_nan` is enabled. When disabled,
/// the same `ValueError` raised for non-finite float *values* applies to keys
/// too, matching CPython's behavior.
fn serialize_float_key(value: f64, out: &mut String, config: &JsonDumpsConfig) -> RunResult<()> {
    out.push('"');
    if value.is_nan() {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("nan"));
        }
        out.push_str("NaN");
    } else if value == f64::INFINITY {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("inf"));
        }
        out.push_str("Infinity");
    } else if value == f64::NEG_INFINITY {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("-inf"));
        }
        out.push_str("-Infinity");
    } else {
        write_json_float_text(value, out);
    }
    out.push('"');
    Ok(())
}

/// Serializes a float using JSON's number and NaN rules.
///
/// Finite floats use CPython-compatible `json` float formatting, including the
/// switch to exponent notation for very small or very large magnitudes while
/// still preserving a decimal point for whole-valued non-exponent outputs.
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

/// Writes a finite float using CPython-compatible JSON float repr rules.
///
/// Python switches to scientific notation when the magnitude is `>= 1e16` or
/// `< 1e-4` (and non-zero). We decide notation by comparing the absolute value
/// directly against these thresholds rather than using `log10().floor()`, which
/// has precision errors at boundary values (e.g. `9999999999999998.0` whose
/// `log10` rounds up to `16.0`). Direct comparison is exact because `1e16` is
/// exactly representable as f64 and `1e-4` as an f64 constant aligns with
/// Python's notation boundary.
///
/// Each path formats the float exactly once: the scientific path writes via
/// `{:e}` and post-processes the exponent to Python style (`e+XX` / `e-XX`),
/// while the fixed path uses `Display` with a `.0` suffix for whole numbers.
fn write_json_float_text(value: f64, out: &mut String) {
    let abs = value.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // Python-style scientific notation: format via `{:e}`, then rewrite the
        // exponent from Rust's bare `e<N>` to Python's `e+XX` / `e-XX`.
        let start = out.len();
        write!(out, "{value:e}").expect("writing to String cannot fail");
        let e_pos = out[start..].find('e').expect("scientific format must contain 'e'") + start;
        let exponent: i32 = out[e_pos + 1..].parse().expect("exponent must be a valid integer");
        out.truncate(e_pos);
        let exp_sign = if exponent >= 0 { '+' } else { '-' };
        write!(out, "e{exp_sign}{:02}", exponent.unsigned_abs()).expect("writing to String cannot fail");
    } else {
        // Fixed notation: single `Display` write, appending `.0` for whole numbers.
        let start = out.len();
        write!(out, "{value}").expect("writing to String cannot fail");
        if !out[start..].contains('.') {
            out.push_str(".0");
        }
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
/// Uses a byte-oriented batch strategy inspired by serde_json: a 256-entry
/// lookup table classifies each byte in O(1), and contiguous runs of safe bytes
/// are flushed with a single `push_str` rather than character-by-character.
///
/// When `ensure_ascii` is enabled, non-ASCII code points (bytes >= 0x80) are
/// emitted as `\uXXXX` escapes using surrogate pairs for supplementary-plane
/// characters.
fn write_json_string(value: &str, out: &mut String, ensure_ascii: bool) {
    out.push('"');
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];

        if ensure_ascii && byte >= 0x7F {
            // Flush the safe ASCII run accumulated so far.
            out.push_str(&value[start..i]);
            if byte == 0x7F {
                // DEL (0x7F) is a control character that CPython escapes.
                out.push_str("\\u007f");
                i += 1;
            } else {
                // Decode the full character at this position and emit \uXXXX escapes.
                let ch = value[i..].chars().next().expect("valid UTF-8");
                write_json_escape_for_non_ascii(ch, out);
                i += ch.len_utf8();
            }
            start = i;
            continue;
        }

        let escape = ESCAPE_TABLE[byte as usize];
        if escape == 0 {
            // Safe byte — keep scanning.
            i += 1;
            continue;
        }

        // Flush the safe run before this byte.
        out.push_str(&value[start..i]);

        // Write the escape sequence.
        match escape {
            b'b' => out.push_str("\\b"),
            b't' => out.push_str("\\t"),
            b'n' => out.push_str("\\n"),
            b'f' => out.push_str("\\f"),
            b'r' => out.push_str("\\r"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'u' => {
                write!(out, "\\u{:04x}", u32::from(byte)).expect("writing to String cannot fail");
            }
            _ => unreachable!(),
        }

        i += 1;
        start = i;
    }

    // Flush the final safe run.
    out.push_str(&value[start..]);
    out.push('"');
}

/// Byte lookup table for JSON string escaping.
///
/// Each entry is either 0 (byte is safe, no escaping needed) or a shorthand
/// character that indicates which escape to emit:
/// - `b'"'`  → `\"`
/// - `b'\\'` → `\\`
/// - `b'b'`  → `\b` (backspace, 0x08)
/// - `b't'`  → `\t` (tab, 0x09)
/// - `b'n'`  → `\n` (newline, 0x0A)
/// - `b'f'`  → `\f` (form feed, 0x0C)
/// - `b'r'`  → `\r` (carriage return, 0x0D)
/// - `b'u'`  → `\u00XX` (other control characters, 0x00–0x1F)
#[rustfmt::skip]
static ESCAPE_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    // Control characters 0x00–0x1F default to \u00XX escapes.
    let mut i = 0;
    while i < 0x20 {
        table[i] = b'u';
        i += 1;
    }
    // Override the named escapes.
    table[0x08] = b'b';  // backspace
    table[0x09] = b't';  // tab
    table[0x0A] = b'n';  // newline
    table[0x0C] = b'f';  // form feed
    table[0x0D] = b'r';  // carriage return
    table[0x22] = b'"';  // quote
    table[0x5C] = b'\\'; // backslash
    table
};

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
