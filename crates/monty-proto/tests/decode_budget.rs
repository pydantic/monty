//! Regression coverage for every schema repeated field and every allocation
//! primitive, with independent allocator measurements rather than just counters.

use std::{io::Cursor, panic::catch_unwind, thread};

use allocation_counter::measure;
use insta::{allow_duplicates, assert_snapshot};
use monty_proto::{
    BudgetVec, DEFAULT_MAX_DECODE_BYTES, FrameReader, WireArena, WireFunctionCall, budgeted_prost::encoding,
    decode_budget_remaining, decode_frame, pb, with_decode_budget,
};
use monty_types::{ClassTypeNode, MontyNode, MontyUuid, NodeId};
use num_bigint::{BigInt, Sign};
use prost::{
    DecodeError, Message,
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType, encode_key, encode_varint},
};

include!("oracle/repeated_fields.rs");

/// Each generated repeated field must pay for its slots, including empty messages.
fn check_repeated<M: Message + Default, T>(
    name: &str,
    tag: u32,
    wire: WireType,
    payload: &[u8],
    items: impl Fn(&M) -> &BudgetVec<T>,
) {
    for count in [0, 1, 4, 5, 16, 65] {
        let bytes = repeated_field(tag, wire, payload, count);
        let (message, charged) = measured_decode::<M>(&bytes);
        assert_eq!(items(&message).len(), count, "{name}");
        assert!(charged >= items(&message).capacity() * size_of::<T>(), "{name}");
        // No fixtures have nonempty heap payloads, so only vector allocations count.
        let multiplier = if matches!(name, "monty.v1.FunctionCall.args" | "monty.v1.FunctionCall.kwargs") {
            2 * size_of::<usize>() / size_of::<NodeId>()
        } else {
            1
        };
        assert_eq!(charged, vector_charge::<T>(count) * multiplier, "{name}");
        if count > 0 {
            with_decode_budget(charged - 1, || {
                assert!(M::decode(bytes.as_slice()).is_err(), "{name}");
            });
        }
    }
}

/// Measures successful decoding separately from wire construction and assertions.
fn measured_decode<M: Message + Default>(bytes: &[u8]) -> (M, usize) {
    let mut decoded = None;
    let mut charged = 0;
    let allocation = measure(|| {
        with_decode_budget(DEFAULT_MAX_DECODE_BYTES, || {
            decoded = Some(M::decode(bytes).expect("valid fixture"));
            charged = DEFAULT_MAX_DECODE_BYTES - decode_budget_remaining().unwrap();
        });
    });
    assert!(
        allocation.bytes_total <= charged as u64,
        "{allocation:?}, charged {charged}"
    );
    assert!(
        allocation.bytes_max <= charged as u64,
        "{allocation:?}, charged {charged}"
    );
    (decoded.unwrap(), charged)
}

/// Cumulative allocation requests for the decoder's geometric vector growth.
fn vector_charge<T>(len: usize) -> usize {
    let mut capacity = 0;
    let mut total = 0;
    while capacity < len {
        capacity = (capacity * 2).max(4);
        total += capacity * size_of::<T>();
    }
    total
}

/// Builds wire fixtures without creating any decoded wrappers.
fn repeated_field(tag: u32, wire: WireType, payload: &[u8], count: usize) -> Vec<u8> {
    let mut field = Vec::new();
    encode_key(tag, wire, &mut field);
    if wire == WireType::LengthDelimited {
        encode_varint(payload.len() as u64, &mut field);
    }
    field.extend_from_slice(payload);
    field.repeat(count)
}

/// Empty generated wrappers used to bypass value-only accounting entirely.
#[test]
fn generated_wrapper_attacks_are_bounded() {
    let traceback = repeated_field(3, WireType::LengthDelimited, &[], 100_000);
    let futures = repeated_field(1, WireType::LengthDelimited, &[], 100_000);
    let segments = repeated_field(1, WireType::LengthDelimited, &[], 100_000);
    let inputs = repeated_field(2, WireType::LengthDelimited, &[], 100_000);
    with_decode_budget(4096, || {
        let err = pb::RaisedException::decode(traceback.as_slice()).unwrap_err();
        assert_snapshot!(err, @"failed to decode Protobuf message: RaisedException.traceback: frame exceeds decode memory budget");
    });
    with_decode_budget(4096, || {
        let err = pb::ResumeFutures::decode(futures.as_slice()).unwrap_err();
        assert_snapshot!(err, @"failed to decode Protobuf message: ResumeFutures.results: frame exceeds decode memory budget");
    });
    with_decode_budget(4096, || {
        let err = pb::Print::decode(segments.as_slice()).unwrap_err();
        assert_snapshot!(err, @"failed to decode Protobuf message: Print.segments: frame exceeds decode memory budget");
    });
    with_decode_budget(4096, || {
        let err = pb::Feed::decode(inputs.as_slice()).unwrap_err();
        assert_snapshot!(err, @"failed to decode Protobuf message: Feed.inputs: frame exceeds decode memory budget");
    });
}

/// A denied growth must not allocate the replacement buffer or decode its payload.
#[test]
fn growth_is_rejected_before_allocating() {
    let mut print = pb::Print {
        segments: vec![pb::PrintSegment::default(); 1024].into(),
    };
    let bytes = repeated_field(1, WireType::LengthDelimited, &[0x12, 1, b'x'], 1);
    let capacity = print.segments.capacity();
    let mut error = None;
    let allocations = measure(|| {
        with_decode_budget(0, || error = Some(print.merge(bytes.as_slice()).unwrap_err()));
    });
    assert_eq!(print.segments.len(), 1024);
    assert_eq!(print.segments.capacity(), capacity);
    // Only the bounded DecodeError and its field path may allocate.
    assert!(allocations.bytes_total < 1024, "{allocations:?}");
    assert_snapshot!(error.unwrap(), @"failed to decode Protobuf message: Print.segments: frame exceeds decode memory budget");
}

/// Host/domain conversions preserve the same allocation without charging it again.
#[test]
fn budget_vectors_transfer_existing_storage_without_copying() {
    let original = vec![1u32, 2, 3, 4];
    let pointer = original.as_ptr();
    let capacity = original.capacity();
    let mut restored = None;
    let allocations = measure(|| {
        with_decode_budget(0, || {
            let mut values = BudgetVec::from(original);
            values.truncate(1);
            values.try_push(5).unwrap();
            restored = Some(values.into_inner());
            assert_eq!(decode_budget_remaining(), Some(0));
        });
    });
    let restored = restored.unwrap();
    assert_eq!(restored, vec![1, 5]);
    assert_eq!(restored.as_ptr(), pointer);
    assert_eq!(restored.capacity(), capacity);
    assert_eq!(allocations.bytes_total, 0);
}

/// Fallible insertion preserves the vector on failure and never refunds cleared storage.
#[test]
fn budget_vectors_reject_growth_without_losing_elements() {
    with_decode_budget(12 * size_of::<u64>(), || {
        let mut values = BudgetVec::new();
        for value in 0u64..8 {
            values.try_push(value).unwrap();
        }
        let pointer = values.as_ptr();
        assert_eq!(decode_budget_remaining(), Some(0));
        assert_snapshot!(values.try_push(8).unwrap_err(), @"failed to decode Protobuf message: frame exceeds decode memory budget");
        assert_eq!(values.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(values.as_ptr(), pointer);
        assert_eq!(values.capacity(), 8);
        values.clear();
        values.try_push(9).unwrap();
        assert_eq!(values.as_ptr(), pointer);
        assert_eq!(decode_budget_remaining(), Some(0));
    });
}

/// Buffers not all present in today's schema still need a guarded adapter.
#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "monty_proto::budgeted_prost")]
struct Buffers {
    #[prost(string, tag = "1")]
    text: String,
    #[prost(bytes = "vec", tag = "2")]
    data: BudgetVec<u8>,
    #[prost(string, repeated, tag = "3")]
    texts: BudgetVec<String>,
    #[prost(bytes = "vec", repeated, tag = "4")]
    chunks: BudgetVec<BudgetVec<u8>>,
}

/// Strings/bytes share one owned allocation, even with fragmented input buffers.
#[test]
fn buffer_payloads_and_repeated_slots_are_charged_once() {
    let expected = Buffers {
        text: "abc".to_owned(),
        data: vec![1, 2, 3, 4, 5].into(),
        texts: vec!["hello".to_owned(), "world".to_owned()].into(),
        chunks: vec![vec![1; 7].into(), BudgetVec::new()].into(),
    };
    let wire = expected.encode_to_vec();
    let (actual, charged) = measured_decode::<Buffers>(&wire);
    assert_eq!(actual, expected);
    assert_eq!(
        charged,
        3 + 5 + 10 + 7 + 4 * size_of::<String>() + 4 * size_of::<Vec<u8>>()
    );
    for split in 0..wire.len() {
        with_decode_budget(charged, || {
            let fragmented = wire[..split].chain(&wire[split..]);
            assert_eq!(Buffers::decode(fragmented).unwrap(), expected);
            assert_eq!(decode_budget_remaining(), Some(0));
        });
    }
    with_decode_budget(charged - 1, || assert!(Buffers::decode(wire.as_slice()).is_err()));
}

/// Last-one-wins strings reuse storage but larger replacements pay in full.
#[test]
fn duplicate_buffers_and_oneofs_cannot_refund_budget() {
    let mut wire = repeated_field(1, WireType::LengthDelimited, b"abc", 1);
    wire.extend(repeated_field(1, WireType::LengthDelimited, b"abcdef", 1));
    wire.extend(repeated_field(1, WireType::LengthDelimited, b"x", 1));
    let (decoded, charged) = measured_decode::<Buffers>(&wire);
    assert_eq!(decoded.text, "x");
    assert_eq!(charged, 9);

    // The node decoder replaces a whole kind, so each occurrence allocates afresh.
    let node = repeated_field(8, WireType::LengthDelimited, b"abc", 10);
    let wire = repeated_field(2, WireType::LengthDelimited, &node, 1);
    let (decoded, charged) = measured_decode::<WireArena>(&wire);
    assert_eq!(decoded.0, vec![MontyNode::String("abc".to_owned())]);
    assert_eq!(charged, 30 + vector_charge::<MontyNode>(1));
    with_decode_budget(charged - 1, || assert!(WireArena::decode(wire.as_slice()).is_err()));
}

/// Malformed delimiters cannot cause claimed-length allocation; invalid UTF-8 is rejected.
#[test]
fn malformed_buffers_are_rejected() {
    for wire in [vec![0x0a, 0xff, 0xff, 0xff, 0x7f], vec![0x0a, 10, 1]] {
        with_decode_budget(0, || {
            let err = Buffers::decode(wire.as_slice()).unwrap_err();
            assert_eq!(decode_budget_remaining(), Some(0));
            allow_duplicates! {
                assert_snapshot!(err, @"failed to decode Protobuf message: Buffers.text: buffer underflow");
            }
        });
    }
    let err = with_decode_budget(1, || Buffers::decode(&[0x0a, 1, 0xff][..]).unwrap_err());
    assert_snapshot!(err, @"failed to decode Protobuf message: Buffers.text: invalid string value: data is not UTF-8 encoded");
}

/// Tests both representations of every numeric primitive, not just current schema types.
#[test]
fn packed_and_unpacked_scalars_are_budgeted() {
    macro_rules! check {
        ($module:ident, $value:expr) => {{
            let expected = vec![$value; 17];
            for packed in [false, true] {
                let mut wire = Vec::new();
                if packed {
                    encoding::$module::encode_packed(1, &expected, &mut wire);
                } else {
                    encoding::$module::encode_repeated(1, &expected, &mut wire);
                }
                let decode = || {
                    let mut values = BudgetVec::new();
                    let mut buf = wire.as_slice();
                    while buf.has_remaining() {
                        let (_, wire_type) = encoding::decode_key(&mut buf)?;
                        encoding::$module::merge_repeated(wire_type, &mut values, &mut buf, DecodeContext::default())?;
                    }
                    Ok::<_, DecodeError>(values)
                };
                let budget = (4 + 8 + 16 + 32) * size_of_val(&$value);
                with_decode_budget(budget - 1, || assert!(decode().is_err()));
                with_decode_budget(budget, || {
                    assert_eq!(decode().unwrap(), expected);
                    assert_eq!(decode_budget_remaining(), Some(0));
                });
            }
        }};
    }
    check!(bool, true);
    check!(int32, -123i32);
    check!(int64, -123i64);
    check!(uint32, 123u32);
    check!(uint64, 123u64);
    check!(sint32, -123i32);
    check!(sint64, -123i64);
    check!(fixed32, 123u32);
    check!(fixed64, 123u64);
    check!(sfixed32, -123i32);
    check!(sfixed64, -123i64);
    check!(float, 1.5f32);
    check!(double, 1.5f64);

    with_decode_budget(0, || {
        let mut values = BudgetVec::new();
        encoding::uint32::merge_repeated(
            WireType::LengthDelimited,
            &mut values,
            &mut &[0][..],
            DecodeContext::default(),
        )
        .unwrap();
        assert!(values.is_empty());
    });
}

/// Hand-written containers and boxes use the same allocation owners as generated fields.
#[test]
fn value_allocations_are_measured_independently() {
    let class = ClassTypeNode {
        name: "Example".to_owned(),
        id: MontyUuid::from_u128(123),
        host_defined: false,
        is_dataclass: false,
        attrs: vec![(NodeId(0), NodeId(1))],
    };
    let values = [
        MontyNode::None,
        MontyNode::List(vec![NodeId(1); 33]),
        MontyNode::Dict(vec![(NodeId(1), NodeId(1)); 9]),
        MontyNode::NamedTuple {
            type_name: "Pair".to_owned(),
            field_names: vec!["a".to_owned(), "b".to_owned()],
            values: vec![NodeId(1), NodeId(0)],
        },
        MontyNode::ClassType(Box::new(class.clone())),
        MontyNode::ClassInstance {
            class_type: NodeId(2),
            instance_id: MontyUuid::from_u128(456),
            attrs: vec![],
        },
        MontyNode::BigInt(BigInt::from_bytes_be(Sign::Minus, &[0xab; 513])),
    ];
    for value in values {
        let expected = WireArena(
            vec![
                MontyNode::String("attr".to_owned()),
                MontyNode::None,
                MontyNode::ClassType(Box::new(class.clone())),
                value,
            ]
            .into(),
        );
        let wire = expected.encode_to_vec();
        let (decoded, charged) = measured_decode::<WireArena>(&wire);
        assert_eq!(decoded, expected);
        decoded.into_graph().unwrap();
        with_decode_budget(charged - 1, || assert!(WireArena::decode(wire.as_slice()).is_err()));
    }
}

/// Named-tuple names pay for capacity and bytes, never an additional inline string header.
#[test]
fn named_tuple_has_no_double_charge() {
    let expected = WireArena(
        vec![
            MontyNode::None,
            MontyNode::NamedTuple {
                type_name: "T".to_owned(),
                field_names: vec!["a".to_owned()],
                values: vec![NodeId(0)],
            },
        ]
        .into(),
    );
    let (_, charged) = measured_decode::<WireArena>(&expected.encode_to_vec());
    assert_eq!(
        charged,
        2 + 4 * size_of::<String>() + 2 * size_of::<MontyNode>() + 2 * size_of::<usize>()
    );
}

/// BigInt padding and limb boundaries cannot hide temporary or normalization allocations.
#[test]
fn bigint_conversion_is_preflighted() {
    for len in 0..128 {
        for padding in [0, 256] {
            let mut magnitude = vec![0; padding];
            magnitude.extend(vec![0xab; len]);
            let expected = BigInt::from_bytes_be(Sign::Plus, &magnitude);
            let payload = pb::BigInt {
                negative: false,
                magnitude: magnitude.into(),
            }
            .encode_to_vec();
            let node = repeated_field(6, WireType::LengthDelimited, &payload, 1);
            let wire = repeated_field(2, WireType::LengthDelimited, &node, 1);
            let (decoded, charged) = measured_decode::<WireArena>(&wire);
            assert_eq!(decoded.0, vec![MontyNode::BigInt(expected)]);
            with_decode_budget(charged - 1, || assert!(WireArena::decode(wire.as_slice()).is_err()));
        }
    }
}

/// Diagnostic formatting must not duplicate a huge invalid name after decoding it.
#[test]
fn invalid_value_diagnostics_have_bounded_overhead() {
    let name_len = 1024 * 1024;
    let node = repeated_field(26, WireType::LengthDelimited, &vec![b'x'; name_len], 1);
    let wire = repeated_field(2, WireType::LengthDelimited, &node, 1);
    let mut error = None;
    let allocations = measure(|| {
        with_decode_budget(name_len, || {
            error = Some(WireArena::decode(wire.as_slice()).unwrap_err());
            assert_eq!(decode_budget_remaining(), Some(0));
        });
    });
    assert!(allocations.bytes_total < (name_len + 2048) as u64, "{allocations:?}");
    assert_snapshot!(error.unwrap(), @"failed to decode Protobuf message: invalid wire value (error message exceeds 512 bytes)");
}

/// Allocation attempts outside framing cannot consume a stale or implicit budget.
#[test]
fn raw_decodes_fail_without_a_scope() {
    let print = repeated_field(1, WireType::LengthDelimited, &[], 1);
    let node = repeated_field(8, WireType::LengthDelimited, b"abc", 1);
    let value = repeated_field(2, WireType::LengthDelimited, &node, 1);
    assert_eq!(decode_budget_remaining(), None);
    let err = pb::Print::decode(print.as_slice()).unwrap_err();
    assert_snapshot!(err, @"failed to decode Protobuf message: Print.segments: decode allocation outside a frame; use decode_frame or FrameReader");
    let err = WireArena::decode(value.as_slice()).unwrap_err();
    assert_snapshot!(err, @"failed to decode Protobuf message: decode allocation outside a frame; use decode_frame or FrameReader");
    assert_eq!(decode_budget_remaining(), None);

    // Allocation-free decoding needs no budget.
    assert_eq!(WireArena::decode(&[][..]).unwrap(), WireArena::default());
}

/// A nested scope restores its caller's remaining budget, including on unwind.
#[test]
fn budgets_restore_enclosing_state() {
    assert_eq!(decode_budget_remaining(), None);
    with_decode_budget(123, || {
        with_decode_budget(7, || {
            Buffers::decode(&[0x0a, 3, b'a', b'b', b'c'][..]).unwrap();
            assert_eq!(decode_budget_remaining(), Some(4));
        });
        assert_eq!(decode_budget_remaining(), Some(123));
        assert!(catch_unwind(|| with_decode_budget(0, || panic!("test unwind"))).is_err());
        assert_eq!(decode_budget_remaining(), Some(123));
    });
    assert_eq!(decode_budget_remaining(), None);
}

/// Each framing entry point has an independent scope, even after a failed frame.
#[test]
fn frame_entry_points_scope_success_and_failure() {
    let valid = repeated_field(1, WireType::LengthDelimited, &[], 1);
    let mut invalid = valid.clone();
    invalid.push(0); // invalid key, after allocating a repeated slot
    let frames = [valid.as_slice(), invalid.as_slice(), valid.as_slice()];
    let mut framed = Vec::new();
    for bytes in frames {
        framed.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
        framed.extend_from_slice(bytes);
    }
    let mut reader = FrameReader::new(Cursor::new(framed));
    with_decode_budget(0, || {
        for (index, bytes) in frames.into_iter().enumerate() {
            let direct = decode_frame::<pb::Print>(bytes);
            assert_eq!(decode_budget_remaining(), Some(0));
            let streamed = reader.read::<pb::Print>();
            assert_eq!(decode_budget_remaining(), Some(0));
            if index == 1 {
                assert_snapshot!(direct.unwrap_err(), @"frame decode error: failed to decode Protobuf message: invalid tag value: 0");
                assert_snapshot!(streamed.unwrap_err(), @"frame decode error: failed to decode Protobuf message: invalid tag value: 0");
            } else {
                assert_eq!(direct.unwrap().segments.len(), 1);
                assert_eq!(streamed.unwrap().unwrap().segments.len(), 1);
            }
        }
        assert_eq!(reader.read::<pb::Print>().unwrap(), None);
        assert_eq!(decode_budget_remaining(), Some(0));
    });
    assert_eq!(decode_budget_remaining(), None);
    assert_eq!(decode_frame::<pb::Print>(&valid).unwrap().segments.len(), 1);
    assert_eq!(decode_budget_remaining(), None);
}

/// Unwinding through either frame entry point must release its budget scope.
#[test]
fn frame_entry_points_restore_state_on_unwind() {
    let wire = repeated_field(1, WireType::LengthDelimited, &[], 1);
    with_decode_budget(123, || {
        assert!(catch_unwind(|| decode_frame::<PanicOnDecode>(&wire)).is_err());
        assert_eq!(decode_budget_remaining(), Some(123));
        let mut framed = u32::try_from(wire.len()).unwrap().to_le_bytes().to_vec();
        framed.extend_from_slice(&wire);
        assert!(catch_unwind(|| FrameReader::new(Cursor::new(framed)).read::<PanicOnDecode>()).is_err());
        assert_eq!(decode_budget_remaining(), Some(123));
    });
    assert_eq!(decode_budget_remaining(), None);
}

/// A frame running on another thread cannot inherit or consume this thread's budget.
#[test]
fn scopes_are_thread_local() {
    with_decode_budget(0, || {
        thread::spawn(|| {
            assert_eq!(decode_budget_remaining(), None);
            let wire = repeated_field(1, WireType::LengthDelimited, &[], 1);
            assert_eq!(decode_frame::<pb::Print>(&wire).unwrap().segments.len(), 1);
            assert_eq!(decode_budget_remaining(), None);
        })
        .join()
        .unwrap();
        assert_eq!(decode_budget_remaining(), Some(0));
    });
    assert_eq!(decode_budget_remaining(), None);
}

/// Decode-only fixture that panics after a successful, budgeted allocation.
#[derive(Debug, Default)]
struct PanicOnDecode;

impl Message for PanicOnDecode {
    fn encode_raw(&self, _buf: &mut impl BufMut) {
        unreachable!("decode-only fixture")
    }

    fn encoded_len(&self) -> usize {
        unreachable!("decode-only fixture")
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        pb::Print::default().merge_field(tag, wire_type, buf, ctx)?;
        panic!("test unwind after allocation")
    }

    fn clear(&mut self) {}
}

/// A small frame budget exercises arena exhaustion without allocating a GiB.
const ARENA_TEST_BUDGET: usize = 4096;

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
    with_decode_budget(ARENA_TEST_BUDGET, || WireArena::decode(bytes))
        .map(|arena| arena.0.into_inner())
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
    let too_many = ARENA_TEST_BUDGET / size_of::<MontyNode>() + 1;
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
    let full = ARENA_TEST_BUDGET / size_of::<MontyNode>() - 1;
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
    with_decode_budget(ARENA_TEST_BUDGET, || WireFunctionCall::decode(bytes)).map_err(|err| err.to_string())
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
