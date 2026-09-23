//! Borrowed protobuf encoding and incremental decoding of the value arena.
//!
//! Codegen maps `monty.v1.Arena` to [`WireArena`] via `extern_path`:
//! - **encode** writes borrowed [`MontyNode`]s without an intermediate arena or clones;
//! - **decode** parses one generated [`pb::MontyNode`], validates its completed
//!   payload, and converts it before appending to the domain arena. Strings and
//!   bytes move without copying. Extern-mapped containers decode references directly
//!   into domain buffers, so conversion does not allocate a second reference vector.
//!
//! Protobuf field merging stays in prost, including duplicate message kinds.
//! Arena index invariants are checked once, by [`WireArena::into_graph`]. The
//! flat layout avoids value-depth-dependent recursion in either direction.
//!
//! Generated payloads and domain conversions share one cumulative frame budget,
//! charged before allocation. Node references also carry an allowance for host
//! container storage; growing a buffer pays for the full replacement allocation.
//!
//! `tests/differential.rs` checks byte compatibility and duplicate-field merging
//! against a fully generated oracle compiled from the same schema.
//!
//! Encoding leaf arms whose wire form is a `Display` rendering (`MontyType`,
//! builtin functions, exception type names) allocates the rendered string in
//! both `encoded_len` and `encode_raw`; those arms are rare in real payloads
//! and the strings are tiny.

mod references;

use std::{
    fmt::Display,
    io::{Cursor, Write},
    ops::RangeInclusive,
};

use monty_types::{
    BuiltinsFunctions, CallArgs, MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS, MontyDate, MontyDateTime,
    MontyFileHandle, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, SourceRange,
    unstable::{self, ClassTypeNode, GraphError, MontyGraph, MontyNode, NodeId},
};
use num_bigint::{BigInt, Sign};
use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{
        DecodeContext, WireType, decode_varint, encode_key, encode_varint, encoded_len_varint, key_len, skip_field,
    },
};
pub use references::{WireIndexes, WireNamedTuple, WireNodePairs};

use crate::{
    BudgetVec,
    budgeted_prost::encoding,
    convert::ProtoConvertError,
    decode_budget,
    pb::{self, monty_node::Kind},
};

/// The wire form of a [`MontyGraph`]: what the `monty.v1.Arena` proto message
/// decodes into and encodes from.
///
/// Decoding only collects nodes; [`Self::into_graph`] checks the arena
/// invariants (every child index lower than its holder, class instances
/// pointing at class nodes) once the whole message has arrived, since prost
/// has no end-of-message hook. Senders build it from a validated graph via `From`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireArena(pub BudgetVec<MontyNode>);

impl WireArena {
    /// Wraps a graph for sending. Equivalent to `From`, named for call sites
    /// where `.into()` would be unclear.
    #[must_use]
    pub fn new(graph: MontyGraph) -> Self {
        Self(graph.into_nodes().into())
    }

    /// Validates the decoded nodes into a graph.
    pub fn into_graph(self) -> Result<MontyGraph, ProtoConvertError> {
        MontyGraph::from_nodes(self.0.into_inner()).map_err(|err| graph_error(&err))
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
                    self.0.try_reserve_capacity(reserve)?;
                }
                Ok(())
            }
            2 => {
                // the node's payload was charged as it decoded; this charges its slot
                let node = node_from_proto(merge_message(wire_type, buf, ctx)?)?;
                self.0.try_push(node)
            }
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    /// Releases the arena's storage without refunding its allocation charge.
    fn clear(&mut self) {
        self.0 = BudgetVec::new();
    }
}

/// Wire form of `monty.v1.FunctionCall`: the call's argument arena plus the
/// ids of its positional and keyword arguments.
///
/// Installed with `prost_build::extern_path`, so generated `ChildEvent`
/// decoding still handles the envelope while this payload keeps the argument
/// vectors as bare ids and uses `WireArena`'s incremental node decoding.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WireFunctionCall {
    /// Name of the external function the sandbox is calling.
    pub function_name: String,
    /// The arena `args` and `kwargs` index.
    pub values: WireArena,
    /// Positional arguments, in order.
    pub args: BudgetVec<NodeId>,
    /// Keyword arguments as `(key, value)` ids, preserving wire order.
    pub kwargs: BudgetVec<(NodeId, NodeId)>,
    /// Child-assigned call id used by the matching resume request.
    pub call_id: u32,
    /// Uuid of the routed receiver (a host-backed instance, or a class type
    /// for `__call__`/classmethod calls); `None` for plain external function
    /// calls. The receiver is never included in `args`.
    pub object_id: Option<MontyUuid>,
    /// The worker accepts an eagerly settled coroutine via `ResumeFutures`.
    pub allow_eager_await: bool,
    /// Where the call expression is in the source; `None` for a frame from a
    /// child that predates the field.
    pub position: Option<SourceRange>,
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
        position: SourceRange,
    ) -> Self {
        let (graph, args, kwargs) = unstable::into_call_args_parts(args);
        Self {
            function_name,
            values: WireArena::new(graph),
            args: args.into(),
            kwargs: kwargs.into(),
            call_id,
            object_id,
            allow_eager_await,
            position: Some(position),
        }
    }

    /// Validates the decoded arena and argument ids into [`CallArgs`].
    pub fn into_call_args(self) -> Result<CallArgs, ProtoConvertError> {
        unstable::call_args_from_parts(
            self.values.into_graph()?,
            self.args.into_inner(),
            self.kwargs.into_inner(),
        )
        .map_err(|err| graph_error(&err))
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
        if let Some(position) = &self.position {
            encode_source_range(8, position, buf);
        }
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
            + self
                .position
                .as_ref()
                .map_or(0, |position| submessage_len(8, source_range_len(position)))
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
            8 => {
                // a singular message field repeated on the wire merges, as in
                // prost's generated decoder; the held value moves rather than
                // clones, so repeats cost their own bytes and not the filename's
                let mut position = self
                    .position
                    .take()
                    .map_or_else(pb::SourceRange::default, pb::SourceRange::from);
                encoding::message::merge(wire_type, &mut position, buf, ctx)?;
                self.position = Some(SourceRange::from(position));
                Ok(())
            }
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn clear(&mut self) {
        self.function_name.clear();
        self.values.clear();
        self.args = BudgetVec::new();
        self.kwargs = BudgetVec::new();
        self.call_id = 0;
        self.object_id = None;
        self.allow_eager_await = false;
        self.position = None;
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
    // (monty has no uuid module); node conversion rejects it.
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
        MontyNode::Type(t) => encoding::message::encode(tag::TYPE, &builtin_type_to_pb(*t), buf),
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
        MontyNode::Type(t) => encoding::message::encoded_len(tag::TYPE, &builtin_type_to_pb(*t)),
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

/// A `SourceRange` message encoded from the borrowed domain value, so the
/// filename is written in place rather than cloned into a `pb::SourceRange`.
fn encode_source_range(tag: u32, range: &SourceRange, buf: &mut impl BufMut) {
    encode_message_key(tag, source_range_len(range), buf);
    encode_str(1, &range.filename, buf);
    encoding::message::encode(2, &pb::CodeLoc::from(range.start), buf);
    encoding::message::encode(3, &pb::CodeLoc::from(range.end), buf);
}

/// Body length of [`encode_source_range`]'s message.
fn source_range_len(range: &SourceRange) -> usize {
    str_len(1, &range.filename)
        + encoding::message::encoded_len(2, &pb::CodeLoc::from(range.start))
        + encoding::message::encoded_len(3, &pb::CodeLoc::from(range.end))
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

/// Validates one generated node before retaining it in the domain arena.
/// Owned buffers transfer without copying; BigInt limbs and class boxes are
/// allocated under the same frame budget as the generated payload.
#[expect(clippy::inline_always, reason = "measured improvement in frame decode benchmarks")]
#[inline(always)]
fn node_from_proto(node: pb::MontyNode) -> Result<MontyNode, DecodeError> {
    let kind = node
        .kind
        .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("MontyNode.kind")))?;
    Ok(match kind {
        Kind::Ellipsis(_) => MontyNode::Ellipsis,
        Kind::NotImplemented(_) => MontyNode::NotImplemented,
        Kind::None(_) => MontyNode::None,
        Kind::Boolean(value) => MontyNode::Bool(value),
        Kind::Int(value) => MontyNode::Int(value),
        Kind::Float(value) => MontyNode::Float(value),
        Kind::Str(value) => MontyNode::String(value),
        Kind::Bytes(value) => MontyNode::Bytes(value.into_inner()),
        Kind::List(value) => MontyNode::List(value.0.into_inner()),
        Kind::Tuple(value) => MontyNode::Tuple(value.0.into_inner()),
        Kind::Set(value) => MontyNode::Set(value.0.into_inner()),
        Kind::FrozenSet(value) => MontyNode::FrozenSet(value.0.into_inner()),
        Kind::NamedTuple(value) => MontyNode::NamedTuple {
            type_name: value.type_name,
            field_names: value.field_names.into_inner(),
            values: value.values.into_inner(),
        },
        Kind::Dict(value) => MontyNode::Dict(value.0.into_inner()),
        Kind::Path(value) => MontyNode::Path(value),
        Kind::Function(value) => MontyNode::Function {
            name: value.name,
            docstring: value.docstring,
        },
        Kind::Repr(value) => MontyNode::Repr(value),
        Kind::Cycle(value) => MontyNode::Cycle(value),
        Kind::Bigint(value) => MontyNode::BigInt(bigint_from_proto(value)?),
        Kind::Type(value) => type_to_node(value)?,
        Kind::ClassInstance(value) => {
            let instance_id = value
                .instance_id
                .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("ClassInstanceNode.instance_id")))?;
            let attrs = value
                .attrs
                .ok_or_else(|| to_decode_err(ProtoConvertError::MissingField("ClassInstanceNode.attrs")))?;
            MontyNode::ClassInstance {
                class_type: NodeId(value.class_type),
                instance_id: pb_uuid_to_monty(&instance_id, "ClassInstanceNode.instance_id")?,
                attrs: attrs.0.into_inner(),
            }
        }
        Kind::Date(value) => MontyNode::Date(date_from_proto(&value).map_err(to_decode_err)?),
        Kind::Datetime(value) => MontyNode::DateTime(datetime_from_proto(value).map_err(to_decode_err)?),
        Kind::Time(value) => MontyNode::Time(time_from_proto(value).map_err(to_decode_err)?),
        Kind::Timedelta(value) => MontyNode::TimeDelta(timedelta_from_proto(&value).map_err(to_decode_err)?),
        Kind::Timezone(value) => MontyNode::TimeZone(MontyTimeZone {
            offset_seconds: timezone_offset(value.offset_seconds, "TimeZone.offset_seconds").map_err(to_decode_err)?,
            name: value.name,
        }),
        Kind::Exception(value) => MontyNode::Exception {
            exc_type: value
                .exc_type
                .parse()
                .map_err(|_| to_decode_err(ProtoConvertError::UnknownExcType(value.exc_type)))?,
            arg: value.arg,
        },
        Kind::BuiltinFunction(name) => MontyNode::BuiltinFunction(
            name.parse::<BuiltinsFunctions>()
                .map_err(|_| to_decode_err(ProtoConvertError::UnknownBuiltinFunction(name)))?,
        ),
        Kind::FileHandle(value) => MontyNode::FileHandle(MontyFileHandle {
            mode: value
                .mode
                .parse()
                .map_err(|_| to_decode_err(ProtoConvertError::InvalidFileMode(value.mode)))?,
            path: value.path,
            position: value.position,
        }),
        // The schema reserves UUID values, but Monty does not support them yet.
        Kind::Uuid(_) => return Err(to_decode_err(ProtoConvertError::MissingField("MontyNode.kind"))),
    })
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

/// Decodes one `repeated uint32` field of node ids (packed or not) straight
/// into `ids`. A packed run of `n` bytes holds at most `n` ids, so that many
/// slots are charged and reserved before any is read; no temporary vector.
fn merge_ids(
    wire_type: WireType,
    buf: &mut impl Buf,
    ctx: DecodeContext,
    ids: &mut BudgetVec<NodeId>,
) -> Result<(), DecodeError> {
    let mut id = 0u32;
    if wire_type == WireType::LengthDelimited {
        // the same length checks as prost's packed `merge_loop`
        let len = decode_varint(buf)?;
        let len = usize::try_from(len)
            .ok()
            .filter(|len| *len <= buf.remaining())
            .ok_or_else(|| to_decode_err("buffer underflow"))?;
        reserve_charged(ids, len)?;
        let end = buf.remaining() - len;
        while buf.remaining() > end {
            encoding::uint32::merge(WireType::Varint, &mut id, buf, ctx.clone())?;
            ids.try_push_reserved(NodeId(id))?;
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

/// Bytes charged per reference to a node. A host binding turns each reference
/// into a pointer-sized container entry, often through a temporary vector, so
/// the 4-byte [`NodeId`] alone would let a list of repeated references
/// materialise at several times the budget.
const REFERENCE_COST: usize = 2 * size_of::<usize>();

/// Budget charge for a reference-buffer slot, including host-container headroom.
trait DecodeCost {
    const COST: usize;
}

impl DecodeCost for NodeId {
    const COST: usize = REFERENCE_COST;
}

impl DecodeCost for (NodeId, NodeId) {
    const COST: usize = 2 * REFERENCE_COST;
}

/// Appends references, including host-container headroom in any growth charge.
fn push_charged<T: DecodeCost>(vec: &mut BudgetVec<T>, item: T) -> Result<(), DecodeError> {
    if vec.len() == vec.capacity() {
        let new_capacity = vec
            .capacity()
            .checked_mul(2)
            .ok_or_else(decode_budget::exhausted)?
            .max(MIN_VEC_CAPACITY);
        reserve_charged(vec, new_capacity - vec.len())?;
    }
    vec.try_push_reserved(item)
}

/// Charges a full replacement buffer, including the host-reference allowance.
fn reserve_charged<T: DecodeCost>(vec: &mut BudgetVec<T>, additional: usize) -> Result<(), DecodeError> {
    let capacity = vec.len().checked_add(additional).ok_or_else(decode_budget::exhausted)?;
    vec.try_reserve_capacity_with_overhead(capacity, T::COST - size_of::<T>())
}

/// Maps a semantic validation failure onto prost's decode error so it
/// surfaces through the normal frame-decode path.
//
// `DecodeError::new` is deprecated but has no public replacement in prost
// 0.14 (`DecodeErrorKind` is crate-private); the deprecation note itself
// acknowledges external users. Revisit when prost ships a public constructor.
#[expect(deprecated)]
fn to_decode_err(err: impl Display) -> DecodeError {
    let mut buffer = [0; 512];
    let mut cursor = Cursor::new(buffer.as_mut_slice());
    if write!(cursor, "{err}").is_ok() {
        let len = usize::try_from(cursor.position()).expect("bounded error buffer");
        DecodeError::new(str::from_utf8(&buffer[..len]).expect("Display writes UTF-8").to_owned())
    } else {
        DecodeError::new("invalid wire value (error message exceeds 512 bytes)")
    }
}

// ============================================================================
// Leaf conversions and validation (the wire is untrusted)
// ============================================================================

/// Encodes a [`MontyUuid`] as the wire `Uuid` message (16 raw bytes).
pub(crate) fn uuid_to_pb(uuid: &MontyUuid) -> pb::Uuid {
    pb::Uuid {
        data: uuid.as_bytes().to_vec().into(),
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
fn builtin_type_to_pb(t: MontyType) -> pb::Type {
    pb::Type {
        name: t.to_string(),
        origin: pb::TypeOrigin::Builtin as i32,
        ..pb::Type::default()
    }
}

/// Validates a wire `Type`, budgeting the class box. Its strings and attribute
/// buffers have already been charged during decoding.
/// BUILTIN must not carry an id or attrs; SANDBOX/HOST must carry an id.
fn type_to_node(ty: pb::Type) -> Result<MontyNode, DecodeError> {
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
            Ok(MontyNode::ClassType(decode_budget::boxed(ClassTypeNode {
                name: ty.name,
                id: pb_uuid_to_monty(&id, "Type.id")?,
                host_defined: origin == pb::TypeOrigin::Host,
                is_dataclass: ty.is_dataclass,
                attrs: ty.attrs.map(|attrs| attrs.0.into_inner()).unwrap_or_default(),
            })?))
        }
    }
}

/// Encodes a `BigInt` as sign + big-endian magnitude.
fn bigint_to_proto(bi: &BigInt) -> pb::BigInt {
    let (sign, magnitude) = bi.to_bytes_be();
    pb::BigInt {
        negative: sign == Sign::Minus,
        magnitude: magnitude.into(),
    }
}

/// Decodes sign + big-endian magnitude back to a `BigInt`.
///
/// An all-zero/empty magnitude decodes to zero regardless of the sign flag —
/// `BigInt` normalizes the sign of zero, so no invalid state is possible.
fn bigint_from_proto(mut bi: pb::BigInt) -> Result<BigInt, DecodeError> {
    let sign = if bi.negative { Sign::Minus } else { Sign::Plus };
    // Avoid num-bigint's temporary reversed copy and shrinking allocation for zero padding.
    bi.magnitude.reverse();
    let len = bi.magnitude.iter().rposition(|&byte| byte != 0).map_or(0, |i| i + 1);
    bi.magnitude.truncate(len);
    if len > 0 {
        // Cover rounding to 32- or 64-bit limbs and Vec's minimum capacity.
        decode_budget::charge((len.div_ceil(8) * 8).max(32))?;
    }
    Ok(BigInt::from_bytes_le(sign, &bi.magnitude))
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
