//! The per-frame decode budget as `WireArena` charges it: the sender's
//! `node_count` hint reserves the arena up front, capped by the bytes actually
//! present, and a frame that could not fit is rejected before it is built.

use monty_proto::{DEFAULT_MAX_DECODE_BYTES, WireArena, WireFunctionCall, reset_decode_budget};
use monty_types::unstable::{MontyNode, NodeId};
use prost::{Message, encoding::encode_varint};

/// One `MontyNode { none }` entry as `Arena.nodes` encodes it: entry key and
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
/// hold.
#[test]
fn node_count_hint_is_capped_by_the_message_size() {
    let nodes = decode(&arena_bytes(u32::MAX, 2)).expect("a lying hint still decodes");
    assert_eq!(nodes, vec![MontyNode::None, MontyNode::None]);
}

/// An arena whose nodes would outgrow the budget is rejected when its hint is
/// charged, before any node is built.
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

/// A length small enough to be one varint byte.
fn byte(len: usize) -> u8 {
    u8::try_from(len).expect("fits one varint byte")
}

/// One `MontyNode { list }` entry holding `ids` packed zero ids.
fn list_node(ids: usize) -> Vec<u8> {
    let indexes = [vec![0x0a, byte(ids)], vec![0u8; ids]].concat();
    let kind = [vec![0x5a, byte(indexes.len())], indexes].concat();
    [vec![0x12, byte(kind.len())], kind].concat()
}

/// One `MontyNode { named_tuple }` entry with `names` empty field names.
fn named_tuple_node(names: usize) -> Vec<u8> {
    let body = [0x12, 0x00].repeat(names);
    let kind = [vec![0x6a, byte(body.len())], body].concat();
    [vec![0x12, byte(kind.len())], kind].concat()
}

/// An arena that fills the budget to within two node slots, room for 8
/// references: `full` `None` entries, the last replaced by `last` when given.
fn nearly_full_arena(last: Option<Vec<u8>>) -> Vec<u8> {
    let full = DEFAULT_MAX_DECODE_BYTES / size_of::<MontyNode>() - 1;
    let mut bytes = vec![0x08];
    encode_varint(u64::try_from(full).unwrap(), &mut bytes);
    bytes.extend(NONE_NODE.repeat(full - usize::from(last.is_some())));
    bytes.extend(last.unwrap_or_default());
    bytes
}

/// A `FunctionCall` frame: the arena first, then `args` (packed when `packed`)
/// and `kwargs` pairs, so the arena is charged before the ids are read.
fn function_call_bytes(arena: &[u8], args: usize, packed: bool, kwargs: usize) -> Vec<u8> {
    let mut bytes = vec![0x3a];
    encode_varint(arena.len() as u64, &mut bytes);
    bytes.extend_from_slice(arena);
    if packed {
        bytes.extend([0x12, byte(args)]);
        bytes.extend(vec![0u8; args]);
    } else {
        bytes.extend([0x10, 0x00].repeat(args));
    }
    bytes.extend([0x1a, 0x00].repeat(kwargs));
    bytes
}

fn decode_call(bytes: &[u8]) -> Result<WireFunctionCall, String> {
    reset_decode_budget();
    WireFunctionCall::decode(bytes).map_err(|err| err.to_string())
}

const OVER_BUDGET: &str = "failed to decode Protobuf message: frame exceeds decode memory budget";

/// A call's argument ids are charged before their vector grows: with the
/// arena already at the budget, a packed run of ids tips the frame over.
#[test]
fn argument_ids_are_charged() {
    let arena = nearly_full_arena(None);
    let call = decode_call(&function_call_bytes(&arena, 8, true, 0)).expect("a few ids still fit");
    assert_eq!(call.args, vec![NodeId(0); 8]);
    assert_eq!(
        decode_call(&function_call_bytes(&arena, 64, true, 0)).unwrap_err(),
        OVER_BUDGET
    );
}

/// Unpacked ids (one field per id, which our encoder never writes) decode
/// through the same charged path.
#[test]
fn unpacked_argument_ids_decode() {
    let arena = nearly_full_arena(None);
    let call = decode_call(&function_call_bytes(&arena, 3, false, 0)).expect("unpacked ids decode");
    assert_eq!(call.args, vec![NodeId(0); 3]);
    assert_eq!(
        decode_call(&function_call_bytes(&arena, 64, false, 0)).unwrap_err(),
        OVER_BUDGET
    );
}

/// Keyword pairs are charged as they are pushed.
#[test]
fn keyword_pairs_are_charged() {
    let arena = nearly_full_arena(None);
    let call = decode_call(&function_call_bytes(&arena, 0, true, 4)).expect("a few pairs still fit");
    assert_eq!(call.kwargs, vec![(NodeId(0), NodeId(0)); 4]);
    assert_eq!(
        decode_call(&function_call_bytes(&arena, 0, true, 64)).unwrap_err(),
        OVER_BUDGET
    );
}

/// A container's child ids are charged while its node decodes, before it is
/// pushed, so one huge list cannot be built past the budget. Each id costs two
/// pointers, not its 4 bytes: 16 ids are 64 bytes of `NodeId` but do not fit.
#[test]
fn container_ids_are_charged() {
    let nodes = decode(&nearly_full_arena(Some(list_node(8)))).expect("a short list still fits");
    assert_eq!(nodes.last(), Some(&MontyNode::List(vec![NodeId(0); 8])));
    for ids in [16, 64] {
        assert_eq!(
            decode(&nearly_full_arena(Some(list_node(ids)))).unwrap_err(),
            OVER_BUDGET
        );
    }
}

/// A namedtuple's field names cost a `String` slot each, charged as they arrive.
#[test]
fn named_tuple_field_names_are_charged() {
    assert_eq!(
        decode(&nearly_full_arena(Some(named_tuple_node(8)))).unwrap_err(),
        OVER_BUDGET
    );
}
