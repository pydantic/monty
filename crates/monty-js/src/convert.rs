//! Type conversion between Monty's `MontyObject` and JavaScript values via napi-rs.
//!
//! This module provides bidirectional conversion:
//! - `monty_to_serde`: Convert Monty's `MontyObject` to a serde_json::Value for JS output
//! - `serde_to_monty`: Convert serde_json::Value from JS to Monty's `MontyObject`
//!
//! We use serde_json::Value as an intermediate format since napi-rs can serialize/deserialize it.

use monty::{DictPairs, ExcType, MontyObject};
use napi::bindgen_prelude::*;
use serde_json::{json, Map, Value};

/// Converts Monty's `MontyObject` to a serde_json::Value for JavaScript output.
///
/// The value is wrapped in an object with `_type` and `_value` fields to preserve type information.
pub fn monty_to_serde(obj: &MontyObject) -> Value {
    match obj {
        MontyObject::None => Value::Null,
        MontyObject::Ellipsis => json!({ "_type": "Ellipsis" }),
        MontyObject::Bool(b) => Value::Bool(*b),
        MontyObject::Int(i) => {
            // Check if it fits in JS safe integer range
            if *i >= -(1_i64 << 53) && *i <= (1_i64 << 53) {
                json!(*i)
            } else {
                // Return as string for large integers
                json!({ "_type": "BigInt", "_value": i.to_string() })
            }
        }
        MontyObject::BigInt(bi) => {
            json!({ "_type": "BigInt", "_value": bi.to_string() })
        }
        MontyObject::Float(f) => {
            if f.is_nan() {
                json!({ "_type": "Float", "_value": "NaN" })
            } else if f.is_infinite() {
                json!({ "_type": "Float", "_value": if *f > 0.0 { "Infinity" } else { "-Infinity" } })
            } else {
                json!(*f)
            }
        }
        MontyObject::String(s) => Value::String(s.clone()),
        MontyObject::Bytes(b) => {
            json!({ "_type": "Bytes", "_value": b })
        }
        MontyObject::List(items) => Value::Array(items.iter().map(monty_to_serde).collect()),
        MontyObject::Tuple(items) => {
            json!({
                "_type": "Tuple",
                "_value": items.iter().map(monty_to_serde).collect::<Vec<_>>()
            })
        }
        MontyObject::Dict(pairs) => {
            // Convert to a JS object (only works well with string keys)
            let mut map = Map::new();
            for (k, v) in pairs {
                let key = match k {
                    MontyObject::String(s) => s.clone(),
                    _ => format!("{k:?}"),
                };
                map.insert(key, monty_to_serde(v));
            }
            Value::Object(map)
        }
        MontyObject::Set(items) => {
            json!({
                "_type": "Set",
                "_value": items.iter().map(monty_to_serde).collect::<Vec<_>>()
            })
        }
        MontyObject::FrozenSet(items) => {
            json!({
                "_type": "FrozenSet",
                "_value": items.iter().map(monty_to_serde).collect::<Vec<_>>()
            })
        }
        MontyObject::Exception { exc_type, arg } => {
            json!({
                "_type": "Exception",
                "excType": exc_type.to_string(),
                "message": arg.clone().unwrap_or_default()
            })
        }
        MontyObject::Type(t) => {
            json!({ "_type": "Type", "_value": t.to_string() })
        }
        MontyObject::BuiltinFunction(f) => {
            json!({ "_type": "BuiltinFunction", "_value": f.to_string() })
        }
        MontyObject::Dataclass {
            name,
            field_names,
            attrs,
            ..
        } => {
            let mut fields = Map::new();
            // attrs is a DictPairs containing (key, value) pairs
            // We iterate over field_names and look up each in attrs
            let attrs_map: std::collections::HashMap<&str, &MontyObject> = attrs
                .into_iter()
                .filter_map(|(k, v)| {
                    if let MontyObject::String(key) = k {
                        Some((key.as_str(), v))
                    } else {
                        None
                    }
                })
                .collect();
            for field_name in field_names {
                if let Some(value) = attrs_map.get(field_name.as_str()) {
                    fields.insert(field_name.clone(), monty_to_serde(value));
                }
            }
            json!({
                "_type": "Dataclass",
                "_name": name,
                "_fields": fields
            })
        }
        MontyObject::Repr(s) => Value::String(s.clone()),
        MontyObject::Cycle(_, placeholder) => Value::String(placeholder.clone()),
    }
}

pub struct JsMontyObject<'env>(Unknown<'env>);

impl ToNapiValue for JsMontyObject<'_> {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        Unknown::to_napi_value(env, val.0)
    }
}

/// Converts Monty's `MontyObject` to a JavaScript value, using native JS types where possible.
///
/// This function handles types that need native JS representation:
/// - `Set` and `FrozenSet` → native JS `Set`
/// - `Bytes` → `Uint8Array`
/// - All other types go through serde_json for conversion
pub fn monty_to_js<'e>(obj: &MontyObject, env: &'e Env) -> Result<JsMontyObject<'e>> {
    let unknown = match obj {
        MontyObject::Set(items) | MontyObject::FrozenSet(items) => {
            // Create a native JS Set for both Set and FrozenSet
            // (JavaScript doesn't have a native frozen set, so we use Set for both)
            create_js_set(items, env)?
        }
        MontyObject::Bytes(bytes) => {
            // Create a native Uint8Array
            create_js_uint8array(bytes, env)?
        }
        MontyObject::List(items) => {
            // Create a native JS Array with recursively converted items
            create_js_array(items, env)?.into_unknown(env)?
        }
        MontyObject::Dict(pairs) => {
            // Create a native JS Object with recursively converted values
            create_js_object(pairs, env)?
        }
        MontyObject::Tuple(items) => {
            // Create a tuple representation: an array with __tuple__ marker
            create_js_tuple(items, env)?
        }
        // For all other types, convert through serde_json
        _ => {
            let serde_value = monty_to_serde(obj);
            env.to_js_value(&serde_value)?.into_unknown(env)?
        }
    };
    Ok(JsMontyObject(unknown))
}

/// Creates a native JS `Set` from Monty set items.
fn create_js_set<'e>(items: &[MontyObject], env: &'e Env) -> Result<Unknown<'e>> {
    let global = env.get_global()?;
    let set_constructor: Function<()> = global.get_named_property("Set")?;
    let set: Object<'e> = set_constructor.new_instance(())?.coerce_to_object()?;

    let add_method: Function = set.get_named_property("add")?;
    for item in items {
        let js_item = monty_to_js(item, env)?;
        add_method.apply(set, js_item.0)?;
    }
    set.into_unknown(env)
}

/// Creates a native `Uint8Array` from Monty bytes.
fn create_js_uint8array<'e>(bytes: &[u8], env: &'e Env) -> Result<Unknown<'e>> {
    let buffer = BufferSlice::from_data(env, bytes.to_vec())?;
    buffer.into_unknown(env)
}

/// Creates a native JS `Array` from Monty list items, recursively converting each element.
fn create_js_array<'e>(items: &[MontyObject], env: &'e Env) -> Result<Array<'e>> {
    let mut arr = env.create_array(items.len().try_into().expect("array size overflows u32"))?;
    for (i, item) in items.iter().enumerate() {
        let js_item = monty_to_js(item, env)?;
        arr.set(i.try_into().expect("overflow on array bound"), js_item)?;
    }
    Ok(arr)
}

/// Creates a native JS `Object` from Monty dict pairs, recursively converting values.
fn create_js_object<'e>(pairs: &DictPairs, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    for (k, v) in pairs {
        let key = match k {
            MontyObject::String(s) => s.clone(),
            _ => format!("{k:?}"),
        };
        let js_value = monty_to_js(v, env)?;
        obj.set_named_property(&key, js_value)?;
    }
    obj.into_unknown(env)
}

/// Creates a tuple representation as a JS array with a `__tuple__` marker property.
///
/// This allows distinguishing tuples from lists in JavaScript while still allowing
/// array-like access to tuple elements.
fn create_js_tuple<'e>(items: &[MontyObject], env: &'e Env) -> Result<Unknown<'e>> {
    let mut arr = create_js_array(items, env)?;
    arr.set_named_property("__tuple__", true)?;
    arr.into_unknown(env)
}

/// Converts a serde_json::Value from JavaScript to Monty's `MontyObject`.
pub fn serde_to_monty(value: &Value) -> Result<MontyObject> {
    match value {
        Value::Null => Ok(MontyObject::None),
        Value::Bool(b) => Ok(MontyObject::Bool(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MontyObject::Int(i))
            } else if let Some(f) = n.as_f64() {
                // Check if it's actually an integer
                #[expect(clippy::cast_possible_truncation, reason = "range check ensures safe cast")]
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    Ok(MontyObject::Int(f as i64))
                } else {
                    Ok(MontyObject::Float(f))
                }
            } else {
                Err(Error::from_reason("Invalid number"))
            }
        }
        Value::String(s) => Ok(MontyObject::String(s.clone())),
        Value::Array(arr) => {
            let items: Result<Vec<_>> = arr.iter().map(serde_to_monty).collect();
            Ok(MontyObject::List(items?))
        }
        Value::Object(obj) => {
            // Check for special types
            if let Some(type_str) = obj.get("_type").and_then(|v| v.as_str()) {
                match type_str {
                    "Ellipsis" => return Ok(MontyObject::Ellipsis),
                    "BigInt" => {
                        if let Some(s) = obj.get("_value").and_then(|v| v.as_str()) {
                            let bi: num_bigint::BigInt = s
                                .parse()
                                .map_err(|e| Error::from_reason(format!("Invalid BigInt: {e}")))?;
                            return Ok(MontyObject::BigInt(bi));
                        }
                    }
                    "Float" => {
                        if let Some(s) = obj.get("_value").and_then(|v| v.as_str()) {
                            let f = match s {
                                "NaN" => f64::NAN,
                                "Infinity" => f64::INFINITY,
                                "-Infinity" => f64::NEG_INFINITY,
                                _ => s
                                    .parse()
                                    .map_err(|e| Error::from_reason(format!("Invalid float: {e}")))?,
                            };
                            return Ok(MontyObject::Float(f));
                        }
                    }
                    "Bytes" => {
                        if let Some(arr) = obj.get("_value").and_then(|v| v.as_array()) {
                            let bytes: std::result::Result<Vec<u8>, _> = arr
                                .iter()
                                .map(|v| {
                                    v.as_u64()
                                        .and_then(|n| u8::try_from(n).ok())
                                        .ok_or_else(|| Error::from_reason("Invalid byte value"))
                                })
                                .collect();
                            return Ok(MontyObject::Bytes(bytes?));
                        }
                    }
                    "Tuple" => {
                        if let Some(arr) = obj.get("_value").and_then(|v| v.as_array()) {
                            let items: Result<Vec<_>> = arr.iter().map(serde_to_monty).collect();
                            return Ok(MontyObject::Tuple(items?));
                        }
                    }
                    "Set" => {
                        if let Some(arr) = obj.get("_value").and_then(|v| v.as_array()) {
                            let items: Result<Vec<_>> = arr.iter().map(serde_to_monty).collect();
                            return Ok(MontyObject::Set(items?));
                        }
                    }
                    "FrozenSet" => {
                        if let Some(arr) = obj.get("_value").and_then(|v| v.as_array()) {
                            let items: Result<Vec<_>> = arr.iter().map(serde_to_monty).collect();
                            return Ok(MontyObject::FrozenSet(items?));
                        }
                    }
                    _ => {} // Fall through to treat as dict
                }
            }

            // Treat as a regular dict
            let pairs: Result<Vec<_>> = obj
                .iter()
                .map(|(k, v)| Ok((MontyObject::String(k.clone()), serde_to_monty(v)?)))
                .collect();
            Ok(MontyObject::dict(pairs?))
        }
    }
}

/// Converts a Monty exception type to a JavaScript error class name.
pub fn exc_type_to_js_name(exc_type: ExcType) -> &'static str {
    match exc_type {
        ExcType::Exception => "Error",
        ExcType::BaseException => "Error",
        ExcType::SystemExit => "SystemExitError",
        ExcType::KeyboardInterrupt => "KeyboardInterruptError",
        ExcType::ArithmeticError => "ArithmeticError",
        ExcType::OverflowError => "OverflowError",
        ExcType::ZeroDivisionError => "ZeroDivisionError",
        ExcType::LookupError => "LookupError",
        ExcType::IndexError => "IndexError",
        ExcType::KeyError => "KeyError",
        ExcType::RuntimeError => "RuntimeError",
        ExcType::NotImplementedError => "NotImplementedError",
        ExcType::RecursionError => "RecursionError",
        ExcType::AssertionError => "AssertionError",
        ExcType::AttributeError => "AttributeError",
        ExcType::FrozenInstanceError => "FrozenInstanceError",
        ExcType::MemoryError => "MemoryError",
        ExcType::NameError => "NameError",
        ExcType::UnboundLocalError => "UnboundLocalError",
        ExcType::SyntaxError => "SyntaxError",
        ExcType::TimeoutError => "TimeoutError",
        ExcType::TypeError => "TypeError",
        ExcType::ValueError => "ValueError",
        ExcType::ImportError => "ImportError",
        ExcType::ModuleNotFoundError => "ModuleNotFoundError",
        ExcType::UnicodeDecodeError => "UnicodeDecodeError",
    }
}
