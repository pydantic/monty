//! The per-frame decode budget as `WireArena` charges it: the sender's
//! `node_count` hint reserves the arena up front, capped by the bytes actually
//! present, and a frame that could not fit is rejected before it is built.

use monty_proto::{DEFAULT_MAX_DECODE_BYTES, WireArena, reset_decode_budget};
use monty_types::MontyNode;
use prost::{Message, encoding::encode_varint};

/// One `ValueNode { none }` entry as `Arena.nodes` encodes it: entry key and
/// length, then the `Unit none = 2` kind key and its empty length.
const NONE_NODE: [u8; 4] = [0x12, 0x02, 0x12, 0x00];

/// An `Arena` frame claiming `hint` nodes and carrying `nodes` `None` entries.
fn arena_bytes(hint: u32, nodes: usize) -> Vec<u8> {
    let mut bytes = vec![0x08];
    encode_varint(u64::from(hint), &mut bytes);
    bytes.extend(NONE_NODE.repeat(nodes));
    bytes
}

fn decode(bytes: &[u8]) -> Result<Vec<MontyNode>, String> {
    reset_decode_budget();
    WireArena::decode(bytes)
        .map(|arena| arena.0)
        .map_err(|err| err.to_string())
}

/// A hint far beyond the bytes present reserves only what those bytes could
/// hold, so a lying peer cannot make the receiver allocate on its say-so.
#[test]
fn node_count_hint_is_capped_by_the_message_size() {
    let nodes = decode(&arena_bytes(u32::MAX, 2)).expect("a lying hint still decodes");
    assert_eq!(nodes, vec![MontyNode::None, MontyNode::None]);
}

/// An arena whose nodes would outgrow the budget once decoded is refused up
/// front, while the bytes are still just bytes.
#[test]
fn oversized_arena_is_rejected_before_it_is_built() {
    let too_many = DEFAULT_MAX_DECODE_BYTES / size_of::<MontyNode>() + 1;
    let bytes = arena_bytes(u32::try_from(too_many).unwrap(), too_many);
    assert_eq!(
        decode(&bytes).unwrap_err(),
        "failed to decode Protobuf message: frame exceeds decode memory budget"
    );
}

/// Without a hint the arena grows by doubling, each step charged; a small
/// arena decodes exactly.
#[test]
fn unhinted_arena_decodes() {
    let bytes = arena_bytes(0, 9);
    assert_eq!(decode(&bytes).unwrap().len(), 9);
}
