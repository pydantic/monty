//! JSON parsing support for `json.loads()`.
//!
//! This module owns conversion from JSON bytes into Monty runtime values,
//! including CPython-compatible `JSONDecodeError` construction.

use std::borrow::Cow;

use jiter::{JsonErrorType, JsonValue};

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropWithHeap, HeapData, HeapGuard},
    resource::ResourceTracker,
    types::{Dict, List, LongInt, PyTrait, str::allocate_string},
    value::Value,
};

/// Implements `json.loads(s)`.
///
/// The function accepts exactly one positional argument and rejects keyword
/// arguments. Input may be `str` or `bytes`; parsed JSON values are converted
/// recursively into Monty `Value`s.
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
/// The parser works directly on the underlying byte slice so borrowed strings
/// from `jiter` remain valid throughout conversion.
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
/// Syntax errors are wrapped in `json.JSONDecodeError` using the same
/// line/column/character suffix as CPython.
fn parse_json_bytes(bytes: &[u8], vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    let parsed = JsonValue::parse(bytes, false).map_err(|error| json_error_to_run_error(&error, bytes))?;
    convert_json_value(parsed, vm)
}

/// Converts a `jiter::JsonValue` tree into Monty runtime values.
///
/// Strings, arrays, and objects are allocated recursively. Integer values that
/// exceed `i64` become heap-allocated `LongInt`s so numeric round-tripping
/// preserves precision.
fn convert_json_value(value: JsonValue<'_>, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    match value {
        JsonValue::Null => Ok(Value::None),
        JsonValue::Bool(value) => Ok(Value::Bool(value)),
        JsonValue::Int(value) => Ok(Value::Int(value)),
        JsonValue::BigInt(value) => LongInt::from(value).into_value(vm.heap).map_err(Into::into),
        JsonValue::Float(value) => Ok(Value::Float(value)),
        JsonValue::Str(value) => allocate_string(value.into_owned(), vm.heap),
        JsonValue::Array(items) => {
            let values = Vec::with_capacity(items.len());
            let mut values_guard = HeapGuard::new(values, vm);
            {
                let (values, vm) = values_guard.as_parts_mut();
                for item in items.as_ref().iter().cloned() {
                    values.push(convert_json_value(item, vm)?);
                }
            }
            let values = values_guard.into_inner();
            let list_id = vm.heap.allocate(HeapData::List(List::new(values)))?;
            Ok(Value::Ref(list_id))
        }
        JsonValue::Object(items) => {
            let pairs = Vec::with_capacity(items.len());
            let mut pairs_guard = HeapGuard::new(pairs, vm);
            {
                let (pairs, vm) = pairs_guard.as_parts_mut();
                for (key, value) in items.as_ref().iter().cloned() {
                    let key = allocate_string(key.into_owned(), vm.heap)?;
                    let value = convert_json_value(value, vm)?;
                    pairs.push((key, value));
                }
            }
            let pairs = pairs_guard.into_inner();
            let dict = Dict::from_pairs(pairs, vm)?;
            let dict_id = vm.heap.allocate(HeapData::Dict(dict))?;
            Ok(Value::Ref(dict_id))
        }
    }
}

/// Converts a `jiter` parse error into `json.JSONDecodeError`.
///
/// `jiter` exposes the error byte index plus a helper for computing line and
/// column, which is enough to reproduce CPython's message suffix exactly.
fn json_error_to_run_error(error: &jiter::JsonError, bytes: &[u8]) -> RunError {
    let position = error.get_position(bytes);
    let message = match &error.error_type {
        JsonErrorType::KeyMustBeAString => "Expecting property name enclosed in double quotes".to_owned(),
        _ => error.description(bytes),
    };
    ExcType::json_decode_error(&message, position.line, position.column, error.index)
}
