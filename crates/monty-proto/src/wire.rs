//! Hand-written [`prost::Message`] implementation for [`MontyObject`].
//!
//! Values are the hot payload of the protocol — every external function call
//! ships its arguments, result, and final value across the process boundary.
//! Generated prost code would force a mirror struct tree (`pb::MontyObject`)
//! plus a deep conversion in each direction: a full clone of every string and
//! container on encode, and a second tree rebuild on decode. Instead, the
//! codegen maps the `monty.v1.MontyObject` schema message to [`WireObject`]
//! via `extern_path` (see `src/bin/generate.rs`), and this module implements
//! the wire format directly on top of [`MontyObject`]:
//!
//! - **encode** walks a borrowed [`MontyObject`] and writes bytes — no
//!   intermediate tree, no clones;
//! - **decode** builds the [`MontyObject`] straight from the wire, running
//!   the semantic validation (date ranges, timedelta normalization, enum
//!   names) *during* the parse, so untrusted bytes never exist in memory as
//!   an unvalidated value.
//!
//! Byte-for-byte compatibility with prost's generated encoding is enforced by
//! the differential tests in `tests/differential.rs`, which compare this
//! implementation against a fully-generated oracle compiled from the same
//! `.proto`. Known, deliberate divergence: on malformed input that repeats a
//! message-typed `kind` field, prost merges the duplicate payloads while this
//! implementation replaces the value (last one wins) — stricter, and only
//! observable on frames our encoders never produce.
//!
//! Encoding leaf arms whose wire form is a `Display` rendering (`MontyType`,
//! builtin functions, exception type names) allocates the rendered string in
//! both `encoded_len` and `encode_raw`; those arms are rare in real payloads
//! and the strings are tiny.

use std::{cell::Cell, fmt::Display, ops::RangeInclusive};

use monty::{
    DictPairs, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTimeDelta, MontyTimeZone, MontyType,
};
use num_bigint::{BigInt, Sign};
use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{self, DecodeContext, WireType, encode_key, encode_varint, encoded_len_varint, key_len, skip_field},
};

use crate::{convert::ProtoConvertError, frame::DEFAULT_MAX_DECODE_BYTES, pb};

/// The wire form of a [`MontyObject`]: what the `monty.v1.MontyObject` proto
/// message decodes into and encodes from.
///
/// `None` represents an absent `kind` oneof (an empty message on the wire) —
/// receivers reject it via [`Self::into_object`], exactly like prost's
/// `Option<Kind>`. Senders always build it from a real value via `From`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireObject(pub Option<MontyObject>);

impl WireObject {
    /// Wraps a value for sending. Equivalent to `From`, named for call sites
    /// where `.into()` would be unclear.
    #[must_use]
    pub fn new(obj: MontyObject) -> Self {
        Self(Some(obj))
    }

    /// Unwraps the decoded value, rejecting an absent `kind` oneof.
    pub fn into_object(self) -> Result<MontyObject, ProtoConvertError> {
        self.0.ok_or(ProtoConvertError::MissingField("MontyObject.kind"))
    }
}

impl From<MontyObject> for WireObject {
    fn from(obj: MontyObject) -> Self {
        Self(Some(obj))
    }
}

impl Message for WireObject {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        if let Some(obj) = &self.0 {
            encode_object(obj, buf);
        }
    }

    fn encoded_len(&self) -> usize {
        self.0.as_ref().map_or(0, object_len)
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        if let Some(obj) = decode_field(tag, wire_type, buf, ctx)? {
            self.0 = Some(obj);
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.0 = None;
    }
}

/// Wire form of `monty.v1.FunctionCall` that decodes arguments directly into
/// `MontyObject`s.
///
/// Generated prost code would first build `Vec<WireObject>` / `Vec<Pair>` and
/// the parent would then collect those into the public `TurnEvent` vectors.
/// This type is installed with `prost_build::extern_path`, so generated
/// `ChildEvent` decoding still handles the envelope while this payload avoids
/// the duplicate allocation for large argument lists.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireFunctionCall {
    /// Name of the external function the sandbox is calling.
    pub function_name: String,
    /// Positional arguments, decoded straight from repeated `MontyObject`.
    pub args: Vec<MontyObject>,
    /// Keyword arguments, preserving wire order.
    pub kwargs: Vec<(MontyObject, MontyObject)>,
    /// Child-assigned call id used by the matching resume request.
    pub call_id: u32,
    /// Whether the first argument is the method receiver.
    pub method_call: bool,
}

impl Message for WireFunctionCall {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_str(1, &self.function_name, buf);
        encode_repeated_object(2, &self.args, buf);
        encode_repeated_pair(3, &self.kwargs, buf);
        encode_uint32(4, self.call_id, buf);
        if self.method_call {
            encoding::bool::encode(5, &self.method_call, buf);
        }
    }

    fn encoded_len(&self) -> usize {
        str_len(1, &self.function_name)
            + repeated_object_len(2, &self.args)
            + repeated_pair_len(3, &self.kwargs)
            + uint32_len(4, self.call_id)
            + if self.method_call {
                encoding::bool::encoded_len(5, &self.method_call)
            } else {
                0
            }
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => encoding::string::merge(wire_type, &mut self.function_name, buf, ctx),
            2 => merge_object_item(wire_type, buf, ctx, &mut self.args),
            3 => merge_pair_item(wire_type, buf, ctx, &mut self.kwargs),
            4 => encoding::uint32::merge(wire_type, &mut self.call_id, buf, ctx),
            5 => encoding::bool::merge(wire_type, &mut self.method_call, buf, ctx),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn clear(&mut self) {
        self.function_name.clear();
        self.args.clear();
        self.kwargs.clear();
        self.call_id = 0;
        self.method_call = false;
    }
}

/// Field numbers of the `MontyObject.kind` oneof — must match
/// `proto/monty/v1/monty.proto` exactly (the differential oracle test catches drift).
mod tag {
    pub const ELLIPSIS: u32 = 1;
    pub const NONE: u32 = 2;
    pub const BOOLEAN: u32 = 3;
    pub const INT: u32 = 4;
    pub const BIGINT: u32 = 5;
    pub const FLOAT: u32 = 6;
    pub const STR: u32 = 7;
    pub const BYTES: u32 = 8;
    pub const LIST: u32 = 9;
    pub const TUPLE: u32 = 10;
    pub const NAMED_TUPLE: u32 = 11;
    pub const DICT: u32 = 12;
    pub const SET: u32 = 13;
    pub const FROZEN_SET: u32 = 14;
    pub const DATE: u32 = 15;
    pub const DATETIME: u32 = 16;
    pub const TIMEDELTA: u32 = 17;
    pub const TIMEZONE: u32 = 18;
    pub const EXCEPTION: u32 = 19;
    pub const TYPE: u32 = 20;
    pub const BUILTIN_FUNCTION: u32 = 21;
    pub const PATH: u32 = 22;
    pub const FILE_HANDLE: u32 = 23;
    pub const DATACLASS: u32 = 24;
    pub const FUNCTION: u32 = 25;
    pub const REPR: u32 = 26;
    pub const CYCLE: u32 = 27;
    pub const INSTANCE_TYPE: u32 = 28;
}

// ============================================================================
// Encoding
// ============================================================================

/// Writes `obj` as one `MontyObject.kind` oneof field. Oneof fields always
/// encode, even when the payload is a protobuf default (matching prost).
///
/// Each sub-message arm writes `encode_message_key(tag, <body len>, ...)` then
/// the body; the matching `*_len` and [`object_len`] arms must compute the same
/// length, or the frame corrupts (guarded by `tests/differential.rs`).
fn encode_object(obj: &MontyObject, buf: &mut impl BufMut) {
    match obj {
        MontyObject::Ellipsis => encoding::message::encode(tag::ELLIPSIS, &pb::Unit {}, buf),
        MontyObject::None => encoding::message::encode(tag::NONE, &pb::Unit {}, buf),
        MontyObject::Bool(b) => encoding::bool::encode(tag::BOOLEAN, b, buf),
        MontyObject::Int(i) => encoding::sint64::encode(tag::INT, i, buf),
        MontyObject::BigInt(bi) => encoding::message::encode(tag::BIGINT, &bigint_to_proto(bi), buf),
        MontyObject::Float(f) => encoding::double::encode(tag::FLOAT, f, buf),
        MontyObject::String(s) => encoding::string::encode(tag::STR, s, buf),
        MontyObject::Bytes(b) => encoding::bytes::encode(tag::BYTES, b, buf),
        MontyObject::List(items) => {
            encode_message_key(tag::LIST, value_list_len(items), buf);
            encode_repeated_object(1, items, buf);
        }
        MontyObject::Tuple(items) => {
            encode_message_key(tag::TUPLE, value_list_len(items), buf);
            encode_repeated_object(1, items, buf);
        }
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => {
            encode_message_key(tag::NAMED_TUPLE, named_tuple_len(type_name, field_names, values), buf);
            encode_str(1, type_name, buf);
            encode_repeated_str(2, field_names, buf);
            encode_repeated_object(3, values, buf);
        }
        MontyObject::Dict(pairs) => {
            encode_message_key(tag::DICT, dict_len(pairs), buf);
            encode_dict(pairs, buf);
        }
        MontyObject::Set(items) => {
            encode_message_key(tag::SET, value_list_len(items), buf);
            encode_repeated_object(1, items, buf);
        }
        MontyObject::FrozenSet(items) => {
            encode_message_key(tag::FROZEN_SET, value_list_len(items), buf);
            encode_repeated_object(1, items, buf);
        }
        MontyObject::Date(d) => encoding::message::encode(tag::DATE, &date_to_proto(d), buf),
        MontyObject::DateTime(dt) => {
            encode_message_key(tag::DATETIME, datetime_len(dt), buf);
            encode_datetime(dt, buf);
        }
        MontyObject::TimeDelta(td) => encoding::message::encode(tag::TIMEDELTA, &timedelta_to_proto(td), buf),
        MontyObject::TimeZone(tz) => {
            encode_message_key(tag::TIMEZONE, timezone_len(tz), buf);
            encode_int32(1, tz.offset_seconds, buf);
            encode_opt_str(2, tz.name.as_deref(), buf);
        }
        MontyObject::Exception { exc_type, arg } => {
            let name = exc_type.to_string();
            encode_message_key(tag::EXCEPTION, str_len(1, &name) + opt_str_len(2, arg.as_deref()), buf);
            encode_str(1, &name, buf);
            encode_opt_str(2, arg.as_deref(), buf);
        }
        MontyObject::Type(t) => match t {
            // Sandbox-class type objects carry the class name in a dedicated
            // field so a class named e.g. "int" cannot decode as the builtin.
            MontyType::Instance(name) => encoding::string::encode(tag::INSTANCE_TYPE, name, buf),
            other => encoding::string::encode(tag::TYPE, &other.to_string(), buf),
        },
        MontyObject::BuiltinFunction(bf) => encoding::string::encode(tag::BUILTIN_FUNCTION, &bf.to_string(), buf),
        MontyObject::Path(p) => encoding::string::encode(tag::PATH, p, buf),
        MontyObject::FileHandle(fh) => {
            encode_message_key(tag::FILE_HANDLE, file_handle_len(fh), buf);
            encode_str(1, &fh.path, buf);
            encode_str(2, fh.mode.as_str(), buf);
            encode_uint64(3, fh.position, buf);
        }
        MontyObject::Dataclass {
            name,
            type_id,
            field_names,
            attrs,
            frozen,
        } => {
            encode_message_key(
                tag::DATACLASS,
                dataclass_len(name, *type_id, field_names, attrs, *frozen),
                buf,
            );
            encode_str(1, name, buf);
            encode_uint64(2, *type_id, buf);
            encode_repeated_str(3, field_names, buf);
            // attrs is a non-optional message field that senders always
            // populate, so it encodes even when empty (message presence)
            encode_message_key(4, dict_len(attrs), buf);
            encode_dict(attrs, buf);
            if *frozen {
                encoding::bool::encode(5, frozen, buf);
            }
        }
        MontyObject::Function { name, docstring } => {
            encode_message_key(
                tag::FUNCTION,
                str_len(1, name) + opt_str_len(2, docstring.as_deref()),
                buf,
            );
            encode_str(1, name, buf);
            encode_opt_str(2, docstring.as_deref(), buf);
        }
        MontyObject::Repr(r) => encoding::string::encode(tag::REPR, r, buf),
        MontyObject::Cycle(identity, placeholder) => {
            let identity = *identity as u64;
            encode_message_key(tag::CYCLE, uint64_len(1, identity) + str_len(2, placeholder), buf);
            encode_uint64(1, identity, buf);
            encode_str(2, placeholder, buf);
        }
    }
}

/// Length of `obj` as one `MontyObject.kind` oneof field (key + payload).
/// Mirrors [`encode_object`] arm for arm.
fn object_len(obj: &MontyObject) -> usize {
    match obj {
        MontyObject::Ellipsis => encoding::message::encoded_len(tag::ELLIPSIS, &pb::Unit {}),
        MontyObject::None => encoding::message::encoded_len(tag::NONE, &pb::Unit {}),
        MontyObject::Bool(b) => encoding::bool::encoded_len(tag::BOOLEAN, b),
        MontyObject::Int(i) => encoding::sint64::encoded_len(tag::INT, i),
        MontyObject::BigInt(bi) => encoding::message::encoded_len(tag::BIGINT, &bigint_to_proto(bi)),
        MontyObject::Float(f) => encoding::double::encoded_len(tag::FLOAT, f),
        MontyObject::String(s) => encoding::string::encoded_len(tag::STR, s),
        MontyObject::Bytes(b) => encoding::bytes::encoded_len(tag::BYTES, b),
        MontyObject::List(items) => submessage_len(tag::LIST, value_list_len(items)),
        MontyObject::Tuple(items) => submessage_len(tag::TUPLE, value_list_len(items)),
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => submessage_len(tag::NAMED_TUPLE, named_tuple_len(type_name, field_names, values)),
        MontyObject::Dict(pairs) => submessage_len(tag::DICT, dict_len(pairs)),
        MontyObject::Set(items) => submessage_len(tag::SET, value_list_len(items)),
        MontyObject::FrozenSet(items) => submessage_len(tag::FROZEN_SET, value_list_len(items)),
        MontyObject::Date(d) => encoding::message::encoded_len(tag::DATE, &date_to_proto(d)),
        MontyObject::DateTime(dt) => submessage_len(tag::DATETIME, datetime_len(dt)),
        MontyObject::TimeDelta(td) => encoding::message::encoded_len(tag::TIMEDELTA, &timedelta_to_proto(td)),
        MontyObject::TimeZone(tz) => submessage_len(tag::TIMEZONE, timezone_len(tz)),
        MontyObject::Exception { exc_type, arg } => {
            let name = exc_type.to_string();
            submessage_len(tag::EXCEPTION, str_len(1, &name) + opt_str_len(2, arg.as_deref()))
        }
        MontyObject::Type(t) => match t {
            MontyType::Instance(name) => encoding::string::encoded_len(tag::INSTANCE_TYPE, name),
            other => encoding::string::encoded_len(tag::TYPE, &other.to_string()),
        },
        MontyObject::BuiltinFunction(bf) => encoding::string::encoded_len(tag::BUILTIN_FUNCTION, &bf.to_string()),
        MontyObject::Path(p) => encoding::string::encoded_len(tag::PATH, p),
        MontyObject::FileHandle(fh) => submessage_len(tag::FILE_HANDLE, file_handle_len(fh)),
        MontyObject::Dataclass {
            name,
            type_id,
            field_names,
            attrs,
            frozen,
        } => submessage_len(
            tag::DATACLASS,
            dataclass_len(name, *type_id, field_names, attrs, *frozen),
        ),
        MontyObject::Function { name, docstring } => {
            submessage_len(tag::FUNCTION, str_len(1, name) + opt_str_len(2, docstring.as_deref()))
        }
        MontyObject::Repr(r) => encoding::string::encoded_len(tag::REPR, r),
        MontyObject::Cycle(identity, placeholder) => {
            submessage_len(tag::CYCLE, uint64_len(1, *identity as u64) + str_len(2, placeholder))
        }
    }
}

/// Writes the key and length prefix of a length-delimited field.
fn encode_message_key(tag: u32, body_len: usize, buf: &mut impl BufMut) {
    encode_key(tag, WireType::LengthDelimited, buf);
    encode_varint(body_len as u64, buf);
}

/// Length of a length-delimited field: key + length varint + body.
fn submessage_len(tag: u32, body_len: usize) -> usize {
    key_len(tag) + encoded_len_varint(body_len as u64) + body_len
}

/// `ObjectList` body: `repeated MontyObject items = 1`.
fn value_list_len(items: &[MontyObject]) -> usize {
    repeated_object_len(1, items)
}

/// `repeated MontyObject` field: each element is one length-delimited entry.
fn encode_repeated_object(tag: u32, items: &[MontyObject], buf: &mut impl BufMut) {
    for obj in items {
        encode_message_key(tag, object_len(obj), buf);
        encode_object(obj, buf);
    }
}

fn repeated_object_len(tag: u32, items: &[MontyObject]) -> usize {
    items.iter().map(|obj| submessage_len(tag, object_len(obj))).sum()
}

/// `NamedTuple` body: `string type_name = 1; repeated string
/// field_names = 2; repeated MontyObject values = 3`.
fn named_tuple_len(type_name: &str, field_names: &[String], values: &[MontyObject]) -> usize {
    str_len(1, type_name) + repeated_str_len(2, field_names) + repeated_object_len(3, values)
}

/// `Dict` body: `repeated Pair pairs = 1` where `Pair` is
/// `MontyObject key = 1; MontyObject value = 2` (both always present).
fn dict_len(pairs: &DictPairs) -> usize {
    pairs
        .into_iter()
        .map(|(key, value)| submessage_len(1, pair_len(key, value)))
        .sum()
}

fn encode_dict(pairs: &DictPairs, buf: &mut impl BufMut) {
    encode_repeated_pair(1, pairs, buf);
}

/// `repeated Pair` field: each entry is a length-delimited key/value message.
fn encode_repeated_pair<'a>(
    tag: u32,
    pairs: impl IntoIterator<Item = &'a (MontyObject, MontyObject)>,
    buf: &mut impl BufMut,
) {
    for (key, value) in pairs {
        encode_message_key(tag, pair_len(key, value), buf);
        encode_message_key(1, object_len(key), buf);
        encode_object(key, buf);
        encode_message_key(2, object_len(value), buf);
        encode_object(value, buf);
    }
}

fn repeated_pair_len<'a>(tag: u32, pairs: impl IntoIterator<Item = &'a (MontyObject, MontyObject)>) -> usize {
    pairs
        .into_iter()
        .map(|(key, value)| submessage_len(tag, pair_len(key, value)))
        .sum()
}

fn pair_len(key: &MontyObject, value: &MontyObject) -> usize {
    submessage_len(1, object_len(key)) + submessage_len(2, object_len(value))
}

/// `DateTime` body: scalar fields 1–7 (implicit presence, skipped at
/// zero) plus explicit-presence `offset_seconds = 8` / `timezone_name = 9`.
fn datetime_len(dt: &MontyDateTime) -> usize {
    int32_len(1, dt.year)
        + uint32_len(2, u32::from(dt.month))
        + uint32_len(3, u32::from(dt.day))
        + uint32_len(4, u32::from(dt.hour))
        + uint32_len(5, u32::from(dt.minute))
        + uint32_len(6, u32::from(dt.second))
        + uint32_len(7, dt.microsecond)
        + dt.offset_seconds.map_or(0, |off| encoding::int32::encoded_len(8, &off))
        + opt_str_len(9, dt.timezone_name.as_deref())
}

fn encode_datetime(dt: &MontyDateTime, buf: &mut impl BufMut) {
    encode_int32(1, dt.year, buf);
    encode_uint32(2, u32::from(dt.month), buf);
    encode_uint32(3, u32::from(dt.day), buf);
    encode_uint32(4, u32::from(dt.hour), buf);
    encode_uint32(5, u32::from(dt.minute), buf);
    encode_uint32(6, u32::from(dt.second), buf);
    encode_uint32(7, dt.microsecond, buf);
    if let Some(off) = dt.offset_seconds {
        encoding::int32::encode(8, &off, buf);
    }
    encode_opt_str(9, dt.timezone_name.as_deref(), buf);
}

/// `TimeZone` body: `int32 offset_seconds = 1; optional string name = 2`.
fn timezone_len(tz: &MontyTimeZone) -> usize {
    int32_len(1, tz.offset_seconds) + opt_str_len(2, tz.name.as_deref())
}

/// `FileHandle` body: `string path = 1; string mode = 2;
/// uint64 position = 3`.
fn file_handle_len(fh: &MontyFileHandle) -> usize {
    str_len(1, &fh.path) + str_len(2, fh.mode.as_str()) + uint64_len(3, fh.position)
}

/// `Dataclass` body: `string name = 1; uint64 type_id = 2; repeated
/// string field_names = 3; Dict attrs = 4; bool frozen = 5`.
fn dataclass_len(name: &str, type_id: u64, field_names: &[String], attrs: &DictPairs, frozen: bool) -> usize {
    str_len(1, name)
        + uint64_len(2, type_id)
        + repeated_str_len(3, field_names)
        + submessage_len(4, dict_len(attrs))
        + if frozen {
            encoding::bool::encoded_len(5, &frozen)
        } else {
            0
        }
}

// --- proto3 field helpers, mirroring prost's generated default-skipping ---

/// Implicit-presence string field: skipped when empty.
fn encode_str(tag: u32, s: &str, buf: &mut impl BufMut) {
    if !s.is_empty() {
        encode_message_key(tag, s.len(), buf);
        buf.put_slice(s.as_bytes());
    }
}

fn str_len(tag: u32, s: &str) -> usize {
    if s.is_empty() { 0 } else { submessage_len(tag, s.len()) }
}

/// Explicit-presence (`optional`) string field: encoded whenever `Some`,
/// including `Some("")`.
fn encode_opt_str(tag: u32, s: Option<&str>, buf: &mut impl BufMut) {
    if let Some(s) = s {
        encode_message_key(tag, s.len(), buf);
        buf.put_slice(s.as_bytes());
    }
}

fn opt_str_len(tag: u32, s: Option<&str>) -> usize {
    s.map_or(0, |s| submessage_len(tag, s.len()))
}

/// Repeated string field: every element is encoded, including empty strings.
fn encode_repeated_str(tag: u32, items: &[String], buf: &mut impl BufMut) {
    for s in items {
        encode_message_key(tag, s.len(), buf);
        buf.put_slice(s.as_bytes());
    }
}

fn repeated_str_len(tag: u32, items: &[String]) -> usize {
    items.iter().map(|s| submessage_len(tag, s.len())).sum()
}

/// Implicit-presence `int32` field: skipped at zero.
fn encode_int32(tag: u32, value: i32, buf: &mut impl BufMut) {
    if value != 0 {
        encoding::int32::encode(tag, &value, buf);
    }
}

fn int32_len(tag: u32, value: i32) -> usize {
    if value == 0 {
        0
    } else {
        encoding::int32::encoded_len(tag, &value)
    }
}

/// Implicit-presence `uint32` field: skipped at zero.
fn encode_uint32(tag: u32, value: u32, buf: &mut impl BufMut) {
    if value != 0 {
        encoding::uint32::encode(tag, &value, buf);
    }
}

fn uint32_len(tag: u32, value: u32) -> usize {
    if value == 0 {
        0
    } else {
        encoding::uint32::encoded_len(tag, &value)
    }
}

/// Implicit-presence `uint64` field: skipped at zero.
fn encode_uint64(tag: u32, value: u64, buf: &mut impl BufMut) {
    if value != 0 {
        encoding::uint64::encode(tag, &value, buf);
    }
}

fn uint64_len(tag: u32, value: u64) -> usize {
    if value == 0 {
        0
    } else {
        encoding::uint64::encoded_len(tag, &value)
    }
}

// ============================================================================
// Decoding
// ============================================================================

/// Decodes one `MontyObject.kind` field, validating as it parses. `None`
/// means the tag was unknown and skipped (forward compatibility, matching
/// prost's generated decoder).
fn decode_field(
    tag: u32,
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
) -> Result<Option<MontyObject>, DecodeError> {
    let obj = match tag {
        tag::ELLIPSIS => {
            merge_message::<pb::Unit>(wire_type, buf, ctx)?;
            MontyObject::Ellipsis
        }
        tag::NONE => {
            merge_message::<pb::Unit>(wire_type, buf, ctx)?;
            MontyObject::None
        }
        tag::BOOLEAN => {
            let mut v = false;
            encoding::bool::merge(wire_type, &mut v, buf, ctx)?;
            MontyObject::Bool(v)
        }
        tag::INT => {
            let mut v = 0i64;
            encoding::sint64::merge(wire_type, &mut v, buf, ctx)?;
            MontyObject::Int(v)
        }
        tag::BIGINT => MontyObject::BigInt(bigint_from_proto(&merge_message(wire_type, buf, ctx)?)),
        tag::FLOAT => {
            let mut v = 0f64;
            encoding::double::merge(wire_type, &mut v, buf, ctx)?;
            MontyObject::Float(v)
        }
        tag::STR => MontyObject::String(merge_string(wire_type, buf, ctx)?),
        tag::BYTES => {
            let mut v = Vec::new();
            encoding::bytes::merge(wire_type, &mut v, buf, ctx)?;
            MontyObject::Bytes(v)
        }
        tag::LIST => MontyObject::List(merge_value_list(wire_type, buf, ctx)?),
        tag::TUPLE => MontyObject::Tuple(merge_value_list(wire_type, buf, ctx)?),
        tag::NAMED_TUPLE => {
            let nt: NamedTupleBody = merge_message(wire_type, buf, ctx)?;
            MontyObject::NamedTuple {
                type_name: nt.type_name,
                field_names: nt.field_names,
                values: nt.values,
            }
        }
        tag::DICT => MontyObject::Dict(merge_dict(wire_type, buf, ctx)?),
        tag::SET => MontyObject::Set(merge_value_list(wire_type, buf, ctx)?),
        tag::FROZEN_SET => MontyObject::FrozenSet(merge_value_list(wire_type, buf, ctx)?),
        tag::DATE => {
            let d: pb::Date = merge_message(wire_type, buf, ctx)?;
            MontyObject::Date(date_from_proto(&d).map_err(to_decode_err)?)
        }
        tag::DATETIME => {
            let dt: pb::DateTime = merge_message(wire_type, buf, ctx)?;
            MontyObject::DateTime(datetime_from_proto(dt).map_err(to_decode_err)?)
        }
        tag::TIMEDELTA => {
            let td: pb::TimeDelta = merge_message(wire_type, buf, ctx)?;
            MontyObject::TimeDelta(timedelta_from_proto(&td).map_err(to_decode_err)?)
        }
        tag::TIMEZONE => {
            let tz: pb::TimeZone = merge_message(wire_type, buf, ctx)?;
            MontyObject::TimeZone(MontyTimeZone {
                offset_seconds: tz.offset_seconds,
                name: tz.name,
            })
        }
        tag::EXCEPTION => {
            let exc: pb::Exception = merge_message(wire_type, buf, ctx)?;
            MontyObject::Exception {
                exc_type: exc
                    .exc_type
                    .parse()
                    .map_err(|_| to_decode_err(ProtoConvertError::UnknownExcType(exc.exc_type)))?,
                arg: exc.arg,
            }
        }
        tag::TYPE => {
            let name = merge_string(wire_type, buf, ctx)?;
            MontyType::from_type_name(&name)
                .map(MontyObject::Type)
                .ok_or_else(|| to_decode_err(ProtoConvertError::UnknownType(name)))?
        }
        tag::INSTANCE_TYPE => MontyObject::Type(MontyType::Instance(merge_string(wire_type, buf, ctx)?)),
        tag::BUILTIN_FUNCTION => {
            let name = merge_string(wire_type, buf, ctx)?;
            MontyObject::builtin_function_from_name(&name)
                .ok_or_else(|| to_decode_err(ProtoConvertError::UnknownBuiltinFunction(name)))?
        }
        tag::PATH => MontyObject::Path(merge_string(wire_type, buf, ctx)?),
        tag::FILE_HANDLE => {
            let fh: pb::FileHandle = merge_message(wire_type, buf, ctx)?;
            MontyObject::FileHandle(MontyFileHandle {
                mode: fh
                    .mode
                    .parse()
                    .map_err(|_| to_decode_err(ProtoConvertError::InvalidFileMode(fh.mode)))?,
                path: fh.path,
                position: fh.position,
            })
        }
        tag::DATACLASS => {
            let dc: DataclassBody = merge_message(wire_type, buf, ctx)?;
            let attrs = dc
                .attrs
                .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("Dataclass.attrs")))?;
            MontyObject::Dataclass {
                name: dc.name,
                type_id: dc.type_id,
                field_names: dc.field_names,
                attrs: DictPairs::from(attrs.0),
                frozen: dc.frozen,
            }
        }
        tag::FUNCTION => {
            let func: pb::Function = merge_message(wire_type, buf, ctx)?;
            MontyObject::Function {
                name: func.name,
                docstring: func.docstring,
            }
        }
        tag::REPR => MontyObject::Repr(merge_string(wire_type, buf, ctx)?),
        tag::CYCLE => {
            let c: pb::Cycle = merge_message(wire_type, buf, ctx)?;
            let identity = usize::try_from(c.identity).map_err(|_| {
                to_decode_err(ProtoConvertError::InvalidValue {
                    field: "Cycle.identity",
                    reason: format!("{} does not fit in usize", c.identity),
                })
            })?;
            MontyObject::Cycle(identity, c.placeholder)
        }
        _ => {
            skip_field(wire_type, tag, buf, ctx)?;
            return Ok(None);
        }
    };
    // Charge against the frame budget — every value flows through here, so this
    // is the bound that stops a cheap frame OOMing the host on decode.
    charge_decode(obj.host_size())?;
    Ok(Some(obj))
}

/// Decodes one length-delimited sub-message into a fresh `M` (the generated
/// leaf and container types). `message::merge` enforces prost's recursion
/// limit, which bounds value nesting exactly as before.
fn merge_message<M: Message + Default>(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
) -> Result<M, DecodeError> {
    let mut msg = M::default();
    encoding::message::merge(wire_type, &mut msg, buf, ctx)?;
    Ok(msg)
}

/// Decodes one string field.
fn merge_string(wire_type: WireType, buf: &mut impl Buf, ctx: DecodeContext) -> Result<String, DecodeError> {
    let mut s = String::new();
    encoding::string::merge(wire_type, &mut s, buf, ctx)?;
    Ok(s)
}

/// Decodes an `ObjectList` (list/tuple/set/frozenset payload) straight into
/// `Vec<MontyObject>` via [`ObjectList`], skipping the `Vec<WireObject>` wrapper
/// the generated `pb::ObjectList` would force and the extra unwrap pass over it.
fn merge_value_list(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
) -> Result<Vec<MontyObject>, DecodeError> {
    Ok(merge_message::<ObjectList>(wire_type, buf, ctx)?.0)
}

/// Decodes a `Dict` straight into [`DictPairs`] via [`PairList`], skipping
/// the `Vec<pb::Pair>` wrapper.
fn merge_dict(wire_type: WireType, buf: &mut impl Buf, ctx: DecodeContext) -> Result<DictPairs, DecodeError> {
    Ok(DictPairs::from(merge_message::<PairList>(wire_type, buf, ctx)?.0))
}

/// Decodes one repeated `MontyObject` entry into an already-owned vector.
fn merge_object_item(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
    items: &mut Vec<MontyObject>,
) -> Result<(), DecodeError> {
    let item: WireObject = merge_message(wire_type, buf, ctx)?;
    items.push(item.into_object().map_err(to_decode_err)?);
    Ok(())
}

/// Decodes one repeated `Pair` entry into an already-owned vector.
fn merge_pair_item(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
    pairs: &mut Vec<(MontyObject, MontyObject)>,
) -> Result<(), DecodeError> {
    let pair: pb::Pair = merge_message(wire_type, buf, ctx)?;
    pairs.push(pair_to_kv(pair)?);
    Ok(())
}

/// Unwraps one decoded `Pair` into a `(key, value)`, rejecting an absent key or
/// value. Used by [`PairList`].
fn pair_to_kv(pair: pb::Pair) -> Result<(MontyObject, MontyObject), DecodeError> {
    let key = pair
        .key
        .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("Pair.key")))?;
    let value = pair
        .value
        .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("Pair.value")))?;
    Ok((
        key.into_object().map_err(to_decode_err)?,
        value.into_object().map_err(to_decode_err)?,
    ))
}

/// Decode-only `prost::Message` materializing a `repeated MontyObject` field
/// straight into `Vec<MontyObject>`, skipping the `Vec<WireObject>` buffer (and
/// unwrap pass) `pb::ObjectList` would force; only a per-element `WireObject` is
/// transient. Never encoded (values encode via [`encode_repeated_object`]), so
/// the encode methods are unreachable.
#[derive(Default)]
struct ObjectList(Vec<MontyObject>);

impl Message for ObjectList {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        // `ObjectList.items` is field 1; any other tag is unknown → skip.
        if tag == 1 {
            merge_object_item(wire_type, buf, ctx, &mut self.0)
        } else {
            skip_field(wire_type, tag, buf, ctx)
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("ObjectList is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("ObjectList is decode-only")
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Decode-only `prost::Message` that materializes a `repeated Pair` field
/// directly into `(key, value)` tuples — the dict analogue of [`ObjectList`],
/// avoiding the `Vec<pb::Pair>` wrapper. Decode-only; encode is unreachable.
#[derive(Default)]
struct PairList(Vec<(MontyObject, MontyObject)>);

impl Message for PairList {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        // `Dict.pairs` is field 1; any other tag is unknown → skip.
        if tag == 1 {
            merge_pair_item(wire_type, buf, ctx, &mut self.0)
        } else {
            skip_field(wire_type, tag, buf, ctx)
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("PairList is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("PairList is decode-only")
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Decode-only `prost::Message` for `NamedTuple`, materializing the
/// `repeated MontyObject values` field straight into `Vec<MontyObject>` (the
/// [`ObjectList`] trick inlined alongside the other two fields) instead of the
/// `Vec<WireObject>` the generated `pb::NamedTuple` would build and then
/// unwrap. Decode-only; named tuples encode via [`encode_object`]'s arm.
#[derive(Default)]
struct NamedTupleBody {
    type_name: String,
    field_names: Vec<String>,
    values: Vec<MontyObject>,
}

impl Message for NamedTupleBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        // Field numbers from `NamedTuple` in monty.proto; unknown → skip.
        match tag {
            1 => encoding::string::merge(wire_type, &mut self.type_name, buf, ctx),
            2 => encoding::string::merge_repeated(wire_type, &mut self.field_names, buf, ctx),
            3 => merge_object_item(wire_type, buf, ctx, &mut self.values),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("NamedTupleBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("NamedTupleBody is decode-only")
    }

    fn clear(&mut self) {
        self.type_name.clear();
        self.field_names.clear();
        self.values.clear();
    }
}

/// Decode-only `prost::Message` for `Dataclass`, decoding the `attrs`
/// field (a `Dict`) straight into [`DictPairs`] via [`PairList`] rather
/// than the `Vec<pb::Pair>` wrapper the generated `pb::Dataclass` would
/// build and then unwrap. `attrs` stays `Option` so an absent message field is
/// rejected by [`decode_field`] (presence, not a default). Decode-only.
#[derive(Default)]
struct DataclassBody {
    name: String,
    type_id: u64,
    field_names: Vec<String>,
    attrs: Option<PairList>,
    frozen: bool,
}

impl Message for DataclassBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        // Field numbers from `Dataclass` in monty.proto; unknown → skip.
        match tag {
            1 => encoding::string::merge(wire_type, &mut self.name, buf, ctx),
            2 => encoding::uint64::merge(wire_type, &mut self.type_id, buf, ctx),
            3 => encoding::string::merge_repeated(wire_type, &mut self.field_names, buf, ctx),
            // `get_or_insert` mirrors prost's message-field merge: repeated
            // occurrences accumulate `pairs` into the same `PairList`.
            4 => encoding::message::merge(wire_type, self.attrs.get_or_insert_with(PairList::default), buf, ctx),
            5 => encoding::bool::merge(wire_type, &mut self.frozen, buf, ctx),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("DataclassBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("DataclassBody is decode-only")
    }

    fn clear(&mut self) {
        self.name.clear();
        self.type_id = 0;
        self.field_names.clear();
        self.attrs = None;
        self.frozen = false;
    }
}

/// Maps a semantic validation failure onto prost's decode error so it
/// surfaces through the normal frame-decode path.
//
// `DecodeError::new` is deprecated but has no public replacement in prost
// 0.14 (`DecodeErrorKind` is crate-private); the deprecation note itself
// acknowledges external users. Revisit when prost ships a public constructor.
#[expect(deprecated)]
fn to_decode_err(err: impl Display) -> DecodeError {
    DecodeError::new(err.to_string())
}

// ============================================================================
// Leaf conversions and validation (the wire is untrusted)
// ============================================================================

/// Encodes a `BigInt` as sign + big-endian magnitude.
fn bigint_to_proto(bi: &BigInt) -> pb::BigInt {
    let (sign, magnitude) = bi.to_bytes_be();
    pb::BigInt {
        negative: sign == Sign::Minus,
        magnitude,
    }
}

/// Decodes sign + big-endian magnitude back to a `BigInt`.
///
/// An all-zero/empty magnitude decodes to zero regardless of the sign flag —
/// `BigInt` normalizes the sign of zero, so no invalid state is possible.
fn bigint_from_proto(bi: &pb::BigInt) -> BigInt {
    let sign = if bi.negative { Sign::Minus } else { Sign::Plus };
    BigInt::from_bytes_be(sign, &bi.magnitude)
}

fn date_to_proto(d: &MontyDate) -> pb::Date {
    pb::Date {
        year: d.year,
        month: u32::from(d.month),
        day: u32::from(d.day),
    }
}

fn date_from_proto(d: &pb::Date) -> Result<MontyDate, ProtoConvertError> {
    let (year, month, day) = date_fields(d.year, d.month, d.day, ["Date.year", "Date.month", "Date.day"])?;
    Ok(MontyDate { year, month, day })
}

fn datetime_from_proto(dt: pb::DateTime) -> Result<MontyDateTime, ProtoConvertError> {
    if dt.offset_seconds.is_none() && dt.timezone_name.is_some() {
        return Err(ProtoConvertError::InvalidValue {
            field: "DateTime.timezone_name",
            reason: "timezone_name requires offset_seconds".to_owned(),
        });
    }
    let (year, month, day) = date_fields(
        dt.year,
        dt.month,
        dt.day,
        ["DateTime.year", "DateTime.month", "DateTime.day"],
    )?;
    Ok(MontyDateTime {
        year,
        month,
        day,
        hour: ranged_u8(dt.hour, 0..=23, "DateTime.hour")?,
        minute: ranged_u8(dt.minute, 0..=59, "DateTime.minute")?,
        second: ranged_u8(dt.second, 0..=59, "DateTime.second")?,
        microsecond: bounded(dt.microsecond, 999_999, "DateTime.microsecond")?,
        offset_seconds: dt.offset_seconds,
        timezone_name: dt.timezone_name,
    })
}

fn timedelta_to_proto(td: &MontyTimeDelta) -> pb::TimeDelta {
    pb::TimeDelta {
        days: td.days,
        seconds: td.seconds,
        microseconds: td.microseconds,
    }
}

fn timedelta_from_proto(td: &pb::TimeDelta) -> Result<MontyTimeDelta, ProtoConvertError> {
    Ok(MontyTimeDelta {
        days: td.days,
        // out-of-range components would violate `MontyTimeDelta`'s
        // documented normalization invariants and corrupt arithmetic
        // and formatting once inside the sandbox
        seconds: normalized(td.seconds, 86_400, "TimeDelta.seconds")?,
        microseconds: normalized(td.microseconds, 1_000_000, "TimeDelta.microseconds")?,
    })
}

/// Validates wire year/month/day fields against the invariants documented on
/// `MontyDate`/`MontyDateTime` (year 1..=9999, month 1..=12, day valid for the
/// month/year). The wire is untrusted, and an out-of-range date would corrupt
/// comparison, arithmetic, and formatting once inside the sandbox.
/// `fields` names the year/month/day wire fields for error messages.
fn date_fields(year: i32, month: u32, day: u32, fields: [&'static str; 3]) -> Result<(i32, u8, u8), ProtoConvertError> {
    let [year_field, month_field, day_field] = fields;
    if !(1..=9999).contains(&year) {
        return Err(ProtoConvertError::InvalidValue {
            field: year_field,
            reason: format!("{year} is outside the range 1..=9999"),
        });
    }
    let month = ranged_u8(month, 1..=12, month_field)?;
    let day = ranged_u8(day, 1..=u32::from(days_in_month(year, month)), day_field)?;
    Ok((year, month, day))
}

/// Days in a Gregorian month; `month` must already be validated to 1..=12.
fn days_in_month(year: i32, month: u8) -> u8 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Checks a wire `u32` against an inclusive range and narrows it to `u8`.
fn ranged_u8(value: u32, range: RangeInclusive<u32>, field: &'static str) -> Result<u8, ProtoConvertError> {
    if range.contains(&value) {
        Ok(u8::try_from(value).expect("range bounds fit in u8"))
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} is outside the range {}..={}", range.start(), range.end()),
        })
    }
}

/// Checks a wire `i32` against the half-open normalized range `0..max`.
fn normalized(value: i32, max: i32, field: &'static str) -> Result<i32, ProtoConvertError> {
    if (0..max).contains(&value) {
        Ok(value)
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} is outside the normalized range 0..{max}"),
        })
    }
}

/// Checks a wire `u32` against an inclusive upper bound.
fn bounded(value: u32, max: u32, field: &'static str) -> Result<u32, ProtoConvertError> {
    if value <= max {
        Ok(value)
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} exceeds maximum {max}"),
        })
    }
}

// ============================================================================
// Decode memory budget
// ============================================================================

thread_local! {
    /// Host-memory budget (bytes) left for the value(s) decoding in the current
    /// frame on this thread.
    ///
    /// Thread-local because the budget must be *ambient*: a frame is decoded by
    /// prost's generated `Message::decode`, which calls our
    /// [`WireObject::merge_field`] — and that fixed signature has no slot to
    /// thread a budget through. Per *thread* rather than a global atomic because
    /// concurrent workers decode on separate threads. The limit is a hard
    /// constant ([`DEFAULT_MAX_DECODE_BYTES`]): the resting value, and what
    /// [`reset_decode_budget`] restores per frame.
    static DECODE_BUDGET: Cell<usize> = const { Cell::new(DEFAULT_MAX_DECODE_BYTES) };
}

/// Resets this thread's decode budget to the full [`DEFAULT_MAX_DECODE_BYTES`].
///
/// [`crate::FrameReader::read`] calls this before decoding each frame, which is
/// what makes the budget *per frame* rather than cumulative — a (possibly
/// compromised) child can't drain it across many frames, and a single ≤256 MiB
/// frame still can't amplify cheap elements into GiB of host `MontyObject`s.
///
/// Callers that decode a message *without* going through [`crate::FrameReader`]
/// (e.g. a transport that does its own framing, like a WebSocket) MUST call
/// this before each `Message::decode`, or the budget drains cumulatively across
/// decodes on the same thread and eventually rejects legitimate messages.
pub fn reset_decode_budget() {
    DECODE_BUDGET.set(DEFAULT_MAX_DECODE_BYTES);
}

/// Charges `bytes` of decoded host memory against the current frame's budget,
/// erroring once a frame would exceed it. Called once per [`MontyObject`] from
/// [`decode_field`] — the choke point every value routes through — so it bounds
/// total host memory incrementally, rejecting an over-budget frame before its
/// value tree is fully built.
fn charge_decode(bytes: usize) -> Result<(), DecodeError> {
    DECODE_BUDGET.with(|budget| match budget.get().checked_sub(bytes) {
        Some(remaining) => {
            budget.set(remaining);
            Ok(())
        }
        None => Err(to_decode_err("frame exceeds decode memory budget")),
    })
}
