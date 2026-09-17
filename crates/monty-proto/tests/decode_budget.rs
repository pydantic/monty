//! Regression coverage for every schema repeated field and every allocation
//! primitive, with independent allocator measurements rather than just counters.

use std::{io::Cursor, panic::catch_unwind, thread};

use allocation_counter::measure;
use insta::{allow_duplicates, assert_snapshot};
use monty_proto::{
    BudgetVec, DEFAULT_MAX_DECODE_BYTES, FrameReader, WireFunctionCall, WireObject, budgeted_prost::encoding,
    decode_budget_remaining, decode_frame, pb, with_decode_budget,
};
use monty_types::{DictPairs, MontyClassInstance, MontyClassType, MontyObject, MontyType, MontyUuid};
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
        assert_eq!(charged, vector_charge::<T>(count), "{name}");
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
    let segments = repeated_field(3, WireType::LengthDelimited, &[], 100_000);
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
    let bytes = repeated_field(3, WireType::LengthDelimited, &[0x12, 1, b'x'], 1);
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

    // WireObject replaces a whole kind, so each occurrence allocates afresh.
    let wire = repeated_field(8, WireType::LengthDelimited, b"abc", 10);
    let (decoded, charged) = measured_decode::<WireObject>(&wire);
    assert_eq!(decoded.0, Some(MontyObject::String("abc".to_owned())));
    assert_eq!(charged, 30);
    with_decode_budget(29, || assert!(WireObject::decode(wire.as_slice()).is_err()));
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
    let class = MontyClassType {
        name: "Example".to_owned(),
        id: MontyUuid::from_u128(123),
        host_defined: false,
        is_dataclass: false,
        attrs: DictPairs::from(vec![(MontyObject::String("attr".to_owned()), MontyObject::None)]),
    };
    let values = [
        MontyObject::None,
        MontyObject::List(vec![MontyObject::None; 33]),
        MontyObject::Dict(DictPairs::from(vec![(MontyObject::None, MontyObject::None); 9])),
        MontyObject::NamedTuple {
            type_name: "Pair".to_owned(),
            field_names: vec!["a".to_owned(), "b".to_owned()],
            values: vec![MontyObject::None, MontyObject::String("value".to_owned())],
        },
        MontyObject::Type(MontyType::Instance(Box::new(class.clone()))),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: class,
            instance_id: MontyUuid::from_u128(456),
            attrs: DictPairs::default(),
        })),
        MontyObject::BigInt(BigInt::from_bytes_be(Sign::Minus, &[0xab; 513])),
    ];
    for expected in values {
        let wire = WireObject::new(expected.clone()).encode_to_vec();
        let (decoded, charged) = measured_decode::<WireObject>(&wire);
        assert_eq!(decoded.0, Some(expected));
        if charged > 0 {
            with_decode_budget(charged - 1, || assert!(WireObject::decode(wire.as_slice()).is_err()));
        }
    }
}

/// Named-tuple names pay for capacity and bytes, never an additional inline string header.
#[test]
fn named_tuple_has_no_double_charge() {
    let expected = MontyObject::NamedTuple {
        type_name: "T".to_owned(),
        field_names: vec!["a".to_owned()],
        values: vec![MontyObject::None],
    };
    let (_, charged) = measured_decode::<WireObject>(&WireObject::new(expected).encode_to_vec());
    assert_eq!(charged, 2 + 4 * size_of::<String>() + 4 * size_of::<MontyObject>());
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
            let wire = repeated_field(6, WireType::LengthDelimited, &payload, 1);
            let (decoded, charged) = measured_decode::<WireObject>(&wire);
            assert_eq!(decoded.0, Some(MontyObject::BigInt(expected)));
            if charged > 0 {
                with_decode_budget(charged - 1, || assert!(WireObject::decode(wire.as_slice()).is_err()));
            }
        }
    }
}

/// Diagnostic formatting must not duplicate a huge invalid name after decoding it.
#[test]
fn invalid_value_diagnostics_have_bounded_overhead() {
    let name_len = 1024 * 1024;
    let wire = repeated_field(26, WireType::LengthDelimited, &vec![b'x'; name_len], 1);
    let mut error = None;
    let allocations = measure(|| {
        with_decode_budget(name_len, || {
            error = Some(WireObject::decode(wire.as_slice()).unwrap_err());
            assert_eq!(decode_budget_remaining(), Some(0));
        });
    });
    assert!(allocations.bytes_total < (name_len + 2048) as u64, "{allocations:?}");
    assert_snapshot!(error.unwrap(), @"failed to decode Protobuf message: invalid wire value (error message exceeds 512 bytes)");
}

/// Allocation attempts outside framing cannot consume a stale or implicit budget.
#[test]
fn raw_decodes_fail_without_a_scope() {
    let print = repeated_field(3, WireType::LengthDelimited, &[], 1);
    let value = repeated_field(8, WireType::LengthDelimited, b"abc", 1);
    assert_eq!(decode_budget_remaining(), None);
    let err = pb::Print::decode(print.as_slice()).unwrap_err();
    assert_snapshot!(err, @"failed to decode Protobuf message: Print.segments: decode allocation outside a frame; use decode_frame or FrameReader");
    let err = WireObject::decode(value.as_slice()).unwrap_err();
    assert_snapshot!(err, @"failed to decode Protobuf message: decode allocation outside a frame; use decode_frame or FrameReader");
    assert_eq!(decode_budget_remaining(), None);

    // Allocation-free decoding needs no budget.
    assert_eq!(WireObject::decode(&[0x12, 0][..]).unwrap().0, Some(MontyObject::None));
}

/// A nested scope restores its caller's remaining budget, including on unwind.
#[test]
fn budgets_restore_enclosing_state() {
    assert_eq!(decode_budget_remaining(), None);
    with_decode_budget(123, || {
        with_decode_budget(7, || {
            WireObject::decode(&[0x42, 3, b'a', b'b', b'c'][..]).unwrap();
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
    let valid = repeated_field(3, WireType::LengthDelimited, &[], 1);
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
    let wire = repeated_field(3, WireType::LengthDelimited, &[], 1);
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
            let wire = repeated_field(3, WireType::LengthDelimited, &[], 1);
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
