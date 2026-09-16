//! Bidirectional conversion between the sandbox boundary's value arenas and
//! JavaScript values via napi-rs: [`DecodedArena`] / [`monty_to_js`] for
//! values flowing out, [`GraphEncoder`] / [`js_to_monty`] for values flowing
//! in. Sharing survives both ways: one JS object referenced twice in a
//! message crosses as one node, and one node decodes to one JS object.
//!
//! ## Type Mappings
//!
//! ### Native JS types (bidirectional):
//! - `None` ↔ `null`
//! - `Bool` ↔ `boolean`
//! - `Int` ↔ `number` (if within safe integer range) or `BigInt`
//! - `BigInt` ↔ `BigInt`
//! - `Float` ↔ `number` (including `NaN`, `Infinity`, `-Infinity`)
//! - `String` ↔ `string`
//! - `Bytes` ↔ `Buffer` (Node.js)
//! - `List` ↔ `Array`
//! - `Dict` ↔ `Map` (preserves key types and insertion order)
//! - `Set` ↔ `Set`
//! - `FrozenSet` ↔ `Set` (JS has no frozen set)
//!
//! ### Marked JS types (with `__monty_type__` property):
//! - `Ellipsis` → `{ __monty_type__: 'Ellipsis' }`
//! - `Tuple` → `Array` with `__tuple__: true`
//! - `Exception` → `{ __monty_type__: 'Exception', excType, message }`
//! - `Type` → `{ __monty_type__: 'Type', value }`
//! - `BuiltinFunction` → `{ __monty_type__: 'BuiltinFunction', value }`
//! - `ClassInstance` → `{ __monty_type__: 'ClassInstance', type, instanceId, attrs }`
//! - `FileHandle` ↔ `{ __monty_type__: 'FileHandle', path, mode, position }`
//! - `Repr` → plain `string`
//! - `Cycle` → placeholder `string`
#![expect(unsafe_code, reason = "napi API is unsafe")]

use std::{borrow::Cow, collections::HashMap, ptr, vec::IntoIter};

use monty_types::{
    ClassTypeNode, ExcType, FileMode, MontyDate, MontyDateTime, MontyFileHandle, MontyGraph, MontyNode, MontyObject,
    MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NodeId,
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

/// Converts one value to a JS value, using native JS types where possible
/// (`number`/`BigInt`, `Map`, `Set`, `Buffer`, `__tuple__`-marked arrays).
/// Types without a JS equivalent get `__monty_type__` marker properties so
/// they round-trip.
pub fn monty_to_js<'e>(value: &MontyObject, env: &'e Env) -> Result<JsMontyObject<'e>> {
    Ok(JsMontyObject(DecodedArena::new(&value.graph, env)?.get(value.root)))
}

/// One message's arena as JS values.
///
/// Every node is converted once, in arena order, so a child always exists
/// before its holder and a node referenced twice (a shared sub-object, an
/// argument passed twice) is one JS object. Instances of one class share its
/// `classType` object. The pass is a loop, not a recursion, so nesting depth
/// costs nothing on the native stack.
pub struct DecodedArena<'e> {
    built: Vec<Unknown<'e>>,
}

impl<'e> DecodedArena<'e> {
    /// Decodes every node of `graph`.
    pub fn new(graph: &MontyGraph, env: &'e Env) -> Result<Self> {
        let mut built = Vec::with_capacity(graph.len());
        for node in graph.nodes() {
            let value = decode_node(node, graph, &built, env)?;
            built.push(value);
        }
        Ok(Self { built })
    }

    /// The value for `id`, which the arena's validation put in range.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Unknown<'e> {
        self.built[id.index()]
    }
}

/// Converts one node; every child id is lower, so already in `built`.
fn decode_node<'e>(node: &MontyNode, graph: &MontyGraph, built: &[Unknown<'e>], env: &'e Env) -> Result<Unknown<'e>> {
    let child = |id: &NodeId| built[id.index()];
    let children = |ids: &[NodeId]| ids.iter().map(child).collect::<Vec<_>>();
    match node {
        MontyNode::None => create_js_null(env),
        MontyNode::Ellipsis => create_js_ellipsis(env),
        MontyNode::NotImplemented => create_js_not_implemented(env),
        MontyNode::Bool(b) => create_js_bool(*b, env),
        MontyNode::Int(i) => create_js_int(*i, env),
        MontyNode::BigInt(bi) => create_js_bigint(bi, env),
        MontyNode::Float(f) => env.create_double(*f)?.into_unknown(env),
        MontyNode::String(s) => env.create_string(s)?.into_unknown(env),
        MontyNode::Bytes(bytes) => create_js_buffer(bytes, env),
        MontyNode::List(items) => create_js_array(&children(items), env)?.into_unknown(env),
        MontyNode::Tuple(items) => create_js_tuple(&children(items), env),
        // NamedTuple is converted to a tuple (loses named access in JS)
        MontyNode::NamedTuple { values, .. } => create_js_tuple(&children(values), env),
        MontyNode::Dict(pairs) => create_js_map(pairs.iter().map(|(k, v)| (child(k), child(v))), env),
        MontyNode::Set(items) | MontyNode::FrozenSet(items) => create_js_set(&children(items), env),
        MontyNode::Exception { exc_type, arg } => create_js_exception(*exc_type, arg.as_deref(), env),
        MontyNode::Date(date) => create_js_date(date, env),
        MontyNode::DateTime(datetime) => create_js_datetime(datetime, env),
        MontyNode::Time(time) => create_js_time(time, env),
        MontyNode::TimeDelta(delta) => create_js_timedelta(delta, env),
        MontyNode::TimeZone(timezone) => create_js_timezone(timezone, env),
        MontyNode::Type(t) => create_js_type_marker(&t.to_string(), env),
        MontyNode::ClassType(class) => {
            let mut obj = Object::new(env)?;
            obj.set_named_property("__monty_type__", "Type")?;
            obj.set_named_property("classType", create_js_class_type(class, graph, built, env)?)?;
            obj.into_unknown(env)
        }
        MontyNode::BuiltinFunction(f) => create_js_builtin_function_marker(&f.to_string(), env),
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => {
            // the class object decoded for the instance's class node, so
            // every instance of a class shares it
            let class_marker: Object = child(class_type).coerce_to_object()?;
            let class_object: Object = class_marker.get_named_property("classType")?;
            let mut obj = Object::new(env)?;
            obj.set_named_property("__monty_type__", "ClassInstance")?;
            obj.set_named_property("type", class_object)?;
            // uuids as canonical lowercase strings — JS has no 128-bit integer type
            obj.set_named_property("instanceId", instance_id.to_string())?;
            obj.set_named_property("attrs", create_js_attr_pairs(attrs, graph, built, env)?)?;
            obj.into_unknown(env)
        }
        MontyNode::Path(p) => env.create_string(p)?.into_unknown(env),
        MontyNode::FileHandle(handle) => create_js_file_handle(handle, env),
        MontyNode::Repr(s) | MontyNode::Cycle(s) => env.create_string(s)?.into_unknown(env),
        // Function objects are internal to the name lookup protocol and should not normally
        // appear as final output values. If they do, represent as a string with the function name.
        MontyNode::Function { name, .. } => env.create_string(name)?.into_unknown(env),
    }
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

/// Creates a native JS Array from already-decoded items.
fn create_js_array<'e>(items: &[Unknown<'e>], env: &'e Env) -> Result<Array<'e>> {
    let mut arr = env.create_array(items.len().try_into().expect("array size overflows u32"))?;
    for (i, item) in items.iter().enumerate() {
        arr.set(i.try_into().expect("overflow on array index"), *item)?;
    }
    Ok(arr)
}

/// Creates a tuple representation as a JS array with a `__tuple__` marker property.
///
/// This allows distinguishing tuples from lists in JavaScript while still allowing
/// array-like access to tuple elements. The marker is non-enumerable so the
/// array still compares deep-equal to a plain array of the same elements
/// (and `Object.keys`/spreads see only the indices).
fn create_js_tuple<'e>(items: &[Unknown<'e>], env: &'e Env) -> Result<Unknown<'e>> {
    let mut arr = create_js_array(items, env)?;
    let marker = create_js_bool(true, env)?;
    arr.define_properties(&[Property::new()
        .with_utf8_name("__tuple__")?
        .with_value(&marker)
        .with_property_attributes(PropertyAttributes::Writable | PropertyAttributes::Configurable)])?;
    arr.into_unknown(env)
}

/// Creates a native JS `Map` from already-decoded key/value pairs.
///
/// Using `Map` instead of plain objects preserves:
/// - Non-string key types (numbers, booleans, etc.)
/// - Insertion order
/// - Proper equality semantics for keys
fn create_js_map<'e>(pairs: impl Iterator<Item = (Unknown<'e>, Unknown<'e>)>, env: &'e Env) -> Result<Unknown<'e>> {
    let map = new_js_map(env)?;
    let set_method: Unknown = map.get_named_property("set")?;
    for (js_key, js_value) in pairs {
        call_method_2_args(env.raw(), map.raw(), set_method.raw(), js_key.raw(), js_value.raw())?;
    }
    map.into_unknown(env)
}

/// A fresh JS `Map`.
fn new_js_map(env: &Env) -> Result<Object<'_>> {
    let global = env.get_global()?;
    let map_constructor: Function<()> = global.get_named_property("Map")?;
    map_constructor.new_instance(())?.coerce_to_object()
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

/// Creates a native JS Set from already-decoded items.
fn create_js_set<'e>(items: &[Unknown<'e>], env: &'e Env) -> Result<Unknown<'e>> {
    let global = env.get_global()?;
    let set_constructor: Function<()> = global.get_named_property("Set")?;
    let set: Object<'e> = set_constructor.new_instance(())?.coerce_to_object()?;

    let add_method: Function = set.get_named_property("add")?;
    for item in items {
        add_method.apply(set, *item)?;
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

/// Builds the plain `classType` object a `Type` marker carries and every
/// instance of the class shares: name, uuid, flags and the eager class attrs.
fn create_js_class_type<'e>(
    class: &ClassTypeNode,
    graph: &MontyGraph,
    built: &[Unknown<'e>],
    env: &'e Env,
) -> Result<Object<'e>> {
    let mut obj = Object::new(env)?;
    obj.set_named_property("name", class.name.as_str())?;
    // uuids as canonical lowercase strings — JS has no 128-bit integer type
    obj.set_named_property("id", class.id.to_string())?;
    obj.set_named_property("hostDefined", class.host_defined)?;
    obj.set_named_property("isDataclass", class.is_dataclass)?;
    obj.set_named_property("attrs", create_js_attr_pairs(&class.attrs, graph, built, env)?)?;
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

/// Builds the `[name, value]` pair array attrs cross as (order preserved,
/// non-string keys skipped): attr names are sandbox-controlled, and pair
/// entries cannot clobber a prototype the way `obj[k] = v` on a plain object
/// could. The TS layer maps a `ClassInstance` marker to the original wrapped
/// instance or a `MontyClassProxy`.
fn create_js_attr_pairs<'e>(
    attrs: &[(NodeId, NodeId)],
    graph: &MontyGraph,
    built: &[Unknown<'e>],
    env: &'e Env,
) -> Result<Array<'e>> {
    let string_pairs: Vec<(&str, Unknown<'e>)> = attrs
        .iter()
        .filter_map(|(key, value)| match graph.node(*key) {
            MontyNode::String(name) => Some((name.as_str(), built[value.index()])),
            _ => None,
        })
        .collect();
    let mut attrs_arr = env.create_array(string_pairs.len().try_into().expect("attrs size overflows u32"))?;
    for (i, (key, value)) in string_pairs.into_iter().enumerate() {
        let mut pair = env.create_array(2)?;
        pair.set(0, env.create_string(key)?)?;
        pair.set(1, value)?;
        attrs_arr.set(i.try_into().expect("overflow on attrs index"), pair)?;
    }
    Ok(attrs_arr)
}

// =============================================================================
// JS to Monty conversion
// =============================================================================

/// Encodes one JS value as its own arena.
///
/// The single-value form of [`GraphEncoder`]: a return value, a resumed
/// lookup. Values that share one message (a feed's inputs) go through one
/// encoder so their sharing survives.
pub fn js_to_monty<'e>(value: Unknown<'e>, env: &'e Env) -> Result<MontyObject> {
    let mut encoder = GraphEncoder::new(env)?;
    let root = encoder.push(value)?;
    Ok(encoder.finish_object(root))
}

/// Builds one message's arena from JS values, preserving sharing.
///
/// Handles native JS types and `__monty_type__`-marked objects:
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
///
/// Every container, class instance and class type is memoized by JS identity
/// (in a JS `Map`, which also keeps it alive) for the life of the encoder, so
/// pushing the same object twice yields the same node id and the sandbox sees
/// one object; leaves, including leaf markers such as dates, are immutable
/// values and are re-encoded per reference. Nesting is walked on an explicit
/// stack, so depth is bounded by memory, not the native stack. A cycle is an
/// error: the arena is post-order, so a value cannot reach itself.
pub struct GraphEncoder<'e> {
    env: &'e Env,
    graph: MontyGraph,
    /// JS `Map` from container → node index, or `-1` while its children are
    /// still being pushed (a hit on that marker is a cycle).
    memo: Object<'e>,
    /// Class nodes with no eager attrs, one per class id: every instance of a
    /// class shares its node, as the sandbox's export does.
    class_types: HashMap<MontyUuid, NodeId>,
}

impl<'e> GraphEncoder<'e> {
    /// An empty arena for one message.
    pub fn new(env: &'e Env) -> Result<Self> {
        Ok(Self {
            env,
            graph: MontyGraph::new(),
            memo: new_js_map(env)?,
            class_types: HashMap::new(),
        })
    }

    /// Encodes `value` into the arena and returns its node; an object pushed
    /// before returns the node it already has.
    pub fn push(&mut self, value: Unknown<'e>) -> Result<NodeId> {
        let mut stack: Vec<Frame<'e>> = Vec::new();
        let mut next = Child::Value(value);
        loop {
            // descend until a leaf, a memo hit, or an empty container
            let mut done = match self.step(next)? {
                Step::Done(id) => id,
                Step::Enter(mut frame) => match frame.children.next() {
                    Some(child) => {
                        stack.push(frame);
                        next = child;
                        continue;
                    }
                    None => self.complete(frame)?,
                },
            };
            // hand the finished node up, completing every holder it finishes
            loop {
                let Some(mut frame) = stack.pop() else {
                    return Ok(done);
                };
                frame.ids.push(done);
                if let Some(child) = frame.children.next() {
                    stack.push(frame);
                    next = child;
                    break;
                }
                done = self.complete(frame)?;
            }
        }
    }

    /// The arena, once every root has been pushed.
    #[must_use]
    pub fn finish(self) -> MontyGraph {
        self.graph
    }

    /// The arena as one value rooted at `root`, an id [`push`](Self::push) returned.
    #[must_use]
    pub fn finish_object(self, root: NodeId) -> MontyObject {
        // `push` returned `root`, so it is in range
        MontyObject {
            graph: self.graph,
            root,
        }
    }

    /// Resolves one pending child: a leaf is pushed at once, a container
    /// opens a frame for its children.
    fn step(&mut self, child: Child<'e>) -> Result<Step<'e>> {
        match child {
            Child::Value(value) => self.encode(value),
            Child::ClassType(object) => self.enter_class_type(object),
        }
    }

    /// Dispatches on the JS type.
    fn encode(&mut self, value: Unknown<'e>) -> Result<Step<'e>> {
        let env = self.env;
        let value_type = value.get_type()?;
        match value_type {
            ValueType::Null | ValueType::Undefined => Ok(self.leaf(MontyNode::None)),
            ValueType::Boolean => Ok(self.leaf(MontyNode::Bool(value.coerce_to_bool()?))),
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
                    return Ok(self.leaf(MontyNode::Int(n as i64)));
                }
                Ok(self.leaf(MontyNode::Float(n)))
            }
            ValueType::BigInt => {
                let bigint: BigInt = BigInt::from_unknown(value)?;
                // `words` are 64-bit limbs in little-endian order; reassemble into
                // a num-bigint and apply `sign_bit`.
                if bigint.words.is_empty() {
                    return Ok(self.leaf(MontyNode::Int(0)));
                }
                let mut bi = NumBigInt::from(0u64);
                for (i, &word) in bigint.words.iter().enumerate() {
                    let limb = NumBigInt::from(word);
                    bi += limb << (64 * i);
                }
                if bigint.sign_bit {
                    bi = -bi;
                }
                Ok(match i64::try_from(&bi) {
                    Ok(i) => self.leaf(MontyNode::Int(i)),
                    Err(_) => self.leaf(MontyNode::BigInt(bi)),
                })
            }
            ValueType::String => {
                let s: String = value.coerce_to_string()?.into_utf8()?.into_owned()?;
                Ok(self.leaf(MontyNode::String(s)))
            }
            ValueType::Object => {
                let obj: Object = value.coerce_to_object()?;
                if obj.is_buffer()? {
                    let buffer: BufferSlice = BufferSlice::from_unknown(value)?;
                    Ok(self.leaf(MontyNode::Bytes(buffer.to_vec())))
                } else if is_js_map(&obj, env)? {
                    self.enter(obj, |_| Ok((Pending::Dict, js_map_entries(&obj)?)))
                } else if is_js_set(&obj, env)? {
                    self.enter(obj, |_| Ok((Pending::Set, js_set_values(&obj)?)))
                } else if obj.is_array()? {
                    let is_tuple: bool = obj.get_named_property::<Option<bool>>("__tuple__")?.unwrap_or(false);
                    let pending = if is_tuple { Pending::Tuple } else { Pending::List };
                    self.enter(obj, |_| Ok((pending, js_array_items(&obj)?)))
                } else if let Some(monty_type) = get_string_property(&obj, "__monty_type__")? {
                    self.encode_marked(obj, &monty_type)
                } else {
                    // plain object → Dict (with string keys)
                    self.enter(obj, |_| Ok((Pending::Dict, js_object_entries(&obj, env)?)))
                }
            }
            ValueType::Function => {
                // JS functions become `Function` nodes (keyed by `name`) for
                // external function resolution.
                let func_obj: Object = value.coerce_to_object()?;
                let name: String = func_obj
                    .get_named_property::<String>("name")
                    .unwrap_or_else(|_| "<anonymous>".to_string());
                Ok(self.leaf(MontyNode::Function { name, docstring: None }))
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

    /// Converts a JS object with a `__monty_type__` marker.
    fn encode_marked(&mut self, obj: Object<'e>, monty_type: &str) -> Result<Step<'e>> {
        let node = match monty_type {
            "Ellipsis" => MontyNode::Ellipsis,
            "NotImplemented" => MontyNode::NotImplemented,
            "Exception" => {
                let exc_type_str: String = obj.get_named_property("excType")?;
                let message: String = obj.get_named_property("message")?;
                let exc_type: ExcType = exc_type_str
                    .parse()
                    .map_err(|_| Error::from_reason(format!("Unknown exception type: {exc_type_str}")))?;
                let arg = if message.is_empty() { None } else { Some(message) };
                MontyNode::Exception { exc_type, arg }
            }
            "Date" => MontyNode::Date(MontyDate {
                year: obj.get_named_property::<i32>("year")?,
                month: obj.get_named_property::<u8>("month")?,
                day: obj.get_named_property::<u8>("day")?,
            }),
            "DateTime" => MontyNode::DateTime(MontyDateTime {
                year: obj.get_named_property::<i32>("year")?,
                month: obj.get_named_property::<u8>("month")?,
                day: obj.get_named_property::<u8>("day")?,
                hour: obj.get_named_property::<u8>("hour")?,
                minute: obj.get_named_property::<u8>("minute")?,
                second: obj.get_named_property::<u8>("second")?,
                microsecond: obj.get_named_property::<u32>("microsecond")?,
                offset_seconds: obj.get_named_property::<Option<i32>>("offsetSeconds")?,
                timezone_name: obj.get_named_property::<Option<String>>("timezoneName")?,
            }),
            "Time" => MontyNode::Time(MontyTime {
                hour: obj.get_named_property::<u8>("hour")?,
                minute: obj.get_named_property::<u8>("minute")?,
                second: obj.get_named_property::<u8>("second")?,
                microsecond: obj.get_named_property::<u32>("microsecond")?,
                offset_seconds: obj.get_named_property::<Option<i32>>("offsetSeconds")?,
                timezone_name: obj.get_named_property::<Option<String>>("timezoneName")?,
                fold: obj.get_named_property::<Option<u8>>("fold")?.unwrap_or(0),
            }),
            "TimeDelta" => MontyNode::TimeDelta(MontyTimeDelta {
                days: obj.get_named_property::<i32>("days")?,
                seconds: obj.get_named_property::<i32>("seconds")?,
                microseconds: obj.get_named_property::<i32>("microseconds")?,
            }),
            "TimeZone" => MontyNode::TimeZone(MontyTimeZone {
                offset_seconds: obj.get_named_property::<i32>("offsetSeconds")?,
                name: obj.get_named_property::<Option<String>>("name")?,
            }),
            "Type" => {
                // A class type (ClassType wrapper, or a round-tripped host class)
                // crosses structurally; a builtin type marker carries only its
                // name, resolved the same way the wasm worker path does.
                return if obj.has_named_property("classType")? {
                    let class_type: Object = obj.get_named_property("classType")?;
                    self.enter_class_type(class_type)
                } else {
                    let value: String = obj.get_named_property("value")?;
                    let t = MontyType::from_type_name(&value)
                        .ok_or_else(|| Error::from_reason(format!("unknown type name {value:?}")))?;
                    Ok(self.leaf(MontyNode::Type(t)))
                };
            }
            // BuiltinFunction objects can't be fully round-tripped; return as Repr
            "BuiltinFunction" => {
                let value: String = obj.get_named_property("value")?;
                MontyNode::Repr(format!("<built-in function {value}>"))
            }
            "FileHandle" => {
                let path = get_required_string_property(&obj, "path", "MontyFileHandle")?;
                let mode = get_required_string_property(&obj, "mode", "MontyFileHandle")?;
                let mode: FileMode = mode
                    .parse()
                    .map_err(|error: Cow<'static, str>| Error::from_reason(error.into_owned()))?;
                let position = get_file_handle_position(&obj)?;
                MontyNode::FileHandle(MontyFileHandle { path, mode, position })
            }
            "ClassInstance" => {
                return self.enter(obj, |_| {
                    let class_type: Object = obj.get_named_property("type")?;
                    let instance_id = get_uuid_string_property(&obj, "instanceId", "ClassInstance")?;
                    let mut children = vec![Child::ClassType(class_type)];
                    children.extend(js_attr_pairs(obj.get_named_property("attrs")?, "ClassInstance")?);
                    Ok((Pending::ClassInstance { instance_id }, children))
                });
            }
            _ => return Err(Error::from_reason(format!("Unknown Monty marker type: {monty_type}"))),
        };
        Ok(self.leaf(node))
    }

    /// Pushes a leaf node.
    fn leaf(&mut self, node: MontyNode) -> Step<'e> {
        Step::Done(self.graph.push(node))
    }

    /// Opens a frame for a container, unless it is memoized: a finished one
    /// returns its node, one still being pushed is a cycle.
    fn enter(
        &mut self,
        obj: Object<'e>,
        begin: impl FnOnce(&mut Self) -> Result<(Pending, Vec<Child<'e>>)>,
    ) -> Result<Step<'e>> {
        match self.memo_get(obj)? {
            Some(Slot::Done(id)) => Ok(Step::Done(id)),
            Some(Slot::InProgress) => Err(Error::from_reason("Circular reference detected")),
            None => {
                self.memo_set(obj, Slot::InProgress)?;
                let (pending, children) = begin(self)?;
                Ok(Step::Enter(Frame::new(pending, Some(obj), children)))
            }
        }
    }

    /// Opens a frame for a class node, or reuses one: the same `classType`
    /// object gives the same node, and a class with no eager attrs has one
    /// node per id however it is spelled. A class met again while its own
    /// attrs are being pushed (a class constant that is an instance of the
    /// class) gets an attr-less duplicate rather than a cycle error, as the
    /// sandbox's export does.
    fn enter_class_type(&mut self, object: Object<'e>) -> Result<Step<'e>> {
        let header = ClassHeader::read(&object)?;
        match self.memo_get(object)? {
            Some(Slot::Done(id)) => return Ok(Step::Done(id)),
            Some(Slot::InProgress) => return Ok(self.leaf(header.node(vec![]))),
            None => self.memo_set(object, Slot::InProgress)?,
        }
        let children = js_attr_pairs(object.get_named_property("attrs")?, "ClassType")?;
        if children.is_empty() {
            if let Some(id) = self.class_types.get(&header.id).copied() {
                self.memo_set(object, Slot::Done(id))?;
                return Ok(Step::Done(id));
            }
        }
        Ok(Step::Enter(Frame::new(
            Pending::ClassType(header),
            Some(object),
            children,
        )))
    }

    /// Builds a container's node once every child id is known.
    fn complete(&mut self, frame: Frame<'e>) -> Result<NodeId> {
        let ids = frame.ids;
        let node = match frame.pending {
            Pending::List => MontyNode::List(ids),
            Pending::Tuple => MontyNode::Tuple(ids),
            Pending::Set => MontyNode::Set(ids),
            Pending::Dict => MontyNode::Dict(id_pairs(&ids)),
            Pending::ClassType(header) => header.node(id_pairs(&ids)),
            Pending::ClassInstance { instance_id } => MontyNode::ClassInstance {
                class_type: ids[0],
                instance_id,
                attrs: id_pairs(&ids[1..]),
            },
        };
        let attr_less_class = match &node {
            MontyNode::ClassType(class) if class.attrs.is_empty() => Some(class.id),
            _ => None,
        };
        let id = self.graph.push(node);
        if let Some(class_id) = attr_less_class {
            self.class_types.entry(class_id).or_insert(id);
        }
        if let Some(obj) = frame.key {
            self.memo_set(obj, Slot::Done(id))?;
        }
        Ok(id)
    }

    /// The memo entry for `obj`, if any.
    fn memo_get(&self, obj: Object<'e>) -> Result<Option<Slot>> {
        let get: Function<Unknown, Unknown> = self.memo.get_named_property("get")?;
        let entry = get.apply(self.memo, obj.into_unknown(self.env)?)?;
        if entry.get_type()? == ValueType::Undefined {
            Ok(None)
        } else {
            let index: i64 = entry.coerce_to_number()?.get_int64()?;
            Ok(Some(if index < 0 {
                Slot::InProgress
            } else {
                Slot::Done(NodeId(u32::try_from(index).map_err(|_| invalid_memo())?))
            }))
        }
    }

    /// Records the memo entry for `obj`.
    fn memo_set(&self, obj: Object<'e>, slot: Slot) -> Result<()> {
        let env = self.env;
        let index = match slot {
            Slot::InProgress => -1,
            Slot::Done(id) => i64::from(id.0),
        };
        let set: Unknown = self.memo.get_named_property("set")?;
        let index = env.create_int64(index)?;
        call_method_2_args(env.raw(), self.memo.raw(), set.raw(), obj.raw(), index.raw())
    }
}

/// A memoized container's state.
enum Slot {
    /// Its children are still being pushed.
    InProgress,
    /// Its node.
    Done(NodeId),
}

/// The outcome of resolving one pending child.
enum Step<'e> {
    /// A leaf, or a container already in the arena.
    Done(NodeId),
    /// A container whose children come next.
    Enter(Frame<'e>),
}

/// A value still to be pushed.
enum Child<'e> {
    Value(Unknown<'e>),
    /// The plain `classType` object of an instance or `Type` marker.
    ClassType(Object<'e>),
}

/// A container mid-encoding: its children are pushed one at a time on the
/// explicit stack, then the node is built from their ids.
struct Frame<'e> {
    pending: Pending,
    /// The memoized JS object, if the container has one.
    key: Option<Object<'e>>,
    /// Children still to push, in order.
    children: IntoIter<Child<'e>>,
    /// Ids of the children pushed so far.
    ids: Vec<NodeId>,
}

impl<'e> Frame<'e> {
    fn new(pending: Pending, key: Option<Object<'e>>, children: Vec<Child<'e>>) -> Self {
        let ids = Vec::with_capacity(children.len());
        Self {
            pending,
            key,
            children: children.into_iter(),
            ids,
        }
    }
}

/// What a frame builds once its children are pushed.
enum Pending {
    List,
    Tuple,
    Set,
    /// Children alternate key, value.
    Dict,
    /// Children alternate attr name, value.
    ClassType(ClassHeader),
    /// The first child is the class node, then attr name, value pairs.
    ClassInstance {
        instance_id: MontyUuid,
    },
}

/// The fields of a plain `classType` object other than its attrs.
struct ClassHeader {
    name: String,
    id: MontyUuid,
    host_defined: bool,
    is_dataclass: bool,
}

impl ClassHeader {
    /// Reads the plain `classType` object of a Type / ClassInstance marker.
    fn read(obj: &Object<'_>) -> Result<Self> {
        Ok(Self {
            name: obj.get_named_property("name")?,
            id: get_uuid_string_property(obj, "id", "ClassType")?,
            host_defined: obj.get_named_property("hostDefined")?,
            is_dataclass: obj.get_named_property("isDataclass")?,
        })
    }

    /// The class node with the given attr pairs.
    fn node(&self, attrs: Vec<(NodeId, NodeId)>) -> MontyNode {
        MontyNode::ClassType(Box::new(ClassTypeNode {
            name: self.name.clone(),
            id: self.id,
            host_defined: self.host_defined,
            is_dataclass: self.is_dataclass,
            attrs,
        }))
    }
}

/// Regroups the ids of children pushed key, value, key, value, … into pairs.
fn id_pairs(ids: &[NodeId]) -> Vec<(NodeId, NodeId)> {
    ids.as_chunks::<2>()
        .0
        .iter()
        .map(|&[key, value]| (key, value))
        .collect()
}

/// A memo entry that is not a node index: the map is private, so this is a bug.
fn invalid_memo() -> Error {
    Error::from_reason("encoder memo holds an invalid node index")
}

/// Checks if a JS object is an instance of Set.
fn is_js_set(obj: &Object, env: &Env) -> Result<bool> {
    let global = env.get_global()?;
    let set_constructor: Function<()> = global.get_named_property("Set")?;
    obj.instanceof(set_constructor)
}

/// Checks if a JS object is an instance of Map.
fn is_js_map(obj: &Object, env: &Env) -> Result<bool> {
    let global = env.get_global()?;
    let map_constructor: Function<()> = global.get_named_property("Map")?;
    obj.instanceof(map_constructor)
}

/// A JS Map's entries as pending children, key then value.
fn js_map_entries<'e>(map: &Object<'e>) -> Result<Vec<Child<'e>>> {
    let entries_method: Function<()> = map.get_named_property("entries")?;
    let iterator: Object = entries_method.apply(*map, ())?.coerce_to_object()?;
    let mut children = Vec::new();
    loop {
        let next_method: Function<()> = iterator.get_named_property("next")?;
        let result: Object = next_method.apply(iterator, ())?.coerce_to_object()?;
        if result.get_named_property::<bool>("done")? {
            break;
        }
        // value is [key, value] array
        let entry: Object = result.get_named_property::<Unknown>("value")?.coerce_to_object()?;
        children.push(Child::Value(entry.get_element(0)?));
        children.push(Child::Value(entry.get_element(1)?));
    }
    Ok(children)
}

/// A JS Set's values as pending children.
fn js_set_values<'e>(set: &Object<'e>) -> Result<Vec<Child<'e>>> {
    let values_method: Function<()> = set.get_named_property("values")?;
    let iterator: Object = values_method.apply(*set, ())?.coerce_to_object()?;
    let mut children = Vec::new();
    loop {
        let next_method: Function<()> = iterator.get_named_property("next")?;
        let result: Object = next_method.apply(iterator, ())?.coerce_to_object()?;
        if result.get_named_property::<bool>("done")? {
            break;
        }
        children.push(Child::Value(result.get_named_property("value")?));
    }
    Ok(children)
}

/// A JS Array's items as pending children.
fn js_array_items<'e>(arr: &Object<'e>) -> Result<Vec<Child<'e>>> {
    let length: u32 = arr.get_named_property("length")?;
    (0..length).map(|i| Ok(Child::Value(arr.get_element(i)?))).collect()
}

/// A plain JS object's own property names and values as pending children,
/// key then value. Since JS object keys are always strings, every key becomes
/// a string; for full key type preservation, use a JS `Map` instead.
fn js_object_entries<'e>(obj: &Object<'e>, env: &'e Env) -> Result<Vec<Child<'e>>> {
    let keys = obj.get_property_names()?;
    let length: u32 = keys.get_named_property("length")?;
    let mut children = Vec::with_capacity(2 * length as usize);
    for i in 0..length {
        let key: Unknown = keys.get_element(i)?;
        let key_str: String = key.coerce_to_string()?.into_utf8()?.into_owned()?;
        let value: Unknown = obj.get_named_property(&key_str)?;
        children.push(Child::Value(env.create_string(&key_str)?.into_unknown(env)?));
        children.push(Child::Value(value));
    }
    Ok(children)
}

/// The `[name, value]` pair array attrs cross as, as pending children (name
/// then value); a non-string name is rejected.
fn js_attr_pairs<'e>(attrs_arr: Array<'e>, type_name: &str) -> Result<Vec<Child<'e>>> {
    let mut children = Vec::with_capacity(2 * attrs_arr.len() as usize);
    for i in 0..attrs_arr.len() {
        let Some(pair) = attrs_arr.get::<Array>(i)? else {
            return Err(Error::from_reason(format!(
                "{type_name} attrs entries must be [name, value] pairs"
            )));
        };
        let key = pair
            .get::<Unknown>(0)?
            .filter(|key| key.get_type().is_ok_and(|t| t == ValueType::String))
            .ok_or_else(|| Error::from_reason(format!("{type_name} attr name must be a string")))?;
        let value = pair
            .get::<Unknown>(1)?
            .ok_or_else(|| Error::from_reason(format!("{type_name} attr value missing")))?;
        children.push(Child::Value(key));
        children.push(Child::Value(value));
    }
    Ok(children)
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
