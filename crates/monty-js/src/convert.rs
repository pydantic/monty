//! Bidirectional conversion between Monty's `MontyObject` and JavaScript
//! values via napi-rs (`monty_to_js` / `js_to_monty`).
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
//! - `MontyObject::ClassInstance` → `{ __monty_type__: 'ClassInstance', name, instanceId, typeId, attrs, frozen, isDataclass }`
//! - `MontyObject::FileHandle` ↔ `{ __monty_type__: 'FileHandle', path, mode, position }`
//! - `MontyObject::Repr` → plain `string`
//! - `MontyObject::Cycle` → placeholder `string`
#![expect(unsafe_code, reason = "napi API is unsafe")]

use std::{borrow::Cow, ptr};

use monty_types::{
    ClassType, DictPairs, ExcType, FileMode, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTime,
    MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid,
};
use napi::{bindgen_prelude::*, sys::Status};
use num_bigint::BigInt as NumBigInt;

/// JavaScript safe integer range: -(2^53) to 2^53.
const JS_SAFE_INT_MIN: i64 = -(1_i64 << 53);
const JS_SAFE_INT_MAX: i64 = 1_i64 << 53;
const JS_MAX_SAFE_POSITION: u64 = (1_u64 << 53) - 1;
const JS_MAX_SAFE_POSITION_F64: f64 = 9_007_199_254_740_991.0;

/// Wrapper letting `monty_to_js` return a dynamically typed JS value from a
/// napi function.
pub struct JsMontyObject<'env>(pub(crate) Unknown<'env>);

impl ToNapiValue for JsMontyObject<'_> {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        Unknown::to_napi_value(env, val.0)
    }
}

/// Converts a `MontyObject` to a JS value, using native JS types where
/// possible (`number`/`BigInt`, `Map`, `Set`, `Buffer`, `__tuple__`-marked
/// arrays). Types without a JS equivalent get `__monty_type__` marker
/// properties so they round-trip.
pub fn monty_to_js<'e>(obj: &MontyObject, env: &'e Env) -> Result<JsMontyObject<'e>> {
    let unknown = match obj {
        MontyObject::None => create_js_null(env)?,
        MontyObject::Ellipsis => create_js_ellipsis(env)?,
        MontyObject::NotImplemented => create_js_not_implemented(env)?,
        MontyObject::Bool(b) => create_js_bool(*b, env)?,
        MontyObject::Int(i) => create_js_int(*i, env)?,
        MontyObject::BigInt(bi) => create_js_bigint(bi, env)?,
        MontyObject::Float(f) => env.create_double(*f)?.into_unknown(env)?,
        MontyObject::String(s) => env.create_string(s)?.into_unknown(env)?,
        MontyObject::Bytes(bytes) => create_js_buffer(bytes, env)?,
        MontyObject::List(items) => create_js_array(items, env)?.into_unknown(env)?,
        MontyObject::Tuple(items) => create_js_tuple(items, env)?,
        // NamedTuple is converted to a tuple (loses named access in JS)
        MontyObject::NamedTuple { values, .. } => create_js_tuple(values, env)?,
        MontyObject::Dict(pairs) => create_js_map(pairs, env)?,
        MontyObject::Set(items) | MontyObject::FrozenSet(items) => create_js_set(items, env)?,
        MontyObject::Exception { exc_type, arg } => create_js_exception(*exc_type, arg.as_deref(), env)?,
        MontyObject::Date(date) => create_js_date(date, env)?,
        MontyObject::DateTime(datetime) => create_js_datetime(datetime, env)?,
        MontyObject::Time(time) => create_js_time(time, env)?,
        MontyObject::TimeDelta(delta) => create_js_timedelta(delta, env)?,
        MontyObject::TimeZone(timezone) => create_js_timezone(timezone, env)?,
        MontyObject::Type(MontyType::Instance(class_type)) => create_js_class_type_marker(class_type, env)?,
        MontyObject::Type(t) => create_js_type_marker(&t.to_string(), env)?,
        MontyObject::BuiltinFunction(f) => create_js_builtin_function_marker(&f.to_string(), env)?,
        MontyObject::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => create_js_class_instance(class_type, instance_id, attrs, env)?,
        MontyObject::Path(p) => env.create_string(p)?.into_unknown(env)?,
        MontyObject::FileHandle(handle) => create_js_file_handle(handle, env)?,
        MontyObject::Repr(s) | MontyObject::Cycle(_, s) => env.create_string(s)?.into_unknown(env)?,
        // Function objects are internal to the name lookup protocol and should not normally
        // appear as final output values. If they do, represent as a string with the function name.
        MontyObject::Function { name, .. } => env.create_string(name)?.into_unknown(env)?,
    };
    Ok(JsMontyObject(unknown))
}

/// Creates a JS null value.
fn create_js_null(env: &Env) -> Result<Unknown<'_>> {
    let mut result = ptr::null_mut();
    // SAFETY: [DH] - all arguments are valid and result is valid on success
    unsafe {
        let status = sys::napi_get_null(env.raw(), &raw mut result);
        if status != Status::napi_ok {
            return Err(Error::from_reason("Failed to create null"));
        }
        Ok(Unknown::from_raw_unchecked(env.raw(), result))
    }
}

/// Creates a JS boolean value.
fn create_js_bool(b: bool, env: &Env) -> Result<Unknown<'_>> {
    let mut result = ptr::null_mut();
    // SAFETY: [DH] - all arguments are valid and result is valid on success
    unsafe {
        let status = sys::napi_get_boolean(env.raw(), b, &raw mut result);
        if status != Status::napi_ok {
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
        BigInt::from(i).into_unknown(env)
    }
}

/// Creates a native JS BigInt from an arbitrary-precision integer. Values that
/// fit in i64 use direct creation; larger ones call the global `BigInt()`
/// constructor with the decimal string.
fn create_js_bigint<'e>(bi: &NumBigInt, env: &'e Env) -> Result<Unknown<'e>> {
    if let Ok(i) = i64::try_from(bi) {
        return BigInt::from(i).into_unknown(env);
    }

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
/// array-like access to tuple elements. The marker is non-enumerable so the
/// array still compares deep-equal to a plain array of the same elements
/// (and `Object.keys`/spreads see only the indices).
fn create_js_tuple<'e>(items: &[MontyObject], env: &'e Env) -> Result<Unknown<'e>> {
    let mut arr = create_js_array(items, env)?;
    let marker = create_js_bool(true, env)?;
    arr.define_properties(&[Property::new()
        .with_utf8_name("__tuple__")?
        .with_value(&marker)
        .with_property_attributes(PropertyAttributes::Writable | PropertyAttributes::Configurable)])?;
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
    let mut result = ptr::null_mut();
    // SAFETY: [DH] - all arguments are valid and result is valid on success
    unsafe {
        let status = sys::napi_call_function(env, this, method, 2, args.as_ptr(), &raw mut result);
        if status != Status::napi_ok {
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

/// Creates a JS object representing NotImplemented: `{ __monty_type__: 'NotImplemented' }`.
fn create_js_not_implemented(env: &Env) -> Result<Unknown<'_>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "NotImplemented")?;
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

/// Creates a JS object representing a Python `datetime.date`.
fn create_js_date<'e>(date: &MontyDate, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Date")?;
    obj.set_named_property("year", date.year)?;
    obj.set_named_property("month", date.month)?;
    obj.set_named_property("day", date.day)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a Python `datetime.timedelta`.
fn create_js_timedelta<'e>(delta: &MontyTimeDelta, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "TimeDelta")?;
    obj.set_named_property("days", delta.days)?;
    obj.set_named_property("seconds", delta.seconds)?;
    obj.set_named_property("microseconds", delta.microseconds)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a Python `datetime.timezone`.
fn create_js_timezone<'e>(timezone: &MontyTimeZone, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "TimeZone")?;
    obj.set_named_property("offsetSeconds", timezone.offset_seconds)?;
    if let Some(name) = &timezone.name {
        obj.set_named_property("name", name.clone())?;
    }
    obj.into_unknown(env)
}

/// Creates a JS object representing a Python `datetime.datetime`.
fn create_js_datetime<'e>(datetime: &MontyDateTime, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "DateTime")?;
    obj.set_named_property("year", datetime.year)?;
    obj.set_named_property("month", datetime.month)?;
    obj.set_named_property("day", datetime.day)?;
    obj.set_named_property("hour", datetime.hour)?;
    obj.set_named_property("minute", datetime.minute)?;
    obj.set_named_property("second", datetime.second)?;
    obj.set_named_property("microsecond", datetime.microsecond)?;
    if let Some(offset_seconds) = datetime.offset_seconds {
        obj.set_named_property("offsetSeconds", offset_seconds)?;
    }
    if let Some(timezone_name) = &datetime.timezone_name {
        obj.set_named_property("timezoneName", timezone_name.clone())?;
    }
    obj.into_unknown(env)
}

/// Creates a JS object representing a Python `datetime.time`.
fn create_js_time<'e>(time: &MontyTime, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Time")?;
    obj.set_named_property("hour", time.hour)?;
    obj.set_named_property("minute", time.minute)?;
    obj.set_named_property("second", time.second)?;
    obj.set_named_property("microsecond", time.microsecond)?;
    if let Some(offset_seconds) = time.offset_seconds {
        obj.set_named_property("offsetSeconds", offset_seconds)?;
    }
    if let Some(timezone_name) = &time.timezone_name {
        obj.set_named_property("timezoneName", timezone_name.clone())?;
    }
    obj.set_named_property("fold", time.fold)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a builtin Type:
/// `{ __monty_type__: 'Type', value: '...' }`.
fn create_js_type_marker<'e>(type_str: &str, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Type")?;
    obj.set_named_property("value", type_str)?;
    obj.into_unknown(env)
}

/// Creates a JS object representing a class type object:
/// `{ __monty_type__: 'Type', classType: { name, id, hostDefined, ... } }`.
fn create_js_class_type_marker<'e>(class_type: &ClassType, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "Type")?;
    obj.set_named_property("classType", create_js_class_type(class_type, env)?)?;
    obj.into_unknown(env)
}

/// Builds the plain `classType` object shared by Type and ClassInstance
/// markers. `parents` entries are Type markers (builtin `{ value }` or class
/// `{ classType }`), so the shape is recursive the same way the wire is.
fn create_js_class_type<'e>(class_type: &ClassType, env: &'e Env) -> Result<Object<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("name", class_type.name.as_str())?;
    // uuids as canonical lowercase strings — JS has no 128-bit integer type
    obj.set_named_property("id", class_type.id.to_string())?;
    obj.set_named_property("hostDefined", class_type.host_defined)?;
    let mut parents = env.create_array(class_type.parents.len().try_into().expect("parents size overflows u32"))?;
    for (i, parent) in class_type.parents.iter().enumerate() {
        let marker = match parent {
            MontyType::Instance(parent_class) => create_js_class_type_marker(parent_class, env)?,
            builtin => create_js_type_marker(&builtin.to_string(), env)?,
        };
        parents.set(i.try_into().expect("overflow on parents index"), marker)?;
    }
    obj.set_named_property("parents", parents)?;
    obj.set_named_property("isDataclass", class_type.is_dataclass)?;
    obj.set_named_property("frozen", class_type.frozen)?;
    Ok(obj)
}

/// Creates a JS object representing a builtin function.
fn create_js_builtin_function_marker<'e>(func_str: &str, env: &'e Env) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "BuiltinFunction")?;
    obj.set_named_property("value", func_str)?;
    obj.into_unknown(env)
}

/// Creates a JS marker object representing a sandbox file handle.
fn create_js_file_handle<'e>(handle: &MontyFileHandle, env: &'e Env) -> Result<Unknown<'e>> {
    if handle.position > JS_MAX_SAFE_POSITION {
        return Err(Error::from_reason(
            "MontyFileHandle position exceeds JavaScript's maximum safe integer",
        ));
    }

    let mut obj = Object::new(env)?;
    obj.set_named_property("path", handle.path.as_str())?;
    obj.set_named_property("mode", handle.mode.as_str())?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "position is within JavaScript's safe integer range"
    )]
    obj.set_named_property("position", handle.position as f64)?;

    let marker = env.create_string("FileHandle")?;
    let binary = create_js_bool(handle.mode.is_binary(), env)?;
    let readable = create_js_bool(handle.mode.readable(), env)?;
    let writable = create_js_bool(handle.mode.writable(), env)?;
    let hidden = PropertyAttributes::empty();
    obj.define_properties(&[
        Property::new()
            .with_utf8_name("__monty_type__")?
            .with_value(&marker)
            .with_property_attributes(hidden),
        Property::new()
            .with_utf8_name("binary")?
            .with_value(&binary)
            .with_property_attributes(hidden),
        Property::new()
            .with_utf8_name("readable")?
            .with_value(&readable)
            .with_property_attributes(hidden),
        Property::new()
            .with_utf8_name("writable")?
            .with_value(&writable)
            .with_property_attributes(hidden),
    ])?;
    obj.freeze()?;
    obj.into_unknown(env)
}

/// Creates the `ClassInstance` marker object for a class instance crossing
/// out of the sandbox.
///
/// `attrs` crosses as an array of `[name, value]` pairs (order preserved,
/// non-string keys skipped) rather than a plain object: attr names are
/// sandbox-controlled, and pair entries cannot clobber a prototype the way
/// `obj[k] = v` on a plain object could. The TS layer converts the marker to
/// the original wrapped instance or a `MontyClassProxy` proxy.
fn create_js_class_instance<'e>(
    class_type: &ClassType,
    instance_id: &MontyUuid,
    attrs: &DictPairs,
    env: &'e Env,
) -> Result<Unknown<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("__monty_type__", "ClassInstance")?;
    obj.set_named_property("type", create_js_class_type(class_type, env)?)?;
    // uuids as canonical lowercase strings — JS has no 128-bit integer type
    obj.set_named_property("instanceId", instance_id.to_string())?;

    let string_pairs: Vec<(&String, &MontyObject)> = attrs
        .into_iter()
        .filter_map(|(k, v)| match k {
            MontyObject::String(key) => Some((key, v)),
            _ => None,
        })
        .collect();
    let mut attrs_arr = env.create_array(string_pairs.len().try_into().expect("attrs size overflows u32"))?;
    for (i, (key, value)) in string_pairs.into_iter().enumerate() {
        let mut pair = env.create_array(2)?;
        pair.set(0, env.create_string(key)?)?;
        pair.set(1, monty_to_js(value, env)?)?;
        attrs_arr.set(i.try_into().expect("overflow on attrs index"), pair)?;
    }
    obj.set_named_property("attrs", attrs_arr)?;

    obj.into_unknown(env)
}

// =============================================================================
// JS to Monty conversion
// =============================================================================

/// Converts a JavaScript value to Monty's `MontyObject`, handling native JS
/// types and `__monty_type__`-marked objects:
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
            // Integral numbers within i64 become Python ints. The i64 range
            // check must be half-open: `i64::MIN as f64` is exactly -2^63,
            // but `i64::MAX as f64` rounds *up* to 2^63 — a value of exactly
            // 2^63 does not fit in i64 (`as` would saturate, silently
            // changing the value), so it crosses as a float instead.
            if n.fract() == 0.0 && n >= i64::MIN as f64 && n < -(i64::MIN as f64) {
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

            // `words` are 64-bit limbs in little-endian order; reassemble into
            // a num-bigint and apply `sign_bit`.
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

            if obj.is_buffer()? {
                let buffer: BufferSlice = BufferSlice::from_unknown(value)?;
                return Ok(MontyObject::Bytes(buffer.to_vec()));
            }
            if is_js_map(&obj, env)? {
                return js_map_to_monty(obj, env);
            }
            if is_js_set(&obj, env)? {
                return js_set_to_monty(obj, env);
            }
            if obj.is_array()? {
                return js_array_to_monty(obj, env);
            }
            if let Some(monty_type) = get_string_property(&obj, "__monty_type__")? {
                return js_marked_object_to_monty(&obj, &monty_type, env);
            }

            // Plain object → Dict (with string keys)
            js_object_to_monty_dict(obj, env)
        }
        ValueType::Function => {
            // JS functions become MontyObject::Function (keyed by `name`) for
            // external function resolution.
            let func_obj: Object = value.coerce_to_object()?;
            let name: String = func_obj
                .get_named_property::<String>("name")
                .unwrap_or_else(|_| "<anonymous>".to_string());
            Ok(MontyObject::Function { name, docstring: None })
        }
        ValueType::Symbol | ValueType::External => {
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
        "NotImplemented" => Ok(MontyObject::NotImplemented),
        "Exception" => {
            let exc_type_str: String = obj.get_named_property("excType")?;
            let message: String = obj.get_named_property("message")?;
            let exc_type: ExcType = exc_type_str
                .parse()
                .map_err(|_| Error::from_reason(format!("Unknown exception type: {exc_type_str}")))?;
            let arg = if message.is_empty() { None } else { Some(message) };
            Ok(MontyObject::Exception { exc_type, arg })
        }
        "Date" => Ok(MontyObject::Date(MontyDate {
            year: obj.get_named_property::<i32>("year")?,
            month: obj.get_named_property::<u8>("month")?,
            day: obj.get_named_property::<u8>("day")?,
        })),
        "DateTime" => Ok(MontyObject::DateTime(MontyDateTime {
            year: obj.get_named_property::<i32>("year")?,
            month: obj.get_named_property::<u8>("month")?,
            day: obj.get_named_property::<u8>("day")?,
            hour: obj.get_named_property::<u8>("hour")?,
            minute: obj.get_named_property::<u8>("minute")?,
            second: obj.get_named_property::<u8>("second")?,
            microsecond: obj.get_named_property::<u32>("microsecond")?,
            offset_seconds: obj.get_named_property::<Option<i32>>("offsetSeconds")?,
            timezone_name: obj.get_named_property::<Option<String>>("timezoneName")?,
        })),
        "Time" => Ok(MontyObject::Time(MontyTime {
            hour: obj.get_named_property::<u8>("hour")?,
            minute: obj.get_named_property::<u8>("minute")?,
            second: obj.get_named_property::<u8>("second")?,
            microsecond: obj.get_named_property::<u32>("microsecond")?,
            offset_seconds: obj.get_named_property::<Option<i32>>("offsetSeconds")?,
            timezone_name: obj.get_named_property::<Option<String>>("timezoneName")?,
            fold: obj.get_named_property::<Option<u8>>("fold")?.unwrap_or(0),
        })),
        "TimeDelta" => Ok(MontyObject::TimeDelta(MontyTimeDelta {
            days: obj.get_named_property::<i32>("days")?,
            seconds: obj.get_named_property::<i32>("seconds")?,
            microseconds: obj.get_named_property::<i32>("microseconds")?,
        })),
        "TimeZone" => Ok(MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: obj.get_named_property::<i32>("offsetSeconds")?,
            name: obj.get_named_property::<Option<String>>("name")?,
        })),
        "Type" => {
            // A class type (ClassType wrapper, or a round-tripped host class)
            // crosses structurally; builtin type objects can't be
            // round-tripped from JS and degrade to Repr.
            if obj.has_named_property("classType")? {
                let class_type: Object = obj.get_named_property("classType")?;
                Ok(MontyObject::Type(MontyType::Instance(Box::new(parse_js_class_type(
                    &class_type,
                )?))))
            } else {
                let value: String = obj.get_named_property("value")?;
                Ok(MontyObject::Repr(format!("<class '{value}'>")))
            }
        }
        "BuiltinFunction" => {
            // BuiltinFunction objects can't be fully round-tripped; return as Repr
            let value: String = obj.get_named_property("value")?;
            Ok(MontyObject::Repr(format!("<built-in function {value}>")))
        }
        "FileHandle" => {
            let path = get_required_string_property(obj, "path", "MontyFileHandle")?;
            let mode = get_required_string_property(obj, "mode", "MontyFileHandle")?;
            let mode: FileMode = mode
                .parse()
                .map_err(|error: Cow<'static, str>| Error::from_reason(error.into_owned()))?;
            let position = get_file_handle_position(obj)?;
            Ok(MontyObject::FileHandle(MontyFileHandle { path, mode, position }))
        }
        "ClassInstance" => {
            let class_type: Object = obj.get_named_property("type")?;
            let class_type = parse_js_class_type(&class_type)?;
            let instance_id = get_uuid_string_property(obj, "instanceId", "ClassInstance")?;

            let attrs_arr: Array = obj.get_named_property("attrs")?;
            let mut attrs_vec = Vec::with_capacity(attrs_arr.len() as usize);
            for i in 0..attrs_arr.len() {
                let Some(pair) = attrs_arr.get::<Array>(i)? else {
                    return Err(Error::from_reason(
                        "ClassInstance attrs entries must be [name, value] pairs",
                    ));
                };
                let key: String = pair
                    .get::<String>(0)?
                    .ok_or_else(|| Error::from_reason("ClassInstance attr name must be a string"))?;
                let value = pair
                    .get::<Unknown>(1)?
                    .ok_or_else(|| Error::from_reason("ClassInstance attr value missing"))?;
                attrs_vec.push((MontyObject::String(key), js_to_monty(value, env)?));
            }

            Ok(MontyObject::ClassInstance {
                class_type,
                instance_id,
                attrs: DictPairs::from(attrs_vec),
            })
        }
        _ => Err(Error::from_reason(format!("Unknown Monty marker type: {monty_type}"))),
    }
}

/// Reads a canonical uuid string property (instance/type ids).
fn get_uuid_string_property(obj: &Object, key: &str, type_name: &str) -> Result<MontyUuid> {
    let value: String = obj.get_named_property(key)?;
    MontyUuid::parse(&value).ok_or_else(|| {
        Error::from_reason(format!(
            "{type_name} {key} must be a canonical uuid string, got {value:?}"
        ))
    })
}

/// Parses the plain `classType` object of a Type / ClassInstance marker.
fn parse_js_class_type(obj: &Object) -> Result<ClassType> {
    let name: String = obj.get_named_property("name")?;
    let id = get_uuid_string_property(obj, "id", "ClassType")?;
    let host_defined: bool = obj.get_named_property("hostDefined")?;
    let parents_arr: Array = obj.get_named_property("parents")?;
    let mut parents = Vec::with_capacity(parents_arr.len() as usize);
    for i in 0..parents_arr.len() {
        let marker = parents_arr
            .get::<Object>(i)?
            .ok_or_else(|| Error::from_reason("ClassType parents entries must be Type markers"))?;
        if marker.has_named_property("classType")? {
            let class_type: Object = marker.get_named_property("classType")?;
            parents.push(MontyType::Instance(Box::new(parse_js_class_type(&class_type)?)));
        } else {
            let value: String = marker.get_named_property("value")?;
            let builtin = MontyType::from_type_name(&value)
                .ok_or_else(|| Error::from_reason(format!("unknown builtin type name {value:?}")))?;
            parents.push(builtin);
        }
    }
    Ok(ClassType {
        name,
        id,
        host_defined,
        parents,
        is_dataclass: obj.get_named_property("isDataclass")?,
        frozen: obj.get_named_property("frozen")?,
    })
}

/// Reads and validates the optional JavaScript-safe file position.
fn get_file_handle_position(obj: &Object) -> Result<u64> {
    if !obj.has_named_property("position")? {
        return Ok(0);
    }

    let value: Unknown = obj.get_named_property("position")?;
    if value.get_type()? == ValueType::Undefined {
        return Ok(0);
    }
    if value.get_type()? != ValueType::Number {
        return Err(Error::from_reason(
            "MontyFileHandle position must be a non-negative safe integer",
        ));
    }

    let position = value.coerce_to_number()?.get_double()?;
    if position.is_finite() && position.fract() == 0.0 && (0.0..=JS_MAX_SAFE_POSITION_F64).contains(&position) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "validated as a non-negative safe integer"
        )]
        Ok(position as u64)
    } else {
        Err(Error::from_reason(
            "MontyFileHandle position must be a non-negative safe integer",
        ))
    }
}

/// Reads a required string field from a marked object without coercion.
fn get_required_string_property(obj: &Object, name: &str, marker: &str) -> Result<String> {
    if !obj.has_named_property(name)? {
        return Err(Error::from_reason(format!("{marker} {name} must be a string")));
    }
    let value: Unknown = obj.get_named_property(name)?;
    if value.get_type()? == ValueType::String {
        value.coerce_to_string()?.into_utf8()?.into_owned()
    } else {
        Err(Error::from_reason(format!("{marker} {name} must be a string")))
    }
}

/// Converts a plain JS object to `MontyObject::Dict`.
///
/// This is a fallback for plain objects (not Map instances). Since JS object keys
/// are always strings, all keys in the resulting Dict will be strings.
/// For full key type preservation, use JS `Map` instead.
fn js_object_to_monty_dict(obj: Object, env: Env) -> Result<MontyObject> {
    let keys = obj.get_property_names()?;
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
