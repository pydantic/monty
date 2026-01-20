//! Type conversion between Monty's `MontyObject` and JavaScript values via napi-rs.
//!
//! This module provides bidirectional conversion:
//! - `monty_to_serde`: Convert Monty's `MontyObject` to a serde_json::Value for JS output
//! - `serde_to_monty`: Convert serde_json::Value from JS to Monty's `MontyObject`
//!
//! We use serde_json::Value as an intermediate format since napi-rs can serialize/deserialize it.

use monty::{ExcType, MontyObject};
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
