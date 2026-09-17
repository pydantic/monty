//! Hand-written [`prost::Message`] implementations for the value arena.
//!
//! Values are the hot payload of the protocol — every external function call
//! ships its arguments, result, and final value across the process boundary.
//! Generated prost code would force a mirror arena (`pb::Arena` of
//! `pb::MontyNode`s) plus a conversion in each direction: a clone of every
//! string and container on encode, and a second arena on decode. Instead, the
//! codegen maps the `monty.v1.Arena` schema message to [`WireArena`] via
//! `extern_path` (see `src/bin/generate.rs`), and this module encodes and
//! decodes [`MontyNode`]s directly:
//!
//! - **encode** walks the borrowed nodes and writes bytes — no intermediate
//!   arena, no clones;
//! - **decode** builds the `Vec<MontyNode>` straight from the wire, running
//!   the semantic validation (date ranges, timedelta normalization, enum
//!   names) *during* the parse, so untrusted bytes never exist in memory as
//!   an unvalidated value, then checks the arena's index invariants once.
//!
//! The arena is flat, so decoding never recurses and nesting depth is not
//! bounded by prost's recursion limit. Every vector is charged against the
//! decode budget before it grows (the arena's slots, a container's child ids,
//! a call's argument ids) and each leaf's payload once parsed, so the only
//! uncharged transient is one leaf of at most the frame's size.
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

use std::{cell::Cell, fmt::Display, mem::size_of, ops::RangeInclusive};

use monty_types::{
    BuiltinsFunctions, CallArgs, ClassTypeNode, GraphError, MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS,
    MontyDate, MontyDateTime, MontyFileHandle, MontyGraph, MontyNode, MontyTime, MontyTimeDelta, MontyTimeZone,
    MontyType, MontyUuid, NodeId,
};
use num_bigint::{BigInt, Sign};
use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{self, DecodeContext, WireType, encode_key, encode_varint, encoded_len_varint, key_len, skip_field},
};

use crate::{convert::ProtoConvertError, frame::DEFAULT_MAX_DECODE_BYTES, pb};

/// The wire form of a [`MontyGraph`]: what the `monty.v1.Arena` proto message
/// decodes into and encodes from.
///
/// Decoding only collects nodes; [`Self::into_graph`] checks the arena
/// invariants (every child index lower than its holder, class instances
/// pointing at class nodes) once the whole message has arrived, since prost
/// has no end-of-message hook. Senders build it from a validated graph via `From`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireArena(pub Vec<MontyNode>);

impl WireArena {
    /// Wraps a graph for sending. Equivalent to `From`, named for call sites
    /// where `.into()` would be unclear.
    #[must_use]
    pub fn new(graph: MontyGraph) -> Self {
        Self(graph.into_nodes())
    }

    /// Validates the decoded nodes into a graph.
    pub fn into_graph(self) -> Result<MontyGraph, ProtoConvertError> {
        MontyGraph::from_nodes(self.0).map_err(|err| graph_error(&err))
    }
}

impl From<MontyGraph> for WireArena {
    fn from(graph: MontyGraph) -> Self {
        Self::new(graph)
    }
}

/// Smallest reservation the decoder makes when a vector starts growing.
const MIN_VEC_CAPACITY: usize = 4;

/// Fewest wire bytes one node occupies (entry key, length, kind key, empty
/// `Unit` payload); caps how many nodes a `node_count` hint may reserve.
const MIN_NODE_WIRE_BYTES: usize = 4;

impl Message for WireArena {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_uint32(1, arena_len_u32(self.0.len()), buf);
        for node in &self.0 {
            encoding::message::encode(2, &NodeRef(node), buf);
        }
    }

    fn encoded_len(&self) -> usize {
        uint32_len(1, arena_len_u32(self.0.len()))
            + self
                .0
                .iter()
                .map(|node| encoding::message::encoded_len(2, &NodeRef(node)))
                .sum::<usize>()
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => {
                let mut hint = 0u32;
                encoding::uint32::merge(wire_type, &mut hint, buf, ctx)?;
                // capped by the bytes left in the message and charged before
                // reserving, so a false hint cannot exceed this frame's budget
                if self.0.capacity() == 0 {
                    let reserve = (hint as usize).min(buf.remaining() / MIN_NODE_WIRE_BYTES);
                    reserve_charged(&mut self.0, reserve)?;
                }
                Ok(())
            }
            2 => {
                // the node's payload was charged as it decoded; this charges its slot
                let node = merge_message::<NodeBody>(wire_type, buf, ctx)?
                    .0
                    .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("MontyNode.kind")))?;
                push_charged(&mut self.0, node)
            }
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    /// Releases the nodes rather than keeping their capacity, so a reused
    /// arena never holds slots the next frame's budget did not charge.
    fn clear(&mut self) {
        self.0 = Vec::new();
    }
}

/// Wire form of `monty.v1.FunctionCall`: the call's argument arena plus the
/// ids of its positional and keyword arguments.
///
/// Installed with `prost_build::extern_path`, so generated `ChildEvent`
/// decoding still handles the envelope while this payload keeps the argument
/// vectors as bare ids and decodes the arena straight into `MontyNode`s.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireFunctionCall {
    /// Name of the external function the sandbox is calling.
    pub function_name: String,
    /// The arena `args` and `kwargs` index.
    pub values: WireArena,
    /// Positional arguments, in order.
    pub args: Vec<NodeId>,
    /// Keyword arguments as `(key, value)` ids, preserving wire order.
    pub kwargs: Vec<(NodeId, NodeId)>,
    /// Child-assigned call id used by the matching resume request.
    pub call_id: u32,
    /// Uuid of the routed receiver (a host-backed instance, or a class type
    /// for `__call__`/classmethod calls); `None` for plain external function
    /// calls. The receiver is never included in `args`.
    pub object_id: Option<MontyUuid>,
    /// The worker accepts an eagerly settled coroutine via `ResumeFutures`.
    pub allow_eager_await: bool,
}

impl WireFunctionCall {
    /// Splits `args` into its arena and the id vectors the wire carries.
    #[must_use]
    pub fn new(
        function_name: String,
        args: CallArgs,
        call_id: u32,
        object_id: Option<MontyUuid>,
        allow_eager_await: bool,
    ) -> Self {
        Self {
            function_name,
            values: WireArena::new(args.graph),
            args: args.arg_ids,
            kwargs: args.kwarg_ids,
            call_id,
            object_id,
            allow_eager_await,
        }
    }

    /// Validates the decoded arena and argument ids into [`CallArgs`].
    pub fn into_call_args(self) -> Result<CallArgs, ProtoConvertError> {
        let call = CallArgs {
            graph: self.values.into_graph()?,
            arg_ids: self.args,
            kwarg_ids: self.kwargs,
        };
        call.check_roots().map_err(|err| graph_error(&err))?;
        Ok(call)
    }
}

impl Message for WireFunctionCall {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_str(1, &self.function_name, buf);
        encode_packed_ids(2, &self.args, buf);
        encode_node_pairs(3, &self.kwargs, buf);
        encode_uint32(4, self.call_id, buf);
        if let Some(id) = &self.object_id {
            encoding::message::encode(5, &uuid_to_pb(id), buf);
        }
        if self.allow_eager_await {
            encoding::bool::encode(6, &true, buf);
        }
        encoding::message::encode(7, &self.values, buf);
    }

    fn encoded_len(&self) -> usize {
        str_len(1, &self.function_name)
            + packed_ids_len(2, &self.args)
            + node_pairs_len(3, &self.kwargs)
            + uint32_len(4, self.call_id)
            + self
                .object_id
                .as_ref()
                .map_or(0, |id| encoding::message::encoded_len(5, &uuid_to_pb(id)))
            + if self.allow_eager_await {
                encoding::bool::encoded_len(6, &true)
            } else {
                0
            }
            + encoding::message::encoded_len(7, &self.values)
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
            2 => merge_ids(wire_type, buf, ctx, &mut self.args),
            3 => {
                let pair: pb::NodePair = merge_message(wire_type, buf, ctx)?;
                push_charged(&mut self.kwargs, (NodeId(pair.key), NodeId(pair.value)))
            }
            4 => encoding::uint32::merge(wire_type, &mut self.call_id, buf, ctx),
            5 => {
                let mut uuid = pb::Uuid::default();
                encoding::message::merge(wire_type, &mut uuid, buf, ctx)?;
                self.object_id = Some(pb_uuid_to_monty(&uuid, "FunctionCall.object_id")?);
                Ok(())
            }
            6 => encoding::bool::merge(wire_type, &mut self.allow_eager_await, buf, ctx),
            7 => encoding::message::merge(wire_type, &mut self.values, buf, ctx),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn clear(&mut self) {
        self.function_name.clear();
        self.values.clear();
        self.args = Vec::new();
        self.kwargs = Vec::new();
        self.call_id = 0;
        self.object_id = None;
        self.allow_eager_await = false;
    }
}

/// Maps an arena invariant failure onto the conversion error hosts see.
pub(crate) fn graph_error(err: &GraphError) -> ProtoConvertError {
    ProtoConvertError::InvalidValue {
        field: "Arena",
        reason: err.to_string(),
    }
}

/// Field numbers of the `MontyNode.kind` oneof — must match
/// `proto/monty/v1/monty.proto` exactly (the differential oracle test catches drift).
mod tag {
    pub const ELLIPSIS: u32 = 1;
    pub const NONE: u32 = 2;
    pub const NOT_IMPLEMENTED: u32 = 3;
    pub const BOOLEAN: u32 = 4;
    pub const INT: u32 = 5;
    pub const BIGINT: u32 = 6;
    pub const FLOAT: u32 = 7;
    pub const STR: u32 = 8;
    pub const BYTES: u32 = 9;
    // 10 is `Uuid uuid`, declared in the schema but not yet implemented
    // (monty has no uuid module) — it decodes like any unknown kind.
    pub const LIST: u32 = 11;
    pub const TUPLE: u32 = 12;
    pub const NAMED_TUPLE: u32 = 13;
    pub const DICT: u32 = 14;
    pub const SET: u32 = 15;
    pub const FROZEN_SET: u32 = 16;
    pub const DATE: u32 = 17;
    pub const TIME: u32 = 18;
    pub const DATETIME: u32 = 19;
    pub const TIMEDELTA: u32 = 20;
    pub const TIMEZONE: u32 = 21;
    pub const EXCEPTION: u32 = 22;
    pub const TYPE: u32 = 23;
    pub const CLASS_INSTANCE: u32 = 24;
    pub const FUNCTION: u32 = 25;
    pub const BUILTIN_FUNCTION: u32 = 26;
    pub const PATH: u32 = 27;
    pub const FILE_HANDLE: u32 = 28;
    pub const REPR: u32 = 29;
    pub const CYCLE: u32 = 30;
}

// ============================================================================
// Encoding
// ============================================================================

/// A borrowed node as one `MontyNode` message, so the arena can encode each
/// entry through prost's length-delimited helpers without cloning.
#[derive(Debug)]
struct NodeRef<'a>(&'a MontyNode);

impl Message for NodeRef<'_> {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_node(self.0, buf);
    }

    fn encoded_len(&self) -> usize {
        node_len(self.0)
    }

    fn merge_field(
        &mut self,
        _tag: u32,
        _wire_type: WireType,
        _buf: &mut impl Buf,
        _ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        unreachable!("NodeRef is encode-only")
    }

    fn clear(&mut self) {
        unreachable!("NodeRef is encode-only")
    }
}

/// The `uint32 node_count` hint; an arena cannot outgrow `u32` ids.
fn arena_len_u32(len: usize) -> u32 {
    u32::try_from(len).expect("arena exceeds u32::MAX nodes")
}

/// Writes `node` as one `MontyNode.kind` oneof field. Oneof fields always
/// encode, even when the payload is a protobuf default (matching prost).
///
/// Each sub-message arm writes `encode_message_key(tag, <body len>, ...)` then
/// the body; the matching `*_len` and [`node_len`] arms must compute the same
/// length, or the frame corrupts (guarded by `tests/differential.rs`).
fn encode_node(node: &MontyNode, buf: &mut impl BufMut) {
    match node {
        MontyNode::Ellipsis => encoding::message::encode(tag::ELLIPSIS, &pb::Unit {}, buf),
        MontyNode::NotImplemented => encoding::message::encode(tag::NOT_IMPLEMENTED, &pb::Unit {}, buf),
        MontyNode::None => encoding::message::encode(tag::NONE, &pb::Unit {}, buf),
        MontyNode::Bool(b) => encoding::bool::encode(tag::BOOLEAN, b, buf),
        MontyNode::Int(i) => encoding::sint64::encode(tag::INT, i, buf),
        MontyNode::BigInt(bi) => encoding::message::encode(tag::BIGINT, &bigint_to_proto(bi), buf),
        MontyNode::Float(f) => encoding::double::encode(tag::FLOAT, f, buf),
        MontyNode::String(s) => encoding::string::encode(tag::STR, s, buf),
        MontyNode::Bytes(b) => encoding::bytes::encode(tag::BYTES, b, buf),
        MontyNode::List(ids) => encode_indexes(tag::LIST, ids, buf),
        MontyNode::Tuple(ids) => encode_indexes(tag::TUPLE, ids, buf),
        MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        } => {
            encode_message_key(tag::NAMED_TUPLE, named_tuple_len(type_name, field_names, values), buf);
            encode_str(1, type_name, buf);
            encode_repeated_str(2, field_names, buf);
            encode_packed_ids(3, values, buf);
        }
        MontyNode::Dict(pairs) => {
            encode_message_key(tag::DICT, node_pairs_len(1, pairs), buf);
            encode_node_pairs(1, pairs, buf);
        }
        MontyNode::Set(ids) => encode_indexes(tag::SET, ids, buf),
        MontyNode::FrozenSet(ids) => encode_indexes(tag::FROZEN_SET, ids, buf),
        MontyNode::Date(d) => encoding::message::encode(tag::DATE, &date_to_proto(d), buf),
        MontyNode::DateTime(dt) => {
            encode_message_key(tag::DATETIME, datetime_len(dt), buf);
            encode_datetime(dt, buf);
        }
        MontyNode::Time(t) => {
            encode_message_key(tag::TIME, time_len(t), buf);
            encode_time(t, buf);
        }
        MontyNode::TimeDelta(td) => encoding::message::encode(tag::TIMEDELTA, &timedelta_to_proto(td), buf),
        MontyNode::TimeZone(tz) => {
            encode_message_key(tag::TIMEZONE, timezone_len(tz), buf);
            encode_int32(1, tz.offset_seconds, buf);
            encode_opt_str(2, tz.name.as_deref(), buf);
        }
        MontyNode::Exception { exc_type, arg } => {
            let name = exc_type.to_string();
            encode_message_key(tag::EXCEPTION, str_len(1, &name) + opt_str_len(2, arg.as_deref()), buf);
            encode_str(1, &name, buf);
            encode_opt_str(2, arg.as_deref(), buf);
        }
        MontyNode::Type(t) => encoding::message::encode(tag::TYPE, &builtin_type_to_pb(t), buf),
        MontyNode::ClassType(class) => {
            encode_message_key(tag::TYPE, class_type_len(class), buf);
            encode_class_type(class, buf);
        }
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => {
            let id = uuid_to_pb(instance_id);
            encode_message_key(tag::CLASS_INSTANCE, class_instance_len(*class_type, &id, attrs), buf);
            encode_uint32(1, class_type.0, buf);
            // instance_id and attrs are message fields, so they encode even
            // when empty (message presence, matching prost)
            encoding::message::encode(2, &id, buf);
            encode_message_key(3, node_pairs_len(1, attrs), buf);
            encode_node_pairs(1, attrs, buf);
        }
        MontyNode::BuiltinFunction(bf) => encoding::string::encode(tag::BUILTIN_FUNCTION, &bf.to_string(), buf),
        MontyNode::Path(p) => encoding::string::encode(tag::PATH, p, buf),
        MontyNode::FileHandle(fh) => {
            encode_message_key(tag::FILE_HANDLE, file_handle_len(fh), buf);
            encode_str(1, &fh.path, buf);
            encode_str(2, fh.mode.as_str(), buf);
            encode_uint64(3, fh.position, buf);
        }
        MontyNode::Function { name, docstring } => {
            encode_message_key(
                tag::FUNCTION,
                str_len(1, name) + opt_str_len(2, docstring.as_deref()),
                buf,
            );
            encode_str(1, name, buf);
            encode_opt_str(2, docstring.as_deref(), buf);
        }
        MontyNode::Repr(r) => encoding::string::encode(tag::REPR, r, buf),
        MontyNode::Cycle(placeholder) => encoding::string::encode(tag::CYCLE, placeholder, buf),
    }
}

/// Length of `node` as one `MontyNode.kind` oneof field (key + payload).
/// Mirrors [`encode_node`] arm for arm.
fn node_len(node: &MontyNode) -> usize {
    match node {
        MontyNode::Ellipsis => encoding::message::encoded_len(tag::ELLIPSIS, &pb::Unit {}),
        MontyNode::NotImplemented => encoding::message::encoded_len(tag::NOT_IMPLEMENTED, &pb::Unit {}),
        MontyNode::None => encoding::message::encoded_len(tag::NONE, &pb::Unit {}),
        MontyNode::Bool(b) => encoding::bool::encoded_len(tag::BOOLEAN, b),
        MontyNode::Int(i) => encoding::sint64::encoded_len(tag::INT, i),
        MontyNode::BigInt(bi) => encoding::message::encoded_len(tag::BIGINT, &bigint_to_proto(bi)),
        MontyNode::Float(f) => encoding::double::encoded_len(tag::FLOAT, f),
        MontyNode::String(s) => encoding::string::encoded_len(tag::STR, s),
        MontyNode::Bytes(b) => encoding::bytes::encoded_len(tag::BYTES, b),
        MontyNode::List(ids) => submessage_len(tag::LIST, packed_ids_len(1, ids)),
        MontyNode::Tuple(ids) => submessage_len(tag::TUPLE, packed_ids_len(1, ids)),
        MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        } => submessage_len(tag::NAMED_TUPLE, named_tuple_len(type_name, field_names, values)),
        MontyNode::Dict(pairs) => submessage_len(tag::DICT, node_pairs_len(1, pairs)),
        MontyNode::Set(ids) => submessage_len(tag::SET, packed_ids_len(1, ids)),
        MontyNode::FrozenSet(ids) => submessage_len(tag::FROZEN_SET, packed_ids_len(1, ids)),
        MontyNode::Date(d) => encoding::message::encoded_len(tag::DATE, &date_to_proto(d)),
        MontyNode::DateTime(dt) => submessage_len(tag::DATETIME, datetime_len(dt)),
        MontyNode::Time(t) => submessage_len(tag::TIME, time_len(t)),
        MontyNode::TimeDelta(td) => encoding::message::encoded_len(tag::TIMEDELTA, &timedelta_to_proto(td)),
        MontyNode::TimeZone(tz) => submessage_len(tag::TIMEZONE, timezone_len(tz)),
        MontyNode::Exception { exc_type, arg } => {
            let name = exc_type.to_string();
            submessage_len(tag::EXCEPTION, str_len(1, &name) + opt_str_len(2, arg.as_deref()))
        }
        MontyNode::Type(t) => encoding::message::encoded_len(tag::TYPE, &builtin_type_to_pb(t)),
        MontyNode::ClassType(class) => submessage_len(tag::TYPE, class_type_len(class)),
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => submessage_len(
            tag::CLASS_INSTANCE,
            class_instance_len(*class_type, &uuid_to_pb(instance_id), attrs),
        ),
        MontyNode::BuiltinFunction(bf) => encoding::string::encoded_len(tag::BUILTIN_FUNCTION, &bf.to_string()),
        MontyNode::Path(p) => encoding::string::encoded_len(tag::PATH, p),
        MontyNode::FileHandle(fh) => submessage_len(tag::FILE_HANDLE, file_handle_len(fh)),
        MontyNode::Function { name, docstring } => {
            submessage_len(tag::FUNCTION, str_len(1, name) + opt_str_len(2, docstring.as_deref()))
        }
        MontyNode::Repr(r) => encoding::string::encoded_len(tag::REPR, r),
        MontyNode::Cycle(placeholder) => encoding::string::encoded_len(tag::CYCLE, placeholder),
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

/// An `Indexes` message (`repeated uint32 items = 1`) as one oneof arm.
fn encode_indexes(tag: u32, ids: &[NodeId], buf: &mut impl BufMut) {
    encode_message_key(tag, packed_ids_len(1, ids), buf);
    encode_packed_ids(1, ids, buf);
}

/// Packed `repeated uint32` field of node ids, as prost encodes it: one
/// length-delimited entry holding every varint, skipped entirely when empty.
fn encode_packed_ids(tag: u32, ids: &[NodeId], buf: &mut impl BufMut) {
    if !ids.is_empty() {
        encode_message_key(tag, packed_body_len(ids), buf);
        for id in ids {
            encode_varint(u64::from(id.0), buf);
        }
    }
}

fn packed_ids_len(tag: u32, ids: &[NodeId]) -> usize {
    if ids.is_empty() {
        0
    } else {
        submessage_len(tag, packed_body_len(ids))
    }
}

/// Bytes of the varints inside a packed id field.
fn packed_body_len(ids: &[NodeId]) -> usize {
    ids.iter().map(|id| encoded_len_varint(u64::from(id.0))).sum()
}

/// `repeated NodePair` field: each entry is a length-delimited
/// `uint32 key = 1; uint32 value = 2` message with implicit presence.
fn encode_node_pairs(tag: u32, pairs: &[(NodeId, NodeId)], buf: &mut impl BufMut) {
    for (key, value) in pairs {
        encode_message_key(tag, node_pair_len(*key, *value), buf);
        encode_uint32(1, key.0, buf);
        encode_uint32(2, value.0, buf);
    }
}

fn node_pairs_len(tag: u32, pairs: &[(NodeId, NodeId)]) -> usize {
    pairs
        .iter()
        .map(|(key, value)| submessage_len(tag, node_pair_len(*key, *value)))
        .sum()
}

fn node_pair_len(key: NodeId, value: NodeId) -> usize {
    uint32_len(1, key.0) + uint32_len(2, value.0)
}

/// `NamedTupleNode` body: `string type_name = 1; repeated string
/// field_names = 2; repeated uint32 values = 3`.
fn named_tuple_len(type_name: &str, field_names: &[String], values: &[NodeId]) -> usize {
    str_len(1, type_name) + repeated_str_len(2, field_names) + packed_ids_len(3, values)
}

/// `Type` body for a class node: `string name = 1; Uuid id = 2; TypeOrigin
/// origin = 3; bool is_dataclass = 4; NodePairs attrs = 5`, attrs only when
/// non-empty (matching the generated encoder's absent field).
fn class_type_len(class: &ClassTypeNode) -> usize {
    str_len(1, &class.name)
        + encoding::message::encoded_len(2, &uuid_to_pb(&class.id))
        + int32_len(3, class_origin(class) as i32)
        + if class.is_dataclass {
            encoding::bool::encoded_len(4, &true)
        } else {
            0
        }
        + if class.attrs.is_empty() {
            0
        } else {
            submessage_len(5, node_pairs_len(1, &class.attrs))
        }
}

fn encode_class_type(class: &ClassTypeNode, buf: &mut impl BufMut) {
    encode_str(1, &class.name, buf);
    encoding::message::encode(2, &uuid_to_pb(&class.id), buf);
    encode_int32(3, class_origin(class) as i32, buf);
    if class.is_dataclass {
        encoding::bool::encode(4, &true, buf);
    }
    if !class.attrs.is_empty() {
        encode_message_key(5, node_pairs_len(1, &class.attrs), buf);
        encode_node_pairs(1, &class.attrs, buf);
    }
}

fn class_origin(class: &ClassTypeNode) -> pb::TypeOrigin {
    if class.host_defined {
        pb::TypeOrigin::Host
    } else {
        pb::TypeOrigin::Sandbox
    }
}

/// `ClassInstanceNode` body: `uint32 class_type = 1; Uuid instance_id = 2;
/// NodePairs attrs = 3`.
fn class_instance_len(class_type: NodeId, id: &pb::Uuid, attrs: &[(NodeId, NodeId)]) -> usize {
    uint32_len(1, class_type.0) + encoding::message::encoded_len(2, id) + submessage_len(3, node_pairs_len(1, attrs))
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

/// `Time` body: scalar fields 1–4 and `fold = 7` (implicit presence, skipped
/// at zero) plus explicit-presence `offset_seconds = 5` / `timezone_name = 6`.
fn time_len(t: &MontyTime) -> usize {
    uint32_len(1, u32::from(t.hour))
        + uint32_len(2, u32::from(t.minute))
        + uint32_len(3, u32::from(t.second))
        + uint32_len(4, t.microsecond)
        + t.offset_seconds.map_or(0, |off| encoding::int32::encoded_len(5, &off))
        + opt_str_len(6, t.timezone_name.as_deref())
        + uint32_len(7, u32::from(t.fold))
}

fn encode_time(t: &MontyTime, buf: &mut impl BufMut) {
    encode_uint32(1, u32::from(t.hour), buf);
    encode_uint32(2, u32::from(t.minute), buf);
    encode_uint32(3, u32::from(t.second), buf);
    encode_uint32(4, t.microsecond, buf);
    if let Some(off) = t.offset_seconds {
        encoding::int32::encode(5, &off, buf);
    }
    encode_opt_str(6, t.timezone_name.as_deref(), buf);
    encode_uint32(7, u32::from(t.fold), buf);
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

/// Decode-only `prost::Message` for one `MontyNode`: its `kind` oneof, decoded
/// and validated by [`decode_field`]. Never encoded (nodes encode via
/// [`NodeRef`]), so the encode methods are unreachable.
#[derive(Default)]
struct NodeBody(Option<MontyNode>);

impl Message for NodeBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        if let Some(node) = decode_field(tag, wire_type, buf, ctx)? {
            self.0 = Some(node);
        }
        Ok(())
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("NodeBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("NodeBody is decode-only")
    }

    fn clear(&mut self) {
        self.0 = None;
    }
}

/// Decodes one `MontyNode.kind` field, validating as it parses. `None`
/// means the tag was unknown and skipped (forward compatibility, matching
/// prost's generated decoder). Child ids are range-checked by
/// [`WireArena::into_graph`] once every node has arrived. Containers charge
/// their id and name vectors as they fill; leaves are charged once built.
fn decode_field(
    tag: u32,
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
) -> Result<Option<MontyNode>, DecodeError> {
    let node = match tag {
        tag::LIST => MontyNode::List(merge_message::<IndexesBody>(wire_type, buf, ctx)?.0),
        tag::TUPLE => MontyNode::Tuple(merge_message::<IndexesBody>(wire_type, buf, ctx)?.0),
        tag::SET => MontyNode::Set(merge_message::<IndexesBody>(wire_type, buf, ctx)?.0),
        tag::FROZEN_SET => MontyNode::FrozenSet(merge_message::<IndexesBody>(wire_type, buf, ctx)?.0),
        tag::NAMED_TUPLE => {
            let nt: NamedTupleBody = merge_message(wire_type, buf, ctx)?;
            MontyNode::NamedTuple {
                type_name: nt.type_name,
                field_names: nt.field_names,
                values: nt.values,
            }
        }
        tag::DICT => MontyNode::Dict(merge_message::<NodePairsBody>(wire_type, buf, ctx)?.0),
        tag::TYPE => type_to_node(merge_message(wire_type, buf, ctx)?)?,
        tag::CLASS_INSTANCE => {
            let ci: ClassInstanceBody = merge_message(wire_type, buf, ctx)?;
            let instance_id = ci
                .instance_id
                .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("ClassInstanceNode.instance_id")))?;
            let attrs = ci
                .attrs
                .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("ClassInstanceNode.attrs")))?;
            MontyNode::ClassInstance {
                class_type: NodeId(ci.class_type),
                instance_id: pb_uuid_to_monty(&instance_id, "ClassInstanceNode.instance_id")?,
                attrs: attrs.0,
            }
        }
        _ => return decode_leaf(tag, wire_type, buf, ctx),
    };
    Ok(Some(node))
}

/// Decodes one leaf kind, charging its payload once built: a leaf owns at
/// most its own wire bytes, so nothing amplifies before the charge.
fn decode_leaf(
    tag: u32,
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
) -> Result<Option<MontyNode>, DecodeError> {
    let node = match tag {
        tag::ELLIPSIS => {
            merge_message::<pb::Unit>(wire_type, buf, ctx)?;
            MontyNode::Ellipsis
        }
        tag::NOT_IMPLEMENTED => {
            merge_message::<pb::Unit>(wire_type, buf, ctx)?;
            MontyNode::NotImplemented
        }
        tag::NONE => {
            merge_message::<pb::Unit>(wire_type, buf, ctx)?;
            MontyNode::None
        }
        tag::BOOLEAN => {
            let mut v = false;
            encoding::bool::merge(wire_type, &mut v, buf, ctx)?;
            MontyNode::Bool(v)
        }
        tag::INT => {
            let mut v = 0i64;
            encoding::sint64::merge(wire_type, &mut v, buf, ctx)?;
            MontyNode::Int(v)
        }
        tag::BIGINT => MontyNode::BigInt(bigint_from_proto(&merge_message(wire_type, buf, ctx)?)),
        tag::FLOAT => {
            let mut v = 0f64;
            encoding::double::merge(wire_type, &mut v, buf, ctx)?;
            MontyNode::Float(v)
        }
        tag::STR => MontyNode::String(merge_string(wire_type, buf, ctx)?),
        tag::BYTES => {
            let mut v = Vec::new();
            encoding::bytes::merge(wire_type, &mut v, buf, ctx)?;
            MontyNode::Bytes(v)
        }
        tag::DATE => {
            let d: pb::Date = merge_message(wire_type, buf, ctx)?;
            MontyNode::Date(date_from_proto(&d).map_err(to_decode_err)?)
        }
        tag::DATETIME => {
            let dt: pb::DateTime = merge_message(wire_type, buf, ctx)?;
            MontyNode::DateTime(datetime_from_proto(dt).map_err(to_decode_err)?)
        }
        tag::TIME => {
            let t: pb::Time = merge_message(wire_type, buf, ctx)?;
            MontyNode::Time(time_from_proto(t).map_err(to_decode_err)?)
        }
        tag::TIMEDELTA => {
            let td: pb::TimeDelta = merge_message(wire_type, buf, ctx)?;
            MontyNode::TimeDelta(timedelta_from_proto(&td).map_err(to_decode_err)?)
        }
        tag::TIMEZONE => {
            let tz: pb::TimeZone = merge_message(wire_type, buf, ctx)?;
            MontyNode::TimeZone(MontyTimeZone {
                offset_seconds: timezone_offset(tz.offset_seconds, "TimeZone.offset_seconds").map_err(to_decode_err)?,
                name: tz.name,
            })
        }
        tag::EXCEPTION => {
            let exc: pb::Exception = merge_message(wire_type, buf, ctx)?;
            MontyNode::Exception {
                exc_type: exc
                    .exc_type
                    .parse()
                    .map_err(|_| to_decode_err(ProtoConvertError::UnknownExcType(exc.exc_type)))?,
                arg: exc.arg,
            }
        }
        tag::BUILTIN_FUNCTION => {
            let name = merge_string(wire_type, buf, ctx)?;
            MontyNode::BuiltinFunction(
                name.parse::<BuiltinsFunctions>()
                    .map_err(|_| to_decode_err(ProtoConvertError::UnknownBuiltinFunction(name)))?,
            )
        }
        tag::PATH => MontyNode::Path(merge_string(wire_type, buf, ctx)?),
        tag::FILE_HANDLE => {
            let fh: pb::FileHandle = merge_message(wire_type, buf, ctx)?;
            MontyNode::FileHandle(MontyFileHandle {
                mode: fh
                    .mode
                    .parse()
                    .map_err(|_| to_decode_err(ProtoConvertError::InvalidFileMode(fh.mode)))?,
                path: fh.path,
                position: fh.position,
            })
        }
        tag::FUNCTION => {
            let func: pb::Function = merge_message(wire_type, buf, ctx)?;
            MontyNode::Function {
                name: func.name,
                docstring: func.docstring,
            }
        }
        tag::REPR => MontyNode::Repr(merge_string(wire_type, buf, ctx)?),
        tag::CYCLE => MontyNode::Cycle(merge_string(wire_type, buf, ctx)?),
        _ => {
            skip_field(wire_type, tag, buf, ctx)?;
            return Ok(None);
        }
    };
    charge_decode(node.decoded_size().saturating_sub(size_of::<MontyNode>()))?;
    Ok(Some(node))
}

/// Decodes one length-delimited sub-message into a fresh `M` (the generated
/// leaf and container types). The arena is flat, so `message::merge`'s
/// recursion limit only ever sees a fixed two or three levels here.
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

/// Decodes one `repeated uint32` field of node ids (packed or not) straight
/// into `ids`. A packed run of `n` bytes holds at most `n` ids, so that many
/// slots are charged and reserved before any is read; no temporary vector.
fn merge_ids(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
    ids: &mut Vec<NodeId>,
) -> Result<(), DecodeError> {
    let mut id = 0u32;
    if wire_type == WireType::LengthDelimited {
        // the same length checks as prost's packed `merge_loop`
        let len = encoding::decode_varint(buf)?;
        let len = usize::try_from(len)
            .ok()
            .filter(|len| *len <= buf.remaining())
            .ok_or_else(|| to_decode_err("buffer underflow"))?;
        reserve_charged(ids, len)?;
        let end = buf.remaining() - len;
        while buf.remaining() > end {
            encoding::uint32::merge(WireType::Varint, &mut id, buf, ctx.clone())?;
            ids.push(NodeId(id));
        }
        if buf.remaining() == end {
            Ok(())
        } else {
            Err(to_decode_err("delimited length exceeded"))
        }
    } else {
        encoding::uint32::merge(wire_type, &mut id, buf, ctx)?;
        push_charged(ids, NodeId(id))
    }
}

/// Appends `item`, charging the decode budget for the slots the vector grows
/// into (doubling, as `Vec` does) before it allocates them.
fn push_charged<T>(vec: &mut Vec<T>, item: T) -> Result<(), DecodeError> {
    if vec.len() == vec.capacity() {
        let new_capacity = vec.capacity().saturating_mul(2).max(MIN_VEC_CAPACITY);
        reserve_charged(vec, new_capacity - vec.len())?;
    }
    vec.push(item);
    Ok(())
}

/// Reserves room for `additional` more items, charging the slots the vector gains.
fn reserve_charged<T>(vec: &mut Vec<T>, additional: usize) -> Result<(), DecodeError> {
    let spare = vec.capacity() - vec.len();
    if additional > spare {
        charge_decode((additional - spare).saturating_mul(size_of::<T>()))?;
        vec.reserve_exact(additional);
    }
    Ok(())
}

/// Decodes one string field, charging its bytes once prost has read them (a
/// string owns no more than its wire bytes, so nothing amplifies first).
fn merge_string_charged(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
    value: &mut String,
) -> Result<(), DecodeError> {
    encoding::string::merge(wire_type, value, buf, ctx)?;
    charge_decode(value.len())
}

/// Decode-only `Indexes` (list, tuple, set and frozenset payloads): the ids
/// decode straight into the node's vector, charged as they arrive.
#[derive(Default)]
struct IndexesBody(Vec<NodeId>);

impl Message for IndexesBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => merge_ids(wire_type, buf, ctx, &mut self.0),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("IndexesBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("IndexesBody is decode-only")
    }

    fn clear(&mut self) {
        self.0 = Vec::new();
    }
}

/// Decode-only `NodePairs` (dict and attribute payloads), each pair charged
/// as it is pushed.
#[derive(Default)]
struct NodePairsBody(Vec<(NodeId, NodeId)>);

impl Message for NodePairsBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => {
                let pair: pb::NodePair = merge_message(wire_type, buf, ctx)?;
                push_charged(&mut self.0, (NodeId(pair.key), NodeId(pair.value)))
            }
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("NodePairsBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("NodePairsBody is decode-only")
    }

    fn clear(&mut self) {
        self.0 = Vec::new();
    }
}

/// Decode-only `NamedTupleNode`: names and value ids charged as they arrive
/// (a field name costs its `String` slot plus its bytes).
#[derive(Default)]
struct NamedTupleBody {
    type_name: String,
    field_names: Vec<String>,
    values: Vec<NodeId>,
}

impl Message for NamedTupleBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => merge_string_charged(wire_type, buf, ctx, &mut self.type_name),
            2 => {
                let mut name = String::new();
                merge_string_charged(wire_type, buf, ctx, &mut name)?;
                push_charged(&mut self.field_names, name)
            }
            3 => merge_ids(wire_type, buf, ctx, &mut self.values),
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
        self.field_names = Vec::new();
        self.values = Vec::new();
    }
}

/// Decode-only `Type`: the fields [`type_to_node`] validates, with the name
/// and attrs charged as they arrive.
#[derive(Default)]
struct TypeBody {
    name: String,
    id: Option<pb::Uuid>,
    origin: i32,
    is_dataclass: bool,
    attrs: Option<NodePairsBody>,
}

impl Message for TypeBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => merge_string_charged(wire_type, buf, ctx, &mut self.name),
            2 => encoding::message::merge(wire_type, self.id.get_or_insert_default(), buf, ctx),
            3 => encoding::int32::merge(wire_type, &mut self.origin, buf, ctx),
            4 => encoding::bool::merge(wire_type, &mut self.is_dataclass, buf, ctx),
            5 => encoding::message::merge(wire_type, self.attrs.get_or_insert_default(), buf, ctx),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("TypeBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("TypeBody is decode-only")
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Decode-only `ClassInstanceNode`, its attrs charged as they arrive.
#[derive(Default)]
struct ClassInstanceBody {
    class_type: u32,
    instance_id: Option<pb::Uuid>,
    attrs: Option<NodePairsBody>,
}

impl Message for ClassInstanceBody {
    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => encoding::uint32::merge(wire_type, &mut self.class_type, buf, ctx),
            2 => encoding::message::merge(wire_type, self.instance_id.get_or_insert_default(), buf, ctx),
            3 => encoding::message::merge(wire_type, self.attrs.get_or_insert_default(), buf, ctx),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("ClassInstanceBody is decode-only")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("ClassInstanceBody is decode-only")
    }

    fn clear(&mut self) {
        *self = Self::default();
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

/// Encodes a [`MontyUuid`] as the wire `Uuid` message (16 raw bytes).
pub(crate) fn uuid_to_pb(uuid: &MontyUuid) -> pb::Uuid {
    pb::Uuid {
        data: uuid.as_bytes().to_vec(),
    }
}

/// Validates a wire `Uuid`: exactly 16 bytes, anything else is rejected.
fn pb_uuid_to_monty(uuid: &pb::Uuid, field: &'static str) -> Result<MontyUuid, DecodeError> {
    MontyUuid::try_from_slice(&uuid.data).ok_or_else(|| {
        to_decode_err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("uuid must be 16 bytes, got {}", uuid.data.len()),
        })
    })
}

/// Encodes a builtin [`MontyType`] as the wire `Type` message: only its
/// Display name (origin BUILTIN, no id). Class types are [`ClassTypeNode`]s
/// and encode via [`encode_class_type`].
fn builtin_type_to_pb(t: &MontyType) -> pb::Type {
    pb::Type {
        name: t.to_string(),
        origin: pb::TypeOrigin::Builtin as i32,
        ..pb::Type::default()
    }
}

/// Validates a decoded wire `Type` into a builtin type leaf or a class node
/// (its box charged here; the name and attrs were charged as they decoded).
/// BUILTIN must not carry an id or attrs; SANDBOX/HOST must carry an id.
fn type_to_node(ty: TypeBody) -> Result<MontyNode, DecodeError> {
    let origin = pb::TypeOrigin::try_from(ty.origin).map_err(|_| {
        to_decode_err(ProtoConvertError::InvalidValue {
            field: "Type.origin",
            reason: format!("unknown origin {}", ty.origin),
        })
    })?;
    let invalid = |reason: &str| {
        to_decode_err(ProtoConvertError::InvalidValue {
            field: "Type",
            reason: reason.to_owned(),
        })
    };
    match origin {
        pb::TypeOrigin::Unspecified => Err(invalid("origin must be specified")),
        pb::TypeOrigin::Builtin => {
            if ty.id.is_some() {
                Err(invalid("a builtin type must not carry an id"))
            } else if ty.attrs.is_some() {
                Err(invalid("a builtin type must not carry attrs"))
            } else {
                MontyType::from_type_name(&ty.name)
                    .map(MontyNode::Type)
                    .ok_or_else(|| to_decode_err(ProtoConvertError::UnknownType(ty.name)))
            }
        }
        pb::TypeOrigin::Sandbox | pb::TypeOrigin::Host => {
            let id = ty.id.ok_or_else(|| invalid("a class type must carry an id"))?;
            charge_decode(size_of::<ClassTypeNode>())?;
            Ok(MontyNode::ClassType(Box::new(ClassTypeNode {
                name: ty.name,
                id: pb_uuid_to_monty(&id, "Type.id")?,
                host_defined: origin == pb::TypeOrigin::Host,
                is_dataclass: ty.is_dataclass,
                attrs: ty.attrs.map(|attrs| attrs.0).unwrap_or_default(),
            })))
        }
    }
}

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
        offset_seconds: dt
            .offset_seconds
            .map(|offset| timezone_offset(offset, "DateTime.offset_seconds"))
            .transpose()?,
        timezone_name: dt.timezone_name,
    })
}

fn time_from_proto(t: pb::Time) -> Result<MontyTime, ProtoConvertError> {
    if t.offset_seconds.is_none() && t.timezone_name.is_some() {
        return Err(ProtoConvertError::InvalidValue {
            field: "Time.timezone_name",
            reason: "timezone_name requires offset_seconds".to_owned(),
        });
    }
    Ok(MontyTime {
        hour: ranged_u8(t.hour, 0..=23, "Time.hour")?,
        minute: ranged_u8(t.minute, 0..=59, "Time.minute")?,
        second: ranged_u8(t.second, 0..=59, "Time.second")?,
        microsecond: bounded(t.microsecond, 999_999, "Time.microsecond")?,
        offset_seconds: t
            .offset_seconds
            .map(|offset| timezone_offset(offset, "Time.offset_seconds"))
            .transpose()?,
        timezone_name: t.timezone_name,
        fold: ranged_u8(t.fold, 0..=1, "Time.fold")?,
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

/// Checks a wire UTC offset against the range `datetime.timezone`
/// accepts, so a forged offset names its own field here rather than surfacing as
/// a generic bad value when the sandbox-side constructor rejects it.
fn timezone_offset(offset: i32, field: &'static str) -> Result<i32, ProtoConvertError> {
    if (MIN_TIMEZONE_OFFSET_SECONDS..=MAX_TIMEZONE_OFFSET_SECONDS).contains(&offset) {
        Ok(offset)
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!(
                "{offset} is outside the range {MIN_TIMEZONE_OFFSET_SECONDS}..={MAX_TIMEZONE_OFFSET_SECONDS}"
            ),
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
    /// [`WireArena::merge_field`] — and that fixed signature has no slot to
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
/// frame still can't amplify cheap nodes into GiB of host `MontyNode`s.
///
/// Callers that decode a message *without* going through [`crate::FrameReader`]
/// (e.g. a transport that does its own framing, like a WebSocket) MUST call
/// this before each `Message::decode`, or the budget drains cumulatively across
/// decodes on the same thread and eventually rejects legitimate messages.
pub fn reset_decode_budget() {
    DECODE_BUDGET.set(DEFAULT_MAX_DECODE_BYTES);
}

/// Charges `bytes` of decoded host memory against the current frame's budget,
/// erroring once a frame would exceed it. Every vector the decoders grow is
/// charged before it allocates, so an over-budget frame is rejected before it
/// is fully built.
fn charge_decode(bytes: usize) -> Result<(), DecodeError> {
    DECODE_BUDGET.with(|budget| match budget.get().checked_sub(bytes) {
        Some(remaining) => {
            budget.set(remaining);
            Ok(())
        }
        None => Err(to_decode_err("frame exceeds decode memory budget")),
    })
}
