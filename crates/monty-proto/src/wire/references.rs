//! Extern-mapped protobuf containers that decode directly into domain references.

use monty_types::unstable::NodeId;
use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType, skip_field},
};

use super::{
    encode_node_pairs, encode_packed_ids, encode_repeated_str, encode_str, merge_ids, merge_message, named_tuple_len,
    node_pairs_len, packed_ids_len, push_charged,
};
use crate::{BudgetVec, budgeted_prost::encoding, pb};

/// Wire `Indexes` stored as domain ids, avoiding an intermediate `Vec<u32>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct WireIndexes(pub BudgetVec<NodeId>);

impl Message for WireIndexes {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_packed_ids(1, &self.0, buf);
    }

    fn encoded_len(&self) -> usize {
        packed_ids_len(1, &self.0)
    }

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

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Wire `NodePairs` stored as domain pairs, with no intermediate pair vector.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct WireNodePairs(pub BudgetVec<(NodeId, NodeId)>);

impl Message for WireNodePairs {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_node_pairs(1, &self.0, buf);
    }

    fn encoded_len(&self) -> usize {
        node_pairs_len(1, &self.0)
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
                let pair: pb::NodePair = merge_message(wire_type, buf, ctx)?;
                push_charged(&mut self.0, (NodeId(pair.key), NodeId(pair.value)))
            }
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Wire named tuple with domain ids and budgeted names, ready for zero-copy conversion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct WireNamedTuple {
    /// Name used when rendering the tuple.
    pub type_name: String,
    /// Ordered attribute names, one per value.
    pub field_names: BudgetVec<String>,
    /// Child ids, checked against the arena after decoding.
    pub values: BudgetVec<NodeId>,
}

impl Message for WireNamedTuple {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        encode_str(1, &self.type_name, buf);
        encode_repeated_str(2, &self.field_names, buf);
        encode_packed_ids(3, &self.values, buf);
    }

    fn encoded_len(&self) -> usize {
        named_tuple_len(&self.type_name, &self.field_names, &self.values)
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match tag {
            1 => encoding::string::merge(wire_type, &mut self.type_name, buf, ctx),
            2 => encoding::string::merge_repeated(wire_type, &mut self.field_names, buf, ctx),
            3 => merge_ids(wire_type, buf, ctx, &mut self.values),
            _ => skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn clear(&mut self) {
        self.type_name.clear();
        self.field_names.clear();
        self.values.clear();
    }
}
