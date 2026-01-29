//! Type conversion between Monty's `MontyObject` and JavaScript values via napi-rs.
//!
//! This module provides bidirectional conversion using native napi-rs APIs:
//! - `monty_to_js`: Convert Monty's `MontyObject` to a JavaScript value
//! - `js_to_monty`: Convert a JavaScript value to Monty's `MontyObject`
//!
//! ## Type Mappings
//!
//! ### Native JS types (bidirectional):
//! - `MontyObject::None` ↔ `null`
//! - `MontyObject::Bool` ↔ `boolean`
//! - `MontyObject::Int` ↔ `number` (if within safe integer range) or `BigInt`
//! - `MontyObject::BigInt` ↔ `BigInt`
//! - `MontyObject::Float` ↔ `number` (including `NaN`, `Infinity`, `-Infinity`)
//! - `MontyObject::String` ↔ `string`
//! - `MontyObject::Bytes` ↔ `Buffer` (Node.js)
//! - `MontyObject::List` ↔ `Array`
//! - `MontyObject::Dict` ↔ `Map` (preserves key types and insertion order)
//! - `MontyObject::Set` ↔ `Set`
//! - `MontyObject::FrozenSet` ↔ `Set` (JS has no frozen set)
//!
//! ### Marked JS types (with `__monty_type__` property):
//! - `MontyObject::Ellipsis` → `{ __monty_type__: 'Ellipsis' }`
//! - `MontyObject::Tuple` → `Array` with `__tuple__: true`
//! - `MontyObject::Exception` → `{ __monty_type__: 'Exception', excType, message }`
//! - `MontyObject::Type` → `{ __monty_type__: 'Type', value }`
//! - `MontyObject::BuiltinFunction` → `{ __monty_type__: 'BuiltinFunction', value }`
//! - `MontyObject::Dataclass` → `{ __monty_type__: 'Dataclass', name, fields, ... }`
//! - `MontyObject::Repr` → plain `string`
//! - `MontyObject::Cycle` → placeholder `string`

use std::collections::HashMap;

use monty::{DictPairs, ExcType, MontyObject};
use napi::bindgen_prelude::*;
use num_bigint::BigInt as NumBigInt;

/// JavaScript safe integer range: -(2^53) to 2^53.
const JS_SAFE_INT_MIN: i64 = -(1_i64 << 53);
const JS_SAFE_INT_MAX: i64 = 1_i64 << 53;

/// Wrapper for returning an unknown JS value from napi functions.
///
/// This allows `monty_to_js` to return dynamically typed JS values.
pub struct JsMontyObject<'env>(pub(crate) Unknown<'env>);

impl JsMontyObject<'_> {
    /// Returns the raw napi value for use in low-level operations.
    pub fn raw(&self) -> sys::napi_value {
        self.0.raw()
    }
}

impl ToNapiValue for JsMontyObject<'_> {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        Unknown::to_napi_value(env, val.0)
    }
}

/// Converts Monty's `MontyObject` to a JavaScript value using native napi-rs APIs.
///
/// This function creates native JS types where possible:
/// - Numbers use JS `number` or `BigInt` depending on size
/// - Dicts use native JS `Map` (preserves key types and insertion order)
/// - Sets use native JS `Set`
/// - Bytes use Node.js `Buffer`
/// - Tuples use arrays with a `__tuple__` marker property
///
/// Types that don't have direct JS equivalents get marker properties to preserve
/// type information for round-tripping.
pub fn monty_to_js<'e>(obj: &MontyObject, env: &'e Env) -> Result<JsMontyObject<'e>> {
    let unknown = match obj {
        MontyObject::None => create_js_null(env)?,
        MontyObject::Ellipsis => create_js_ellipsis(env)?,
        MontyObject::Bool(b) => create_js_bool(*b, env)?,
        MontyObject::Int(i) => create_js_int(*i, env)?,
        MontyObject::BigInt(bi) => create_js_bigint(bi, env)?,
        MontyObject::Float(f) => env.create_double(*f)?.into_unknown(env)?,
        MontyObject::String(s) => env.create_string(s)?.into_unknown(env)?,
        MontyObject::Bytes(bytes) => create_js_buffer(bytes, env)?,
        MontyObject::List(items) => create_js_array(items, env)?.into_unknown(env)?,
        MontyObject::Tuple(items) => create_js_tuple(items, env)?,
        MontyObject::Dict(pairs) => create_js_map(pairs, env)?,
        MontyObject::Set(items) | MontyObject::FrozenSet(items) => create_js_set(items, env)?,
        MontyObject::Exception { exc_type, arg } => create_js_exception(*exc_type, arg.as_deref(), env)?,
        MontyObject::Type(t) => create_js_type_marker(&t.to_string(), env)?,
        MontyObject::BuiltinFunction(f) => create_js_builtin_function_marker(&f.to_string(), env)?,
        MontyObject::Dataclass {
            name,
            type_id,
            field_names,
            attrs,
            methods,
            frozen,
        } => create_js_dataclass(name, *type_id, field_names, attrs, methods, *frozen, env)?,
        MontyObject::Repr(s) | MontyObject::Cycle(_, s) => env.create_string(s)?.into_unknown(env)?,
    };
    Ok(JsMontyObject(unknown))
}

/// Creates a JS null value.
fn create_js_null(env: &Env) -> Result<Unknown<'_>> {
    // Use raw napi to create null
    let mut result = std::ptr::null_mut();
    unsafe {
        let status = sys::napi_get_null(env.raw(), &raw mut result);
        if status != sys::Status::napi_ok {
            return Err(Error::from_reason("Failed to create null"));
        }
        Ok(Unknown::from_raw_unchecked(env.raw(), result))
    }
}

/// Creates a JS boolean value.
fn create_js_bool(b: bool, env: &Env) -> Result<Unknown<'_>> {
    let mut result = std::ptr::null_mut();
    unsafe {
        let status = sys::napi_get_boolean(env.raw(), b, &raw mut result);
        if status != sys::Status::napi_ok {
            return Err(Error::from_reason("Failed to create boolean"));
        }
        Ok(Unknown::from_raw_unchecked(env.raw(), result))
    }
}

/// Creates a JS number or BigInt depending on whether the value fits in JS safe integer range.
fn create_js_int(i: i64, env: &Env) -> Result<Unknown<'_>> {
    if (JS_SAFE_INT_MIN..=JS_SAFE_INT_MAX).contains(&i) {
        env.create_int64(i)?.into_unknown(env)
    } else {
        // Use BigInt for large integers
        BigInt::from(i).into_unknown(env)
    }
}

/// Creates a native JS BigInt from an arbitrary-precision integer.
///
/// For integers that fit in i64, uses direct creation. For larger integers,
/// calls the global `BigInt()` constructor with the decimal string representation.
fn create_js_bigint<'e>(bi: &NumBigInt, env: &'e Env) -> Result<Unknown<'e>> {
    // Try to fit in i64 first for efficiency
    if let Ok(i) = i64::try_from(bi) {
        return BigInt::from(i).into_unknown(env);
    }

    // For larger integers, call global BigInt(string)
    let global = env.get_global()?;
    let bigint_constructor: Function<String> = global.get_named_property("BigInt")?;
    let result = bigint_constructor.call(bi.to_string())?;
    result.into_unknown(env)
}

/// Creates a Node.js Buffer from bytes.
fn create_js_buffer<'e>(bytes: &[u8], env: &'e Env) -> Result<Unknown<'e>> {
    let buffer = BufferSlice::from_data(env, bytes.to_vec())?;
    buffer.into_unknown(env)
}

/// Creates a native JS Array from Monty list items, recursively converting each element.
fn create_js_array<'e>(items: &[MontyObject], env: &'e Env) -> Result<Array<'e>> {
    let mut arr = env.create_array(items.len().try_into().expect("array size overflows u32"))?;
    for (i, item) in items.iter().enumerate() {
        let js_item = monty_to_js(item, env)?;
        arr.set(i.try_into().expect("overflow on array index"), js_item)?;
    }
    Ok(arr)
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

/// Creates a native JS `Map` from Monty dict pairs, recursively converting keys and values.
///
/// Using `Map` instead of plain objects preserves:
/// - Non-string key types (numbers, booleans, etc.)
/// - Insertion order
/// - Proper equality semantics for keys
fn create_js_map<'e>(pairs: &DictPairs, env: &'e Env) -> Result<Unknown<'e>> {
    let global = env.get_global()?;
    let map_constructor: Function<()> = global.get_named_property("Map")?;
    let map: Object<'e> = map_constructor.new_instance(())?.coerce_to_object()?;

    let set_method: Unknown = map.get_named_property("set")?;
    for (k, v) in pairs {
        let js_key = monty_to_js(k, env)?;
        let js_value = monty_to_js(v, env)?;
        // Call map.set(key, value) using raw napi to pass two separate arguments
        call_method_2_args(env.raw(), map.raw(), set_method.raw(), js_key.0.raw(), js_value.0.raw())?;
    }
    map.into_unknown(env)
}

/// Calls a JS method with 2 arguments using raw napi.
///
/// This is needed because napi-rs's `Function::apply` with tuple args doesn't work correctly
/// for methods expecting two separate arguments.
fn call_method_2_args(
    env: sys::napi_env,
    this: sys::napi_value,
    method: sys::napi_value,
    arg1: sys::napi_value,
    arg2: sys::napi_value,
) -> Result<()> {
    let args = [arg1, arg2];
    let mut result = std::ptr::null_mut();
    unsafe {
        let status = sys::napi_call_function(env, this, method, 2, args.as_ptr(), &raw mut result);
        if status != sys::Status::napi_ok {
            return Err(Error::from_reason("Failed to call method"));
        }
    }
    Ok(())
}

/// Creates a native JS Set from Monty set items.
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

/// Creates a JS object representing Ellipsis: `{ __monty_type__: 'Ellipsis' }`.
fn create_js_ellipsis(env: &Env) -> Result<Unknown<'_>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Ellipsis")?;
    obj.into_unknown(env)
}

/// Creates a JS object representing an exception.
fn create_js_exception<'e>(exc_type: ExcType, arg: Option<&str>, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Exception")?;
    obj.set_named_property("excType", exc_type.to_string())?;
    obj.set_named_property("message", arg.unwrap_or(""))?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a Type: `{ __monty_type__: 'Type', value: '...' }`.
fn create_js_type_marker<'e>(type_str: &str, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Type")?;
    obj.set_named_property("value", type_str)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a builtin function.
fn create_js_builtin_function_marker<'e>(func_str: &str, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "BuiltinFunction")?;
    obj.set_named_property("value", func_str)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a dataclass instance.
fn create_js_dataclass<'e>(
    name: &str,
    type_id: u64,
    field_names: &[String],
    attrs: &DictPairs,
    methods: &[String],
    frozen: bool,
    env: &'e Env,
) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Dataclass")?;
    obj.set_named_property("name", name)?;

    // type_id as BigInt since it may exceed JS safe integer range
    let type_id_bigint = BigInt::from(type_id);
    obj.set_named_property("typeId", type_id_bigint)?;

    // field_names as array
    let mut field_names_arr =
        env.create_array(field_names.len().try_into().expect("field_names size overflows u32"))?;
    for (i, field_name) in field_names.iter().enumerate() {
        field_names_arr.set(
            i.try_into().expect("overflow on field_names index"),
            env.create_string(field_name)?,
        )?;
    }
    obj.set_named_property("fieldNames", field_names_arr)?;

    // Build attrs as a nested object mapping field names to values
    let attrs_map: HashMap<&str, &MontyObject> = attrs
        .into_iter()
        .filter_map(|(k, v)| {
            if let MontyObject::String(key) = k {
                Some((key.as_str(), v))
            } else {
                None
            }
        })
        .collect();

    let mut fields_obj = Object::new(env)?;
    for field_name in field_names {
        if let Some(value) = attrs_map.get(field_name.as_str()) {
            let js_value = monty_to_js(value, env)?;
            fields_obj.set_named_property(field_name.as_str(), js_value)?;
        }
    }
    obj.set_named_property("fields", fields_obj)?;

    // methods as array
    let mut methods_arr = env.create_array(methods.len().try_into().expect("methods size overflows u32"))?;
    for (i, method) in methods.iter().enumerate() {
        methods_arr.set(
            i.try_into().expect("overflow on methods index"),
            env.create_string(method)?,
        )?;
    }
    obj.set_named_property("methods", methods_arr)?;

    obj.set_named_property("frozen", frozen)?;

    obj.into_unknown(env)
}

// =============================================================================
// JS to Monty conversion
// =============================================================================

/// Converts a JavaScript value to Monty's `MontyObject`.
///
/// This function handles native JS types and marked objects:
/// - `null` → `None`
/// - `boolean` → `Bool`
/// - `number` → `Int` (if integer) or `Float`
/// - `bigint` → `Int` (if fits in i64) or `BigInt`
/// - `string` → `String`
/// - `Buffer`/`Uint8Array` → `Bytes`
/// - `Array` with `__tuple__` → `Tuple`
/// - `Array` → `List`
/// - `Map` → `Dict`
/// - `Set` → `Set`
/// - `Object` with `__monty_type__` → corresponding Monty type
/// - `Object` → `Dict` (string keys only)
pub fn js_to_monty(value: Unknown<'_>, env: Env) -> Result<MontyObject> {
    let value_type = value.get_type()?;

    match value_type {
        ValueType::Null | ValueType::Undefined => Ok(MontyObject::None),
        ValueType::Boolean => {
            let b: bool = value.coerce_to_bool()?;
            Ok(MontyObject::Bool(b))
        }
        ValueType::Number => {
            let n: f64 = value.coerce_to_number()?.get_double()?;
            // Check if the number is actually an integer (no fractional part)
            // and fits within i64 range
            if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "Checked above that n is integer and within i64 range"
                )]
                return Ok(MontyObject::Int(n as i64));
            }
            Ok(MontyObject::Float(n))
        }
        ValueType::BigInt => {
            let bigint: BigInt = BigInt::from_unknown(value)?;

            // BigInt has public fields: sign_bit (bool) and words (Vec<u64>)
            // Convert words (u64 array) to num-bigint::BigInt
            // Each word is a 64-bit limb, little-endian order
            if bigint.words.is_empty() {
                return Ok(MontyObject::Int(0));
            }

            let mut bi = NumBigInt::from(0u64);
            for (i, &word) in bigint.words.iter().enumerate() {
                let limb = NumBigInt::from(word);
                bi += limb << (64 * i);
            }

            if bigint.sign_bit {
                bi = -bi;
            }

            // Try to fit in i64
            if let Ok(i) = i64::try_from(&bi) {
                Ok(MontyObject::Int(i))
            } else {
                Ok(MontyObject::BigInt(bi))
            }
        }
        ValueType::String => {
            let s: String = value.coerce_to_string()?.into_utf8()?.into_owned()?;
            Ok(MontyObject::String(s))
        }
        ValueType::Object => {
            let obj: Object = value.coerce_to_object()?;

            // Check if it's a Buffer (Uint8Array)
            if obj.is_buffer()? {
                let buffer: BufferSlice = BufferSlice::from_unknown(value)?;
                return Ok(MontyObject::Bytes(buffer.to_vec()));
            }

            // Check if it's a Map
            if is_js_map(&obj, env)? {
                return js_map_to_monty(obj, env);
            }

            // Check if it's a Set
            if is_js_set(&obj, env)? {
                return js_set_to_monty(obj, env);
            }

            // Check if it's an Array
            if obj.is_array()? {
                return js_array_to_monty(obj, env);
            }

            // Check for __monty_type__ marker
            if let Some(monty_type) = get_string_property(&obj, "__monty_type__")? {
                return js_marked_object_to_monty(&obj, &monty_type, env);
            }

            // Plain object → Dict (with string keys)
            js_object_to_monty_dict(obj, env)
        }
        ValueType::Function | ValueType::Symbol | ValueType::External => {
            // These JS types don't have Monty equivalents
            Err(Error::from_reason(format!(
                "Cannot convert JS {value_type:?} to Monty value"
            )))
        }
        // Unknown is not a real JS type, it's a napi-rs placeholder
        ValueType::Unknown => Err(Error::from_reason("Unknown JS value type")),
    }
}

/// Checks if a JS object is an instance of Set.
fn is_js_set(obj: &Object, env: Env) -> Result<bool> {
    let global = env.get_global()?;
    let set_constructor: Function<()> = global.get_named_property("Set")?;
    obj.instanceof(set_constructor)
}

/// Checks if a JS object is an instance of Map.
fn is_js_map(obj: &Object, env: Env) -> Result<bool> {
    let global = env.get_global()?;
    let map_constructor: Function<()> = global.get_named_property("Map")?;
    obj.instanceof(map_constructor)
}

/// Converts a JS Map to `MontyObject::Dict`.
fn js_map_to_monty(map: Object, env: Env) -> Result<MontyObject> {
    // Get the entries iterator
    let entries_method: Function<()> = map.get_named_property("entries")?;
    let iterator: Object = entries_method.apply(map, ())?.coerce_to_object()?;

    let mut pairs = Vec::new();
    loop {
        let next_method: Function<()> = iterator.get_named_property("next")?;
        let result: Object = next_method.apply(iterator, ())?.coerce_to_object()?;

        let done: bool = result.get_named_property::<bool>("done")?;
        if done {
            break;
        }

        // value is [key, value] array
        let entry: Object = result.get_named_property::<Unknown>("value")?.coerce_to_object()?;
        let key: Unknown = entry.get_element(0)?;
        let value: Unknown = entry.get_element(1)?;

        let monty_key = js_to_monty(key, env)?;
        let monty_value = js_to_monty(value, env)?;
        pairs.push((monty_key, monty_value));
    }

    Ok(MontyObject::dict(pairs))
}

/// Converts a JS Set to `MontyObject::Set`.
fn js_set_to_monty(set: Object, env: Env) -> Result<MontyObject> {
    // Get the values iterator
    let values_method: Function<()> = set.get_named_property("values")?;
    let iterator: Object = values_method.apply(set, ())?.coerce_to_object()?;

    let mut items = Vec::new();
    loop {
        let next_method: Function<()> = iterator.get_named_property("next")?;
        let result: Object = next_method.apply(iterator, ())?.coerce_to_object()?;

        let done: bool = result.get_named_property::<bool>("done")?;
        if done {
            break;
        }

        let value: Unknown = result.get_named_property("value")?;
        items.push(js_to_monty(value, env)?);
    }

    Ok(MontyObject::Set(items))
}

/// Converts a JS Array to `MontyObject::List` or `MontyObject::Tuple`.
fn js_array_to_monty(arr: Object, env: Env) -> Result<MontyObject> {
    let is_tuple: bool = arr.get_named_property::<Option<bool>>("__tuple__")?.unwrap_or(false);

    let length: u32 = arr.get_named_property("length")?;
    let mut items = Vec::with_capacity(length as usize);

    for i in 0..length {
        let element: Unknown = arr.get_element(i)?;
        items.push(js_to_monty(element, env)?);
    }

    if is_tuple {
        Ok(MontyObject::Tuple(items))
    } else {
        Ok(MontyObject::List(items))
    }
}

/// Converts a JS object with `__monty_type__` marker to the appropriate `MontyObject`.
fn js_marked_object_to_monty(obj: &Object, monty_type: &str, env: Env) -> Result<MontyObject> {
    match monty_type {
        "Ellipsis" => Ok(MontyObject::Ellipsis),
        "Exception" => {
            let exc_type_str: String = obj.get_named_property("excType")?;
            let message: String = obj.get_named_property("message")?;
            let exc_type: ExcType = exc_type_str
                .parse()
                .map_err(|_| Error::from_reason(format!("Unknown exception type: {exc_type_str}")))?;
            let arg = if message.is_empty() { None } else { Some(message) };
            Ok(MontyObject::Exception { exc_type, arg })
        }
        "Type" => {
            // Type objects can't be fully round-tripped; return as Repr
            let value: String = obj.get_named_property("value")?;
            Ok(MontyObject::Repr(format!("<class '{value}'>")))
        }
        "BuiltinFunction" => {
            // BuiltinFunction objects can't be fully round-tripped; return as Repr
            let value: String = obj.get_named_property("value")?;
            Ok(MontyObject::Repr(format!("<built-in function {value}>")))
        }
        "Dataclass" => {
            let name: String = obj.get_named_property("name")?;

            // type_id is BigInt - access its public fields
            let type_id_bigint: BigInt = obj.get_named_property("typeId")?;
            let type_id = if type_id_bigint.words.is_empty() {
                0u64
            } else if type_id_bigint.sign_bit {
                return Err(Error::from_reason("Dataclass typeId cannot be negative"));
            } else {
                type_id_bigint.words[0]
            };

            // field_names
            let field_names_arr: Array = obj.get_named_property("fieldNames")?;
            let field_names_len = field_names_arr.len();
            let mut field_names = Vec::with_capacity(field_names_len as usize);
            for i in 0..field_names_len {
                let name: String = field_names_arr.get::<String>(i)?.unwrap_or_default();
                field_names.push(name);
            }

            // fields object
            let fields_obj: Object = obj.get_named_property("fields")?;
            let mut attrs_vec = Vec::new();
            for field_name in &field_names {
                if let Some(value) = fields_obj.get_named_property::<Option<Unknown>>(field_name.as_str())? {
                    let monty_value = js_to_monty(value, env)?;
                    attrs_vec.push((MontyObject::String(field_name.clone()), monty_value));
                }
            }
            let attrs = DictPairs::from(attrs_vec);

            // methods
            let methods_arr: Array = obj.get_named_property("methods")?;
            let methods_len = methods_arr.len();
            let mut methods = Vec::with_capacity(methods_len as usize);
            for i in 0..methods_len {
                let method: String = methods_arr.get::<String>(i)?.unwrap_or_default();
                methods.push(method);
            }

            let frozen: bool = obj.get_named_property("frozen")?;

            Ok(MontyObject::Dataclass {
                name,
                type_id,
                field_names,
                attrs,
                methods,
                frozen,
            })
        }
        _ => {
            // Unknown marker type, treat as dict
            js_object_to_monty_dict(*obj, env)
        }
    }
}

/// Converts a plain JS object to `MontyObject::Dict`.
///
/// This is a fallback for plain objects (not Map instances). Since JS object keys
/// are always strings, all keys in the resulting Dict will be strings.
/// For full key type preservation, use JS `Map` instead.
fn js_object_to_monty_dict(obj: Object, env: Env) -> Result<MontyObject> {
    let keys = obj.get_property_names()?;
    // Get length by accessing the "length" property
    let length: u32 = keys.get_named_property("length")?;
    let mut pairs = Vec::with_capacity(length as usize);

    for i in 0..length {
        let key: Unknown = keys.get_element(i)?;
        let key_str: String = key.coerce_to_string()?.into_utf8()?.into_owned()?;
        let value: Unknown = obj.get_named_property(&key_str)?;
        let monty_value = js_to_monty(value, env)?;
        pairs.push((MontyObject::String(key_str), monty_value));
    }

    Ok(MontyObject::dict(pairs))
}

/// Helper to get an optional string property from a JS object.
fn get_string_property(obj: &Object, name: &str) -> Result<Option<String>> {
    let has_property = obj.has_named_property(name)?;
    if !has_property {
        return Ok(None);
    }

    let value: Unknown = obj.get_named_property(name)?;
    if value.get_type()? == ValueType::String {
        let s: String = value.coerce_to_string()?.into_utf8()?.into_owned()?;
        Ok(Some(s))
    } else {
        Ok(None)
    }
}
