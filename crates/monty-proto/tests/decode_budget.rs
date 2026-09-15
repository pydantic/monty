//! The per-frame decode budget must charge what the decoder actually keeps:
//! every value, the vector slots holding it, and each `Feed.inputs` entry —
//! so a frame of cheap elements cannot amplify into gigabytes of host memory.

use std::mem::size_of;

use insta::assert_snapshot;
use monty_proto::{
    DEFAULT_MAX_DECODE_BYTES, MAX_FEED_INPUTS, WireFeed, WireObject, decode_budget_remaining, decode_frame, pb,
    pb::parent_request::Kind, reset_decode_budget,
};
use monty_types::{DictPairs, MontyObject};
use prost::Message;

/// Appends `value` as a protobuf varint.
fn push_varint(mut value: usize, out: &mut Vec<u8>) {
    let low_bits = |value: usize| u8::try_from(value & 0x7f).expect("masked to seven bits");
    while value >= 0x80 {
        out.push(low_bits(value) | 0x80);
        value >>= 7;
    }
    out.push(low_bits(value));
}

/// A `ParentRequest` frame whose `Feed` carries `count` empty `NamedValue`
/// entries: two wire bytes each (`12 00`), and nothing for the value budget
/// to charge.
fn empty_inputs_frame(count: usize) -> Vec<u8> {
    let body_len = count * 2;
    let mut frame = Vec::with_capacity(body_len + 8);
    frame.push(0x1a); // `ParentRequest.feed`: field 3, length-delimited
    push_varint(body_len, &mut frame);
    for _ in 0..count {
        frame.extend_from_slice(&[0x12, 0x00]); // one empty `Feed.inputs` entry
    }
    frame
}

/// A feed request with `count` small named inputs.
fn feed_request(count: usize) -> pb::ParentRequest {
    pb::ParentRequest {
        kind: Some(Kind::Feed(WireFeed {
            code: String::new(),
            inputs: (0..count)
                .map(|i| (format!("v{i}"), MontyObject::Int(i64::try_from(i).unwrap())))
                .collect(),
            skip_type_check: false,
            cwd: String::new(),
        })),
        trace_parent: None,
    }
}

/// Bytes the budget charged to decode `value` as a bare `MontyObject`.
fn charge_for(value: &MontyObject) -> usize {
    let bytes = WireObject::new(value.clone()).encode_to_vec();
    reset_decode_budget();
    WireObject::decode(bytes.as_slice()).expect("value decodes");
    DEFAULT_MAX_DECODE_BYTES - decode_budget_remaining()
}

/// HackMonty gbkZ5p1: a million two-byte wrappers decoded into ~100 MB of
/// parent memory while charging nothing, because only values were budgeted.
/// The first valueless entry now fails the frame before any wrapper exists.
#[test]
fn empty_input_wrappers_are_rejected_before_they_are_materialized() {
    let err = decode_frame::<pb::ParentRequest>(&empty_inputs_frame(1_000_000)).expect_err("valueless inputs");
    assert_snapshot!(err, @"frame decode error: failed to decode Protobuf message: ParentRequest.kind: missing required field NamedValue.value");
    let err = decode_frame::<pb::ParentRequest>(&empty_inputs_frame(1)).expect_err("valueless input");
    assert_snapshot!(err, @"frame decode error: failed to decode Protobuf message: ParentRequest.kind: missing required field NamedValue.value");
}

/// The input count is enforced while the frame decodes, so the 257th entry
/// is refused before it is read rather than after the whole vector is built.
#[test]
fn feed_inputs_are_capped_during_decode() {
    let at_cap = feed_request(MAX_FEED_INPUTS);
    assert_eq!(
        decode_frame::<pb::ParentRequest>(&at_cap.encode_to_vec()).expect("at the cap"),
        at_cap
    );
    let err = decode_frame::<pb::ParentRequest>(&feed_request(MAX_FEED_INPUTS + 1).encode_to_vec())
        .expect_err("over the cap");
    assert_snapshot!(err, @"frame decode error: failed to decode Protobuf message: ParentRequest.kind: feed has more than 256 inputs");
}

/// HackMonty 5Xp8QGM: a container's resident size is its vector's whole
/// capacity, and a one-element list holds four slots. Each vector charges its
/// slots as it grows, so the budget tracks exactly what the decode keeps.
#[test]
fn vector_capacity_is_charged() {
    let base = MontyObject::host_base_size();
    let list = |len: usize| MontyObject::List(vec![MontyObject::None; len]);
    // The list itself, plus four slots the vector grew to for one element.
    assert_eq!(charge_for(&list(1)), base + 4 * base);
    // Four elements fill those slots; a fifth doubles the vector to eight.
    assert_eq!(charge_for(&list(4)), base + 4 * base);
    assert_eq!(charge_for(&list(5)), base + 8 * base);
    // A dict pair is two objects in one slot.
    let dict = MontyObject::Dict(DictPairs::from(vec![(MontyObject::Int(1), MontyObject::Int(2))]));
    assert_eq!(charge_for(&dict), base + 4 * 2 * base);
    // A string element charges its bytes on top of its slot.
    let strings = MontyObject::List(vec![MontyObject::String("abc".to_owned())]);
    assert_eq!(charge_for(&strings), base + 4 * base + 3);
    // Slots handed back by a credit never lift the budget past a fresh frame.
    reset_decode_budget();
    assert_eq!(decode_budget_remaining(), DEFAULT_MAX_DECODE_BYTES);
}

/// A feed input charges its value, its name's bytes, and the slot it occupies.
#[test]
fn feed_inputs_charge_their_slots() {
    let frame = pb::ParentRequest {
        kind: Some(Kind::Feed(WireFeed {
            code: "ab".to_owned(),
            inputs: vec![("ab".to_owned(), MontyObject::Int(1))],
            skip_type_check: false,
            cwd: String::new(),
        })),
        trace_parent: None,
    }
    .encode_to_vec();
    decode_frame::<pb::ParentRequest>(&frame).expect("feed decodes");
    let charged = DEFAULT_MAX_DECODE_BYTES - decode_budget_remaining();
    assert_eq!(charged, 2 + 4 * size_of::<(String, MontyObject)>());
}
